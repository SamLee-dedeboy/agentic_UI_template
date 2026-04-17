# Agentic Visualization

An agentic data-visualization app: upload a CSV or JSON dataset, ask a
question in chat, and Claude replies with interleaved prose and
Vega-Lite charts rendered inline in the same bubble.

```
you:    what's the trend of car prices over time?

Claude: I'll start by looking at what columns are available…
        [describe_dataset → cars.csv: year, price, mileage, make]
        Prices have risen steadily since 2018. Here's the trend:
        ┌──────────────── Avg price by year ────────────────┐
        │   •                                                 │
        │        •                                            │
        │            •                                        │
        │                 •                                   │
        │                       •                             │
        └─────────────────────────────────────────────────────┘
        The steepest jump is between 2020 and 2022 — likely the
        pandemic-era supply crunch.
```

The template ships Rust orchestration, a Python MCP sidecar for data
tools, and a React chat UI with `react-vega`. Fork it to build any
domain-specific agentic-viz app (product analytics, finance dashboards,
ML eval explorers, etc.).

> Derived from [`getAsterisk/opcode`](https://github.com/getAsterisk/opcode).
> Pivoted from a generic tools template into a viz-focused app on top of
> Claude Code 2.x.

## Architecture at a glance

```
┌──────────────┐   stream-json      ┌─────────────────┐   MCP stdio   ┌──────────────┐
│  Browser     │◄─ WebSocket ─────  │  Axum backend   │◄────────────▶│  claude CLI  │
│  ChatView    │                    │  (Rust)         │               │  subprocess  │
│  react-vega  │   /api/datasets/*  │                 │               │              │
│              │◄── REST ─────────▶ │  dataset store  │               │              │
└──────────────┘                    │                 │    stdio      ┌──────────────┐
                                    │  spawns …       │◄─────────────▶│ data_server  │
                                    └─────────────────┘    MCP        │ (Python)     │
                                                                      │ pandas+DuckDB│
                                                                      │ +Altair      │
                                                                      └──────────────┘
```

- **Rust backend** owns HTTP/WebSocket, session cookies, dataset upload
  + binding, and the Claude subprocess.
- **Python MCP sidecar** ([data_server/](data_server/)) owns the data
  tools. Loads the bound dataset into pandas + DuckDB (as SQL table
  `data`), exposes `describe_dataset`, `query_dataset(sql)`, and
  `create_chart(sql, mark, x, y, …)`. Altair emits the Vega-Lite spec.
- **React frontend** renders the chat, uploads datasets, and paints
  `create_chart` tool results with `react-vega` inline in the bubble.
  Interleaving is free: `<ChatView>` already renders text / tool_use /
  tool_result blocks in order inside one bubble.

See [CLAUDE.md](CLAUDE.md) for the full architecture writeup.

## What you get

- **Dataset upload** with schema sniffing (CSV or JSON array-of-objects,
  up to 25 MB by default), cookie-scoped ownership, tempfile storage.
- **Three Python tools** exposed over MCP to Claude:
  `describe_dataset`, `query_dataset`, `create_chart`. Read-only SQL;
  chart specs are pure Vega-Lite JSON.
- **Inline chart rendering** via `react-vega` with ResizeObserver-based
  sizing, transparent background, and axis/legend colors that inherit
  from the app's light/dark theme.
- **Guest-session cookies** with HMAC-signed values, per-cookie rate
  limits, and persistent conversation history in SQLite.
- **Safe defaults**: Claude spawns with `--tools ""` (no filesystem, no
  bash); only the MCP tools you register are available. SQL is guarded
  to SELECT/WITH only and runs in in-memory DuckDB with no disk access.
- **Rust tool surface preserved**: the old Rust MCP `tool-bridge` and
  `ToolRegistry` are still in the tree, just unused by default. Forks
  that want to mix Rust-side tools alongside the Python ones re-enable
  them by registering tools in [backend/src/main.rs](backend/src/main.rs).

## Prerequisites

- **[Claude Code CLI](https://claude.ai/code)** on `PATH`, or
  `APP_CLAUDE_BINARY=/abs/path/to/claude`.
- **[Bun](https://bun.sh)** 1.3+ and **Rust** 1.70+ with cargo.
- **Python 3.10+** (the `mcp` package requires it).

## Running it

### 1. Install

```bash
bun install

# Python MCP server deps — see data_server/README.md for the venv walk-through.
python3 -m venv data_server/.venv
source data_server/.venv/bin/activate
pip install --upgrade pip
pip install -r data_server/requirements.txt
```

Point the backend at the venv's Python so MCP spawns find their deps:

```bash
# Either activate the venv before `cargo run`
source data_server/.venv/bin/activate

# …or set APP_PYTHON_BINARY explicitly (persistable in your shell rc)
export APP_PYTHON_BINARY="$(pwd)/data_server/.venv/bin/python"
```

### 2. One-process prod run

```bash
bun run build                          # compiles frontend into dist/
cd backend
cargo build --bins                     # builds claude-ui-app + tool-bridge
cargo run --bin claude-ui-app          # http://127.0.0.1:8080
```

### 3. Two-process dev loop (HMR)

```bash
# Terminal 1 — backend on :8080 (auto-rebuilds on Rust saves with cargo-watch)
cd backend && cargo watch -x "run --bin claude-ui-app"

# Terminal 2 — frontend on :1420 with HMR, proxies /api + /ws to :8080
bun run dev
```

Open http://localhost:1420. Click **Dataset** in the composer, upload a
CSV, and ask a data question. A good first prompt against the included
fixture ([data_server/tests/fixtures/cars.csv](data_server/tests/fixtures/cars.csv)):

> *what's the trend of car prices over time?*

> **After editing backend Rust code.** Stop the running process
> (Ctrl+C), `cargo build --bins`, restart, and click "New chat" in the
> UI. A running backend keeps its binary mmapped; Claude also caches
> tool context inside a chat so a new chat flushes that state. Python
> edits (`data_server/*.py`) take effect on the next turn automatically
> — no backend restart needed.

## Configuration

| Env var                           | Default | Effect                                                                                          |
| --------------------------------- | ------- | ----------------------------------------------------------------------------------------------- |
| `APP_SESSION_KEY`                 | random  | HMAC key for the `session=` cookie. **Set this in production** or sessions reset on restart.    |
| `APP_DB_PATH`                     | `~/.claude-ui-app/app.db` | SQLite file for conversation persistence.                           |
| `APP_COOKIE_SECURE`               | `0`     | Set `1` when fronting with TLS so the cookie gets `Secure`.                                     |
| `APP_MAX_MSGS_PER_MIN`            | `30`    | Per-cookie message-rate ceiling.                                                                |
| `APP_MAX_CONCURRENT_CONVS`        | `5`     | Per-cookie concurrent-conversation cap.                                                         |
| `APP_UPLOAD_MAX_BYTES`            | `26214400` (25 MB) | Multipart body limit for `POST /api/datasets/upload`.                                |
| `APP_CLIENT_TOOL_TIMEOUT_SECS`    | `120`   | Client-tool round-trip timeout (only matters if you add Rust client tools).                     |
| `APP_CLAUDE_BINARY`               | _unset_ | Explicit `claude` binary path.                                                                  |
| `APP_PYTHON_BINARY`               | `python3` on PATH | Python interpreter used to spawn the MCP sidecar. Point at your venv.                 |
| `APP_DATA_SERVER_SCRIPT`          | `data_server/server.py` (relative to CWD or `../`) | Override the MCP server script location.       |
| `APP_TOOL_BRIDGE_PATH`            | _derived_ | Override the `tool-bridge` binary (only matters if you re-enable Rust tools).                 |
| `APP_ALLOW_SKIP_PERMISSIONS`      | `0`     | Set `1` to let Claude use its built-in filesystem/bash tools. Don't do this in customer-facing deploys. |

CLI:

```
claude-ui-app --host 127.0.0.1 --port 8080
```

## Project layout

```
src/                                       # Frontend (React + Vite)
  App.tsx                                  # owns session + dataset state, wires ChatView slots
  main.tsx                                 # registerToolResult("create_chart", ChartResultCard)
  core/                                    # stable plumbing — keep shape-compatible
    apiAdapter.ts                          # REST + WebSocket helpers
    hooks/useClaudeSession.ts              # stream-json → state, pending-tool queue, reset
    hooks/useTheme.ts                      # light/dark toggle, localStorage-backed
    components/
      ChatView.tsx                         # bubble chat; accepts session + slot props
      PromptInput.tsx                      # composer; leftAdornment slot for upload button
      ThinkingBubble.tsx, ThemeToggle.tsx, Markdown.tsx
    tools/registry.ts                      # clientToolRegistry + toolResultRegistry + helpers
    tools/builtins/                        # empty in the default template; forks add reference components here
  features/                                # viz surface (fork-editable)
    datasets/                              # upload button + chip + REST client
    viz/ChartResultCard.tsx                # react-vega renderer for create_chart results

backend/                                   # Rust (Axum) + MCP bridge
  src/
    main.rs                                # empty ToolRegistry by default; forks add Rust tools here
    web_server.rs                          # HTTP routes, WebSocket, Claude spawn, Python + Rust bridge
    core/
      datasets.rs                          # DatasetStore, CSV/JSON schema inference
      tools.rs                             # ToolRegistry (unchanged; for forks)
      cookies.rs, conversations.rs, ratelimit.rs, stream.rs
    bin/tool_bridge.rs                     # Rust MCP stdio shim — retained for forks

data_server/                               # Python MCP sidecar
  server.py                                # describe_dataset, query_dataset, create_chart
  requirements.txt                         # mcp, pandas, duckdb, altair
  README.md                                # setup + standalone testing
  tests/fixtures/cars.csv                  # fixture for smoke tests

docs/
  README.md                                # hands-on walkthrough (start here after this file)
  tools.md                                 # deep dive on the ToolRegistry (for Rust-tool forks)

CLAUDE.md                                  # architecture notes for Claude Code itself
```

## Extending

- **Add more Python tools**: edit [data_server/server.py](data_server/server.py).
  Register in the `TOOLS` list + implement in `_call_tool`. Add a
  front-end `registerToolResult(name, Component)` in
  [src/main.tsx](src/main.tsx) if you want a custom result bubble.
- **Add Rust tools alongside Python**: register them in
  [backend/src/main.rs](backend/src/main.rs) via
  `b.server_tool(...)` / `b.client_tool(...)`. They'll be exposed
  through the Rust `tool-bridge` MCP server alongside the Python
  `viz-tools` server. See [docs/tools.md](docs/tools.md).
- **Rebrand**: swap the empty-state copy in [src/App.tsx](src/App.tsx)
  and the page title in [index.html](index.html).

## Known gaps

- **No history replay on mount.** Conversations persist to SQLite, but
  the frontend doesn't load prior messages on refresh. Add a call to
  `/api/conversations/:id/messages` in `useClaudeSession`'s mount
  effect if you need it.
- **Dataset store is in-memory.** Process restart drops uploads; per-cookie
  ownership is enforced but multi-instance deploys need external
  storage (S3, Redis, or a shared FS + locking).
- **Python process is re-spawned per turn.** Each Claude turn re-reads
  the CSV into pandas — fine for ≤25 MB, but add a long-lived daemon
  with a socket if you need warm-start on larger datasets.
- **Chart sizing is ResizeObserver-based.** `width: "container"` in
  Vega-Lite doesn't recover from an initial 0-width measurement;
  [ChartResultCard](src/features/viz/ChartResultCard.tsx) measures the
  wrapper itself and passes a numeric width into the spec.
- **SQL safety is belt-and-suspenders.** `query_dataset` refuses
  non-SELECT via token matching; DuckDB is in-memory so writes are also
  physically blocked. Don't rely on one layer alone.

## License

AGPL-3.0 (inherited from opcode).
