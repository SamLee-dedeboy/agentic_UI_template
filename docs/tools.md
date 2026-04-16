# Adding tools

Tools are the core extension point for this template. Every domain
action your Claude-powered app can take — search flights, render a seat
map, charge a card, ask the user to pick a color — is a tool.

This doc covers:

1. When to pick a server tool vs a client tool
2. How to register each kind
3. How the plumbing works end-to-end (so you can debug when it breaks)
4. Conventions for keeping schemas, names, and components in sync

## Server vs client tool

**Server tools** run inside the Axum server, in Rust. Use them when the
action:

- hits an external API (flight search, payment gateway, weather service)
- reads or writes your database
- is deterministic data retrieval or transformation
- shouldn't need the user to do anything

**Client tools** run in the browser, as React components. Use them when
the action:

- is fundamentally interactive (seat picker, confirmation prompt,
  credit-card form)
- needs to render domain-specific UI mid-conversation
- needs information that only lives in the user's browser (geolocation,
  the current viewport, a local draft they're editing)

Client tools block Claude until the user responds. If the user doesn't
respond within `APP_CLIENT_TOOL_TIMEOUT_SECS` (default 120 s), Claude
gets a timeout error and the conversation continues. Keep the UX
immediate — don't use a client tool where the user might alt-tab for
five minutes.

## Registering a server tool

Two steps, both in `backend/src/main.rs`:

```rust
use serde_json::json;

let mut b = core::tools::ToolRegistry::builder();

b.server_tool(
    "search_flights",
    "Search available flights. Use this whenever the user asks about \
     flying somewhere or wants to see options.",
    json!({
        "type": "object",
        "properties": {
            "origin":      { "type": "string", "description": "IATA code or city name" },
            "destination": { "type": "string", "description": "IATA code or city name" },
            "date":        { "type": "string", "description": "YYYY-MM-DD" }
        },
        "required": ["origin", "destination", "date"],
        "additionalProperties": false
    }),
    |input| async move {
        let origin = input.get("origin").and_then(|v| v.as_str()).unwrap_or("");
        let destination = input.get("destination").and_then(|v| v.as_str()).unwrap_or("");
        let date = input.get("date").and_then(|v| v.as_str()).unwrap_or("");
        // Hit your real flight API here.
        Ok(json!({
            "flights": [
                { "id": "AC123", "price_usd": 640, "depart": "09:10", "arrive": "12:30" },
                { "id": "LH456", "price_usd": 715, "depart": "15:45", "arrive": "19:05" }
            ]
        }))
    },
);
```

That's it — no React, no WebSocket. The tool is automatically:

- exposed to Claude on every spawn via the MCP bridge
- auto-approved (`--allowed-tools`) so it doesn't prompt
- reachable as `mcp__template-tools__search_flights` in Claude's
  `tool_use` blocks

## Registering a client tool

Three places, one per step. A client tool's React component is the
handler — but Claude only knows the tool exists if you also declare it
on the backend.

### 1. Backend declaration (`backend/src/main.rs`)

```rust
b.client_tool(
    "show_flight_options",
    "Render a list of flights for the user to pick from. Call this \
     after you've searched, to let them choose one before booking.",
    json!({
        "type": "object",
        "properties": {
            "flights": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id":        { "type": "string" },
                        "price_usd": { "type": "number" },
                        "depart":    { "type": "string" },
                        "arrive":    { "type": "string" }
                    }
                }
            }
        },
        "required": ["flights"]
    }),
);
```

Notice: no handler. Client tools have no server-side dispatch — the
runtime is the React registry.

### 2. React component (`src/features/your-app/FlightOptions.tsx` or wherever)

```tsx
import type { ClientToolProps } from "@/core/tools/registry";
import { Button } from "@/components/ui/button";

type Flight = { id: string; price_usd: number; depart: string; arrive: string };

export function FlightOptions({
  input,
  resolve,
}: ClientToolProps<{ flights: Flight[] }, { picked_id: string }>) {
  return (
    <div className="space-y-2">
      {input.flights.map((f) => (
        <button
          key={f.id}
          className="block w-full rounded border p-3 text-left hover:bg-accent"
          onClick={() => resolve({ picked_id: f.id })}
        >
          <div className="font-medium">{f.id}</div>
          <div className="text-xs text-muted-foreground">
            {f.depart} → {f.arrive} · ${f.price_usd}
          </div>
        </button>
      ))}
    </div>
  );
}
```

### 3. Register the component (`src/main.tsx`)

```tsx
import {
  registerClientTool,
  registerToolResult,
} from "@/core/tools/registry";
import { FlightOptions } from "./features/your-app/FlightOptions";
import { FlightPickResultCard } from "./features/your-app/FlightPickResultCard";

registerClientTool("show_flight_options", FlightOptions);
registerToolResult("show_flight_options", FlightPickResultCard); // optional
```

The name in `registerClientTool` has to match the name in
`b.client_tool(...)` exactly. That's the contract.

## Rendering the result: `registerToolResult`

By default the `tool_result` bubble that follows a `tool_use` renders
as a collapsible `<details>` with pretty-printed JSON. For anything
customer-facing that's not enough — a picked flight should render as
a confirmation card, a weather result as a temperature chip with an
icon, and so on.

