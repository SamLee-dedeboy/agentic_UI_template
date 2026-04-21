# Claude-UI template

A starter template for building **customer-facing, Claude-powered vertical
apps**. Fork this repo, register a handful of domain tools, swap in your
UI components, and you have a chat app where Claude calls your tools and
your React components render the results.

The scenario: a flight-ticketing company wants an agentic booking flow —
users type "I want to fly to Tokyo next Friday", Claude searches for
flights via your backend API, and your `<FlightResults>` component
renders the options. User clicks "Book option 2", Claude reserves the
seat, collects payment, sends confirmation. That's the shape this
template scaffolds — and the template ships a working end-to-end demo
of exactly that flow.

![Weather and flight-search tools rendering inline in chat](docs/Screenshot%202026-04-20%20at%2021.22.53.png)

> Derived from [`getAsterisk/opcode`](https://github.com/getAsterisk/opcode).
> Rewritten as a web-only, customer-facing agentic-app scaffold on top of
> Claude Code 2.x.

## What you get

- **Rust backend** (Axum) that spawns the Claude Code CLI as a subprocess
  per conversation turn, streams `--output-format stream-json` back to
  the browser over WebSocket, and persists every message to SQLite keyed
  by a signed guest-session cookie.
- **Tool bridge**: a companion Rust binary (`tool-bridge`) that speaks
  the MCP protocol. Forks register domain tools in one place; the bridge
  exposes them to Claude and routes invocations back to either a Rust
  handler (server tools) or a React component (client tools).
- **Three working example tools** that demo the whole stack end-to-end:
  - `get_weather` — server tool, calls the free [Open-Meteo](https://open-meteo.com) API (no key).
  - `show_choice` — client tool, renders multiple-choice buttons.
  - `search_flights` + `show_flight_options` — paired server → client tool
    that generates deterministic flight data per route and renders a
    flight picker.
- **React 19 + Vite** frontend with generative-UI chat, live markdown
  rendering (tables, code, lists via `remark-gfm`), a "Claude is
  thinking" bouncing-dots loader, and a light/dark theme toggle.
- **Tool-result renderers** — each example tool has a custom result
  component (`WeatherResultCard`, `ChoiceResultCard`,
  `FlightPickResultCard`) so results look like domain UI, not raw JSON.
  Forks register their own via `registerToolResult(name, Component)`.
- **Guest-session cookies** with HMAC-signed values, per-cookie rate
  limits (messages-per-minute and concurrent-conversations), and
  persistent conversation history. No user accounts required to start;
  forks swap in real auth at the `cookies.rs` seam.
- **Safe defaults**: Claude spawns with `--tools ""` (no filesystem, no
  bash) and `--dangerously-skip-permissions` off. Customer apps don't
  expose host filesystem access; only the tools you register.

## The core abstraction: tools as UI

A **tool** has a name, an input schema, and a handler. The handler is
either server-side Rust (hits a DB, calls an API, charges a card) or
client-side React (renders a component, waits for user input, returns
the user's choice as the result). Claude sees both kinds uniformly via
a single MCP bridge the template ships.

```
┌──────────────┐   stream-json     ┌─────────────────┐   MCP stdio   ┌──────────────┐
│  Browser     │◄────────────────  │  Axum + tool    │─────────────▶ │  claude CLI  │
│  ChatView    │                   │  registry       │               │  subprocess  │
│  React tool  │   tool_call_for_ui│                 │               │              │
│  registry    │◄──── WebSocket ──▶│  tool-bridge    │◄─ tools/call ─│              │
│              │   tool_result_    │  child (spawned │               │              │
│              │   from_ui         │  alongside)     │               │              │
└──────────────┘                   └─────────────────┘               └──────────────┘
```

A user prompt flows through Claude → Claude decides to call a tool →
the bridge forwards to the main server → the server either runs Rust
or round-trips through the WebSocket for UI rendering → the result
flows back through the bridge → Claude responds in natural language.

**Server tool example** (`backend/src/main.rs` + `backend/src/examples/weather.rs`):

```rust
// main.rs
b.server_tool(
    "get_weather",
    "Return the current weather for a city. …",
    json!({ "type": "object", "properties": { "location": {"type":"string"} }, "required": ["location"] }),
    |input| async move { examples::weather::fetch(input).await },
);

// examples/weather.rs — real handler, ~100 lines, calls Open-Meteo
pub async fn fetch(input: Value) -> Result<Value> { /* geocode + forecast */ }
```

**Client tool example** (three pieces):

1. Declare on the backend so Claude knows the tool exists:
   ```rust
   b.client_tool("show_choice", "Ask the user to pick one.", json!({ ... }));
   ```
2. Register the React component that renders it:
   ```tsx
   // src/main.tsx
   import { registerClientTool } from "@/core/tools/registry";
   import { ShowChoice } from "@/core/tools/builtins/ShowChoice";
   registerClientTool("show_choice", ShowChoice);
   ```
3. Optionally, register a custom result renderer so Claude's response
   shows a domain card instead of raw JSON:
   ```tsx
   import { registerToolResult } from "@/core/tools/registry";
   import { ChoiceResultCard } from "@/core/tools/builtins/ChoiceResultCard";
   registerToolResult("show_choice", ChoiceResultCard);
   ```

The component receives `input` and a `resolve(result)` callback. When
the user clicks a button, the component invokes `resolve(...)`; the
template ships the result back to Claude as the `tool_result`. Claude
uses it on the next turn.

See [docs/tools.md](docs/tools.md) for the full guide.

## Prerequisites

- **[Claude Code CLI](https://claude.ai/code)** on the server host (must be
  on `PATH`, or set `APP_CLAUDE_BINARY=/abs/path/to/claude`).
- **[Bun](https://bun.sh)** 1.3+ and **Rust** 1.70+ with cargo.

## Running it

```bash
bun install
bun run build           # produces dist/
cd backend
cargo build --bins      # builds claude-ui-app + tool-bridge (debug; add --release for prod)
cargo run --bin claude-ui-app   # http://127.0.0.1:8080
```

Two-process dev loop with HMR:

```bash
# Terminal 1 — backend on :8080, auto-rebuilds on .rs save
cargo install cargo-watch          # optional, one-time
cd backend && cargo watch -x "run --bin claude-ui-app"

# Terminal 2 — frontend on :1420 with HMR, proxies /api + /ws to :8080
bun run dev
```

Open http://localhost:1420. The empty-state panel explains the template
and offers three suggestion prompts — one per example tool.

> **New here?** Read [docs/README.md](docs/README.md) for a step-by-step
> walkthrough of the template using the three shipped example tools.

> **Important when editing Rust code.** The running backend keeps its
> binary in memory — rebuilding writes a new binary but doesn't hot-swap.
> After any backend change: stop the process (Ctrl+C), rebuild, restart,
> and click "New chat" in the UI so Claude's cached tool results from
> the stale context don't replay.

## Mental model (if you're coming from Node)

| You're used to                 | Here, it's                                              |
| ------------------------------ | ------------------------------------------------------- |
| Express / Fastify              | **Axum** (Rust). Router, handlers, state                |
| `fetch(...)` or axios          | `apiCall(command, params)` from `@/core/apiAdapter`     |
| `nodemon`                      | `cargo watch -x "run --bin claude-ui-app"`              |
| `vite dev` with HMR            | Same. `bun run dev` on `:1420`, proxies `/api` + `/ws`  |
| Session cookies via `express-session` | `backend/src/core/cookies.rs` (HMAC-signed)       |
| Vercel AI SDK `streamUI`       | The tool registry + `tool_call_for_ui` WebSocket dance  |

## Configuration

| Env var                           | Default | Effect                                                                                       |
| --------------------------------- | ------- | -------------------------------------------------------------------------------------------- |
| `APP_SESSION_KEY`                 | random  | HMAC key for the `session=` cookie. **Set this in production** or sessions reset on restart. |
| `APP_DB_PATH`                     | `~/.claude-ui-app/app.db` | SQLite file for conversations + messages.                        |
| `APP_COOKIE_SECURE`               | `0`     | Set `1` when fronting with TLS so the cookie gets `Secure`.                                  |
| `APP_MAX_MSGS_PER_MIN`            | `30`    | Per-cookie message rate.                                                                     |
| `APP_MAX_CONCURRENT_CONVS`        | `5`     | Per-cookie concurrent-conversation cap.                                                      |
| `APP_CLIENT_TOOL_TIMEOUT_SECS`    | `120`   | How long the backend waits for a UI tool result before failing the call.                     |
| `APP_CLAUDE_BINARY`               | _unset_ | Explicit `claude` binary path.                                                               |
| `APP_TOOL_BRIDGE_PATH`            | _derived_ | Override the `tool-bridge` binary location.                                                |
| `APP_ALLOW_SKIP_PERMISSIONS`      | `0`     | Set `1` for internal-dev-tool forks that want Claude's full built-in toolset.                |

CLI flags on the main binary:

```
claude-ui-app --host 127.0.0.1 --port 8080
```

## Project layout

```
src/                                       # Frontend (React 19 + Vite)
  App.tsx                                  # the seam you rewrite — defaults to <ChatView>
  main.tsx                                 # registers client tools + tool-result renderers
  core/
    apiAdapter.ts                          # REST + WebSocket + tool_result_from_ui + closeSession
    hooks/
      useClaudeSession.ts                  # stream-json → state, pending-tool queue, reset
      useTheme.ts                          # light/dark toggle, localStorage-backed
    components/
      ChatView.tsx                         # default customer-facing chat
      SessionRunner.tsx                    # lower-level primitive, no bubbles
      MessageList.tsx, PromptInput.tsx     # primitives
      PendingToolCalls.tsx                 # dock-style tool-call renderer
      Markdown.tsx                         # react-markdown + GFM wrapper
      ThinkingBubble.tsx                   # bouncing-dots "Claude is thinking" loader
      ThemeToggle.tsx                      # sun/moon icon button
    tools/
      registry.ts                          # clientToolRegistry + toolResultRegistry + helpers
      builtins/                            # ShowChoice, FlightResults,
                                           #   WeatherResultCard, ChoiceResultCard,
                                           #   FlightPickResultCard
  components/ui/                           # shadcn/ui primitives (Radix)
  lib/utils.ts

backend/                                   # Rust (Axum) + MCP bridge
  src/
    main.rs                                # builds the ToolRegistry, starts the server
    web_server.rs                          # HTTP routes, WebSocket, Claude spawn, dispatch
    examples/                              # reference tool implementations
      weather.rs                           #   live Open-Meteo call
      flights.rs                           #   deterministic procedural generator
    core/
      tools.rs                             # ToolRegistry, server/client tool runtimes
      stream.rs                            # typed stream-json decoder
      cookies.rs                           # HMAC-signed guest-session cookies
      conversations.rs                     # SQLite-backed message persistence
      ratelimit.rs                         # per-cookie budgets
    bin/tool_bridge.rs                     # MCP stdio shim spawned alongside Claude
    commands/                              # plain-Rust helpers used by web_server
    claude_binary.rs                       # binary discovery (nvm/homebrew/npm/PATH)
    process/registry.rs                    # in-memory process tracking

docs/
  README.md                                # hands-on walkthrough, start here
  tools.md                                 # deep dive on tool registration + plumbing
```

## Known gaps / next steps

- **No history replay on mount.** The backend persists everything; the
  frontend doesn't yet load it. Straightforward addition: call
  `/api/conversations/:id/messages` in `useClaudeSession`'s mount
  effect. Left as an exercise because the right UX (auto-resume vs.
  conversation picker) is fork-specific.
- **Shared-secret auth moved out of the default path.** Forks that need
  real OAuth / SSO should replace `cookies.rs` rather than extend it.
- **No horizontal scaling.** Rate limits and pending-tool-call maps are
  in-process. Multi-instance deployments need Redis or similar.
- **Tool schemas are hand-written JSON Schema.** Forks that want
  type-safety can layer `schemars` on top; we kept the core template
  dep-light.
- **Pending ↔ tool_use correlation is heuristic.** The frontend matches
  a pending client-tool call to its `tool_use` block by `(name, input)`
  equality because backend-generated `tool_call_id` and Claude's
  `tool_use.id` aren't linked on the wire. Breaks only if Claude emits
  two identical calls in one turn — add a tool_use_id to the dispatch
  payload if you need that.

## License

AGPL-3.0 (inherited from opcode).
