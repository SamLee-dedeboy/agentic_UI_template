# Getting started

A hands-on walkthrough of the template using the three example tools it
ships with. Follow this once, top to bottom, and you'll understand how
to add your own.

- Looking for the high-level pitch, configuration reference, and
  architecture diagrams? → [main README](../README.md).
- Looking for a deep dive on tool plumbing and conventions? →
  [docs/tools.md](./tools.md).

## 1. Run the template

Prerequisites: [Bun](https://bun.sh) 1.3+, Rust 1.70+ with cargo, and
the [Claude Code CLI](https://claude.ai/code) on `PATH`.

```bash
bun install
bun run build                      # compiles the frontend into dist/
cd backend
cargo build --bins                 # builds claude-ui-app + tool-bridge
cargo run --bin claude-ui-app      # serves on http://127.0.0.1:8080
```

Open the URL. The welcome panel explains server tools, client tools,
and the chat shell, plus three trigger prompts — one per shipped tool.

> **After editing any Rust file under `backend/src/`**: stop the running
> process (Ctrl+C), `cargo build --bins` again, restart, and click
> "New chat" in the UI. The running backend has its binary loaded in
> memory and won't pick up rebuilds otherwise; within a chat, Claude
> caches prior tool results in context so stale state can mask the fix.

## 2. Trace the server-tool example: `get_weather`

Click **"What's the weather like in Tokyo?"**. Here's what happens.

**Declaration** — [backend/src/main.rs](../backend/src/main.rs):

```rust
b.server_tool(
    "get_weather",
    "Return the current weather for a city. …",
    json!({ "type": "object", "properties": { "location": { "type": "string" } }, "required": ["location"] }),
    |input| async move { examples::weather::fetch(input).await },
);
```

**Implementation** — [backend/src/examples/weather.rs](../backend/src/examples/weather.rs):
a real call to the free [Open-Meteo](https://open-meteo.com) API (no
key required). Geocodes the city, fetches current weather, maps WMO
weather codes to human summaries.

**What happens when you send the prompt:**

1. The browser opens a WebSocket and sends your prompt.
2. The backend spawns `claude` with `--mcp-config` pointing at the
   `tool-bridge` binary.
3. Claude reads the tool description, decides to call `get_weather`
   with `{ "location": "Tokyo" }`.
4. The bridge forwards the call to `/__tools/dispatch` on the main
   server. The Rust handler runs, hits Open-Meteo, returns JSON.
5. Claude gets the JSON as a `tool_result`, writes a natural-language
   reply. The `WeatherResultCard` component renders the tool result as
   a card with an icon + temperature instead of raw JSON.

Try the same prompt with a different city — "Oslo", "Mumbai", "São
Paulo" — and you'll get real, different weather each time.

**Make it yours:** swap the Open-Meteo call for your weather provider.
Only the body of `examples::weather::fetch` changes; the rest of the
plumbing — schema validation, MCP wiring, result-card rendering — stays
exactly the same.

## 3. Trace the client-tool example: `show_choice`

Click **"Help me pick a color for a new website: blue, green, or
purple."**. Claude calls a *client* tool — one where the handler is a
React component, not Rust.

Client tools live in **three** places. Understanding why there are
three is the key to the template.

**Place 1** — backend declaration so Claude knows the tool exists.
[backend/src/main.rs](../backend/src/main.rs):

```rust
b.client_tool(
    "show_choice",
    "Present the user with a short list of choices and wait for them to pick one. …",
    json!({
        "type": "object",
        "properties": {
            "prompt":  { "type": "string" },
            "options": { "type": "array", "items": { "type": "string" }, "minItems": 2 }
        },
        "required": ["prompt", "options"]
    }),
);
```

No handler! Client tools don't have a Rust handler; their handler is
the React component.

**Place 2** — the React component that renders the tool.
[src/core/tools/builtins/ShowChoice.tsx](../src/core/tools/builtins/ShowChoice.tsx):

```tsx
export function ShowChoice({ input, resolve }: ClientToolProps<...>) {
  return (
    <div>
      <p>{input.prompt}</p>
      {input.options.map((opt, i) => (
        <Button key={i} onClick={() => resolve({ index: i, value: opt })}>
          {opt}
        </Button>
      ))}
    </div>
  );
}
```

Two props: `input` (the object Claude passed, matching the schema
above) and `resolve(value)` (the function you call with the result).
Calling `resolve` is what returns a `tool_result` to Claude.

**Place 3** — wire the component to the tool name, and optionally add a
result renderer for a nicer confirmation bubble.
[src/main.tsx](../src/main.tsx):

```tsx
registerClientTool("show_choice", ShowChoice);
registerToolResult("show_choice", ChoiceResultCard); // optional
```

The string must match the name in `b.client_tool(...)` exactly.

**What happens when you send the prompt:**

1. Claude calls `show_choice` with
   `{ "prompt": "Pick a color", "options": ["blue", "green", "purple"] }`.
2. The bridge forwards the call to `/__tools/dispatch` like before —
   but this time the main server sees it's a client tool, so it
   generates a `tool_call_id`, parks a `oneshot::Sender`, and pushes a
   `tool_call_for_ui` event down the WebSocket.
3. The chat view pairs the pending call to the preceding `tool_use`
   block (matching by tool name + input equality, since backend's
   `tool_call_id` and Claude's `tool_use.id` aren't linked on the wire)
   and renders `<ShowChoice>` inline with `input` + `resolve` wired up.
4. You click a button. `resolve({ index: 1, value: "green" })` fires.
5. The browser sends `tool_result_from_ui` back over the WebSocket.
   The backend wakes the parked oneshot; the original HTTP dispatch
   returns; the bridge hands the result to Claude.
6. Claude writes "Green it is — shall I…" and the stream continues.
   `ChoiceResultCard` renders the resolved tool_result as a green-check
   pill.

If you don't click within `APP_CLIENT_TOOL_TIMEOUT_SECS` (default
120 s), Claude gets a timeout error and the card renders a "Selected
(unknown)" fallback so the UI stays sensible.

## 4. Trace the paired example: `search_flights` → `show_flight_options`

Click **"Find me flights from SFO to Tokyo on 2026-05-10."**. This one
exercises the full pattern: a domain **server tool** produces data,
then Claude calls a **client tool** to have the user pick from it.

1. Claude calls `search_flights({ origin: "SFO", destination: "Tokyo",
   date: "2026-05-10" })`. The handler
   ([backend/src/examples/flights.rs](../backend/src/examples/flights.rs))
   is a deterministic procedural generator seeded by the input — same
   query always returns the same three flights, different queries
   diverge. No paid aggregator required for the demo.
2. Claude reads the result and calls
   `show_flight_options({ origin, destination, date, flights })`. The
   `<FlightResults>` component renders a card per flight with airline,
   flight number, times, duration, stops, cabin, and a "Select" button.
3. You click "Select" on a flight. `resolve({ picked_id, picked_flight })`
   fires; the result flows back to Claude; Claude confirms.
   `<FlightPickResultCard>` renders the picked flight as a summary.

Run the same prompt with different routes — JFK → LHR, Paris →
Bangkok — and you'll see different airlines (Air Canada, Lufthansa,
ANA, Emirates, …) and prices/times per route. IATA codes for ~30
common cities are resolved; anything else falls back to the first three
letters of the name.

