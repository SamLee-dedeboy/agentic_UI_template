# data_server — Python MCP sidecar for agentic viz

This process is spawned by the Rust backend once per Claude turn
(`claude --mcp-config` points at a config that launches us). It exposes
three tools Claude can call:

| Tool              | Purpose                                                         |
| ----------------- | --------------------------------------------------------------- |
| `describe_dataset`| Schema, dtypes, null counts, 5 sample rows.                     |
| `query_dataset`   | Read-only SQL (SELECT/WITH) against table `data`; 500-row cap.  |
| `create_chart`    | Run SQL + build a Vega-Lite spec via Altair.                    |

## Contract

- Reads the `DATA_SERVER_CONFIG` env var (path to a JSON file with
  `{dataset_path, format, filename}`) written by the Rust backend.
- Loads the dataset into a pandas DataFrame, registers it with DuckDB
  as the SQL table `data`.
- Serves MCP JSON-RPC over stdio until Claude closes the pipe.

Every Claude turn re-spawns this process, so the CSV/JSON is re-read
each time. For the 25 MB cap the backend enforces, the cold start is
typically under a second.

## Prerequisites

**Python ≥ 3.10** is required — the `mcp` package ships no wheels for
older interpreters, and `pip install` will fail with `No matching
distribution found for mcp` on 3.9 or earlier.

Check first:

```bash
python3 --version   # expect 3.10 or newer
```

If you're on an older version, install a newer Python and use it
explicitly in the venv command below:

```bash
# macOS (Homebrew)
brew install python@3.12

# Debian / Ubuntu
sudo apt install python3.12 python3.12-venv

# Windows — grab an installer from https://www.python.org/downloads/
```

## Install

Use a virtualenv so pandas / duckdb / altair don't pollute your system
Python. From the repo root:

```bash
# Create + activate (one-time). Swap `python3` for `python3.12` etc. if
# your default python3 is older than 3.10.
python3 -m venv data_server/.venv
source data_server/.venv/bin/activate     # fish: `source .venv/bin/activate.fish`
                                          # Windows: `data_server\.venv\Scripts\activate`

# Upgrade pip — older pip sometimes can't resolve `mcp`.
pip install --upgrade pip

# Install deps into the venv.
pip install -r data_server/requirements.txt
```

Sanity-check:

```bash
python -c "import mcp, pandas, duckdb, altair; print('ok')"
```

Verify you're on the venv interpreter:

```bash
which python        # .../data_server/.venv/bin/python
```

### Tell the Rust backend about it

The backend launches Python for you on each Claude turn. Point it at
the venv's interpreter so it picks up these dependencies rather than
system Python:

```bash
export APP_PYTHON_BINARY="$(pwd)/data_server/.venv/bin/python"
cargo run --bin claude-ui-app           # from backend/
```

Persist it in your shell rc, or prepend to the command each time. On
Windows use `data_server\.venv\Scripts\python.exe`.

Alternative: if you just `source data_server/.venv/bin/activate` in the
same shell before `cargo run`, `python3` on PATH will already point at
the venv and `APP_PYTHON_BINARY` isn't needed.

## Standalone smoke test

With the venv activated:

```bash
printf '%s' '{"dataset_path":"../some.csv","format":"csv","filename":"some.csv"}' > /tmp/ds.json
DATA_SERVER_CONFIG=/tmp/ds.json python data_server/server.py
```

The process will block waiting for JSON-RPC input on stdin. Ctrl-C to
exit. In normal operation the backend handles the stdio pipe for you.

## Security

- Only `SELECT` and `WITH` statements are permitted; a regex token check
  refuses `INSERT`, `UPDATE`, `DELETE`, `DROP`, `ALTER`, `CREATE`,
  `ATTACH`, `COPY`, `PRAGMA`, and a handful of other write-side verbs.
- DuckDB runs in-memory with no filesystem handles, so even a guard
  bypass can't touch the host.
- The dataset path is trusted input from the Rust backend (which
  validated the cookie on upload).