Register a result renderer by tool name:

```tsx
import { registerToolResult, type ToolResultProps } from "@/core/tools/registry";

function FlightPickResultCard({
  content,
}: ToolResultProps<{ picked_id: string; picked_flight: Flight }>) {
  const f = content?.picked_flight;
  return <div>Booked {f?.airline} {f?.flight_number} for ${f?.price_usd}</div>;
}

registerToolResult("show_flight_options", FlightPickResultCard);
```

`content` is the parsed JSON value Claude saw — the Rust handler's
return value for server tools, or whatever the client component
`resolve()`d with for client tools. If the result wasn't valid JSON
(plain-text error, malformed payload), `content` is the raw string
instead, so components should degrade gracefully.

Works the same for server and client tools — the `tool_result` wire
shape is identical either way. See `WeatherResultCard`,
`ChoiceResultCard`, `FlightPickResultCard` under
`src/core/tools/builtins/` for ready references.

## End-to-end flow (server tool)

1. User sends a prompt over WebSocket.
2. Backend spawns `claude` with `--mcp-config <tmp.json>` pointing at
   the `tool-bridge` binary.
3. Claude calls `mcp__template-tools__search_flights`. MCP delivers the
   call to the bridge via stdio.
4. The bridge POSTs to `http://127.0.0.1:<port>/__tools/dispatch` with
   `X-Tool-Bridge-Secret: <per-spawn-secret>`.
5. The main server authorizes the secret, looks up the tool, sees it's
   a server tool, runs the Rust closure.
6. The closure returns JSON. The main server wraps it as
   `{ success: true, data: <json> }`.
7. The bridge translates that into an MCP `tool_result` and returns it
   to Claude.
8. Claude continues the turn with the tool result in context.

## End-to-end flow (client tool)

Same as above up through step 5, then diverges:

5. The main server authorizes the secret, looks up the tool, sees it's
   a **client** tool.
6. It generates a random `tool_call_id`, stashes a `oneshot::Sender`
   keyed by that id in `pending_client_tools`, then sends a
   `tool_call_for_ui` message on the session's WebSocket.
7. The frontend's `apiAdapter.ts` routes the message into a session-
   scoped `claude-tool-call:<sessionId>` DOM event.
8. `useClaudeSession` adds it to `pendingToolCalls`. `<ChatView>`
   correlates the pending call to the matching `tool_use` block by
   `(name, stableStringify(input))` — the backend-generated
   `tool_call_id` and Claude's `tool_use.id` aren't linked on the wire,
   so equality matching is how we pair them — and renders the
   registered component inline with the assistant bubble.
9. User interacts. Component calls `resolve(value)`.
10. `resolveClientToolCall(sessionId, tool_call_id, content)` sends a
    `tool_result_from_ui` message back up the WebSocket.
11. The backend's WebSocket handler pops the oneshot sender for
    `tool_call_id` and fires the value.
12. The HTTP request that's been awaiting on that oneshot wakes up and
    returns the value to the bridge, which returns it to Claude.

If the user never acts within the timeout, step 11 never fires; the
oneshot's awaiting end times out and returns an error to Claude.

## Conventions

- **Naming.** Tool names are snake_case, no prefix. The bridge adds the
  `mcp__template-tools__` prefix when exposing them; your frontend
  registry and backend registry use the bare name.
- **Descriptions matter.** Claude decides whether to call a tool based
  on the description. Write it like a prompt — say *when* to use the
  tool, not just *what* it does.
- **Schemas are contracts.** If you change a schema, update both the
  backend declaration and the TypeScript types on the component's
  `input` prop. The template doesn't force these into sync; a mismatch
  just means Claude sends a shape your component doesn't expect.
- **Keep client tools synchronous-ish.** The UI should resolve within a
  minute or two. Anything longer is a smell — consider a server tool
  that returns a task id the frontend can poll.
- **Server tools should be idempotent.** The same tool can be called
  twice in the same turn if Claude retries. For non-idempotent actions
  (charging a card), guard with your own dedupe.

## Debugging

- **Server logs.** The backend dumps every stream-json line it
  forwards and prints Claude's stderr prefixed with `[CLAUDE STDERR]`
  so you can see MCP-level errors without grepping through the UI.
- **Bridge visibility.** The `tool-bridge` binary is stdio-only; its
  errors surface via Claude's stderr, captured by the main server and
  forwarded as `claude-error:<sessionId>` events. The UI error bar
  accumulates every error line for a turn so you see the root cause,
  not just the final "exit code 1".
- **"No client-tool component registered for X"** appears in the UI
  when the backend declares a client tool the frontend hasn't
  registered. Usually a rename or a missed `registerClientTool` call.
- **Timeouts.** If client tools keep timing out, check
  `APP_CLIENT_TOOL_TIMEOUT_SECS` and whether your UI is ever
  unmounted (remount = lost WebSocket = lost oneshot). When a timeout
  fires, `<ChatView>` drops the stale pending call automatically once
  the error `tool_result` arrives, so the live picker doesn't linger.
- **"New chat" hygiene.** `session.reset()` cancels the old backend
  session, closes its WebSocket, and generates a fresh UUID — required
  so stream events from the retired session can't leak into the new
  one. Forks implementing their own reset should do the same.