## 5. Modify an example

Two quick experiments to build intuition.

**A. Swap weather providers.** Open
[backend/src/examples/weather.rs](../backend/src/examples/weather.rs),
replace the Open-Meteo URLs with your preferred API (AccuWeather,
Pirate Weather, whatever), return the same JSON shape. Rebuild, restart,
click "New chat", run the Tokyo prompt again. The rest of the stack
doesn't notice.

**B. Add a "business class only" filter to flights.** Edit
`examples::flights::search` to take an optional `cabin` argument (you'd
also add it to the schema in `main.rs`), filter the generator output.
Claude will pick up the new field from the description and pass it
when relevant.

## 6. Build your prototype

To replace the examples with your research tools:

1. Delete the `get_weather`, `show_choice`, `search_flights`,
   `show_flight_options` registrations from
   [backend/src/main.rs](../backend/src/main.rs). Register your own
   `server_tool(...)` and `client_tool(...)` calls in the same
   `build_tool_registry()` function.
2. Delete `backend/src/examples/` (or leave as reference) and add your
   own domain module.
3. Replace the `src/core/tools/builtins/*.tsx` components with your own
   under `src/features/<your-app>/`. Register via `registerClientTool`
   and `registerToolResult` in `src/main.tsx`.
4. Tweak the empty-state copy in
   [src/core/components/ChatView.tsx](../src/core/components/ChatView.tsx)
   so the suggested prompts exercise *your* tools, not the defaults.
5. Swap the page title in [index.html](../index.html).

## Where to go next

- **[Main README](../README.md)** — configuration env vars, project
  layout, prerequisites, known gaps.
- **[docs/tools.md](./tools.md)** — the complete tool reference:
  server vs client decision matrix, schema conventions, end-to-end
  flow with the exact WebSocket/HTTP frames, tool-result renderers,
  and debugging tips.
- **[CLAUDE.md](../CLAUDE.md)** — architecture notes, CLI invocation
  shape, gotchas. Primarily for Claude Code itself when working on
  this repo, but useful for humans too.
