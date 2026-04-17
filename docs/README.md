# Getting started

A hands-on walkthrough of the agentic-viz template. Follow this once
top to bottom and you'll understand how to extend the Python MCP data
tools, the React chart renderer, and the dataset lifecycle.

- High-level pitch, configuration reference, architecture diagram? →
  [main README](../README.md).
- Adding Rust-side tools alongside the Python MCP sidecar? →
  [docs/tools.md](./tools.md).
- Python sidecar internals (install, standalone testing, SQL safety)?
  → [data_server/README.md](../data_server/README.md).

## 1. Run the template

Prerequisites: [Bun](https://bun.sh) 1.3+, Rust 1.70+, the
[Claude Code CLI](https://claude.ai/code) on `PATH`, **Python 3.10+**.

```bash
# JS + Rust deps
bun install

# Python MCP sidecar deps (in a venv)
python3 -m venv data_server/.venv
source data_server/.venv/bin/activate
pip install --upgrade pip
pip install -r data_server/requirements.txt

# Build + run
bun run build
cd backend
cargo build --bins
cargo run --bin claude-ui-app          # http://127.0.0.1:8080
```

> **Point the backend at the venv's Python.** The Rust backend spawns
> Python on each Claude turn. Either activate the venv in the same
> shell before `cargo run`, or export
> `APP_PYTHON_BINARY="$(pwd)/../data_server/.venv/bin/python"`. If you
> forget this, the default `python3` on PATH doesn't have `mcp` and
> Claude quietly sees no viz tools.

Open http://127.0.0.1:8080. The empty-state panel nudges you to upload
a dataset.

## 2. Upload a dataset

Click **Dataset** in the composer and pick a CSV or a JSON array-of-
objects (≤ 25 MB). A fixture lives at
[data_server/tests/fixtures/cars.csv](../data_server/tests/fixtures/cars.csv)
— 15 rows with `year, price, mileage, make` columns.

What happens:

1. Browser POSTs the file to `/api/datasets/upload` (multipart).
2. Backend writes it to `$TMPDIR/claude-ui-ds-<uuid>.csv` and parses
   column names + 5 sample rows + row count with the `csv` crate.
3. Backend returns `{dataset_id, filename, format, row_count, columns, sample_rows}`.
4. Frontend stashes the id and renders the `DatasetChip` above the
   composer.

Backend surface: [backend/src/web_server.rs](../backend/src/web_server.rs)
`upload_dataset()`; schema inference:
[backend/src/core/datasets.rs](../backend/src/core/datasets.rs).

Frontend surface:
[src/features/datasets/DatasetUploadButton.tsx](../src/features/datasets/DatasetUploadButton.tsx)
and [src/features/datasets/api.ts](../src/features/datasets/api.ts).

## 3. Ask a data question

With `cars.csv` uploaded, try:

> *what's the trend of car prices over time?*

What happens, end to end:

1. **Bind the dataset to the session.** Before the prompt is sent,
   `App.tsx` POSTs `/api/datasets/bind` with `{session_id, dataset_id}`
   (see [src/App.tsx](../src/App.tsx) `handleBeforeSend`).
2. **Open a WebSocket** and send the prompt
   ([src/core/apiAdapter.ts](../src/core/apiAdapter.ts)).
3. **Spawn Claude.** The backend notices the session has a bound
   dataset and calls `prepare_python_bridge` ([web_server.rs](../backend/src/web_server.rs)),
   which writes two tempfiles:
   - An MCP config pointing at `python data_server/server.py`, with the
     Python binary resolved via `APP_PYTHON_BINARY`.
   - A data-server config with `{dataset_path, format, filename}`
     passed to the Python process via `DATA_SERVER_CONFIG` env var.
   It also injects an `--append-system-prompt` summarizing the dataset
   schema so Claude knows what columns exist without having to ask.
4. **Python MCP server boots.** Reads the config, loads the CSV into a
   pandas DataFrame, registers it with DuckDB as SQL table `data`,
   announces three tools over stdio: `describe_dataset`,
   `query_dataset`, `create_chart`.
5. **Claude calls tools.** Typical flow for a trend question:
   1. `describe_dataset()` → schema + sample rows confirm column types.
   2. `query_dataset("SELECT year, AVG(price) FROM data GROUP BY year ORDER BY year")`
      → Claude inspects the aggregate.
   3. `create_chart({ sql: "SELECT year, AVG(price) AS avg_price FROM data GROUP BY year ORDER BY year", mark: "line", x: {field: "year", type: "ordinal"}, y: {field: "avg_price", type: "quantitative"}, title: "Avg price by year" })`
      → Python runs the SQL, builds an Altair chart, calls
      `.to_dict()`, returns `{vega_lite_spec, summary, row_count}`.
6. **Frontend renders.** `<ChatView>` already renders text /
   `tool_use` / `tool_result` blocks in order within one bubble, so
   interleaving is free. `create_chart`'s result hits the custom
   renderer registered in [src/main.tsx](../src/main.tsx):
   [ChartResultCard](../src/features/viz/ChartResultCard.tsx) paints
   the Vega-Lite spec with `react-vega`, sized via ResizeObserver.

## 4. Trace a `create_chart` call in detail

Everything above happens in one `tool_result` round-trip. If you want
to verify the wire:

- The backend prints `[TRACE] Command: …` on each spawn — look for the
  two `--mcp-config` flags (the Rust tool-bridge is empty by default,
  the Python viz-tools config is the one that matters) and the
  `--allowed-tools mcp__viz-tools__…,…` line.
- `data_server/server.py` prints any exception tracebacks to stderr,
  which the Rust backend forwards to the UI as `error` events (you'll
  see a red bubble above the composer).
- In the browser DevTools, inspect the `<svg>` inside the chart card:
  it should have `width="~500"` and a real `viewBox`. If it's width=0,
  reload the tab — [ChartResultCard](../src/features/viz/ChartResultCard.tsx)
  remeasures on mount via ResizeObserver but a stale first render can
  persist across hot reloads.

## 5. Extend: add a new Python data tool

Say you want a `column_stats` tool that returns mean / std / quantiles
for a numeric column. Two files to touch:

**[data_server/server.py](../data_server/server.py)** — register + implement:

```python
TOOLS.append(types.Tool(
    name="column_stats",
    description="Return mean, std, min, max, and quartiles for a numeric column.",
    inputSchema={
        "type": "object",
        "properties": {"column": {"type": "string"}},
        "required": ["column"],
        "additionalProperties": False,
    },
))

def _column_stats(column: str) -> dict:
    series = DF[column]
    desc = series.describe()
    return {"column": column, "stats": json.loads(desc.to_json())}

# In _call_tool:
elif name == "column_stats":
    result = _column_stats(args["column"])
```

**[backend/src/web_server.rs](../backend/src/web_server.rs)** — add the
tool to the `allowed_tools` list in `prepare_python_bridge` so Claude
can auto-invoke it without permission prompts:

```rust
let allowed_tools = vec![
    "mcp__viz-tools__describe_dataset".into(),
    "mcp__viz-tools__query_dataset".into(),
    "mcp__viz-tools__create_chart".into(),
    "mcp__viz-tools__column_stats".into(),   // new
];
```

Rebuild the backend, click "New chat", and Claude will pick up the new
tool from the next `tools/list`. Python edits hot-apply per turn — no
Python restart required.

Optionally, register a custom result bubble in
[src/main.tsx](../src/main.tsx):

```tsx
registerToolResult("column_stats", ColumnStatsCard);
```

Without one, the result falls back to a collapsed JSON preview in
[ChatView](../src/core/components/ChatView.tsx), which is fine for
tools Claude just reads off — no UI required.

## 6. Extend: add a Rust-side tool alongside Python

Some tools don't need Python (API calls, DB lookups, business logic).
Register them in [backend/src/main.rs](../backend/src/main.rs) via
`b.server_tool(...)` / `b.client_tool(...)` — they become available
through the Rust `tool-bridge` MCP server in addition to the Python
`viz-tools` server. See [docs/tools.md](./tools.md) for the full
server-vs-client decision matrix and the exact schema + handler shape.

## 7. Rebrand for your fork

To make this your own:

1. Update the empty-state copy in [src/App.tsx](../src/App.tsx)
   (`<EmptyState>`) so the suggestions describe your domain.
2. Swap the page title in [index.html](../index.html).
3. Edit the system prompt in `prepare_python_bridge`
   ([backend/src/web_server.rs](../backend/src/web_server.rs)) if your
   domain needs different guidance for Claude (e.g., "always include a
   95% CI", "never aggregate across regions", etc.).
4. Add fork-specific tools under `data_server/` or in `backend/src/`
   per the extension guides above.

## Where to go next

- **[Main README](../README.md)** — configuration reference, project
  layout, prerequisites.
- **[data_server/README.md](../data_server/README.md)** — Python
  sidecar install, standalone testing, SQL safety guarantees.
- **[docs/tools.md](./tools.md)** — deep reference for adding Rust
  server/client tools (still valid; the plumbing is unchanged).
- **[CLAUDE.md](../CLAUDE.md)** — architecture notes for Claude Code
  itself when working on this repo.
