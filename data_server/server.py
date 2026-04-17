"""MCP stdio server: exposes the agentic-viz data tools to Claude.

Spawned by the Rust backend with `APP_PYTHON_BINARY data_server/server.py`
and `DATA_SERVER_CONFIG=<tempfile>` in the env. The config tempfile
holds the path, format, and filename of the dataset bound to the
current chat session. We load it into a pandas DataFrame at startup,
register the frame with DuckDB as a SQL table named `data`, and then
serve three tools over MCP:

  - describe_dataset()          schema + 5 sample rows
  - query_dataset(sql)          read-only SELECT against `data`, 500-row cap
  - create_chart(sql, mark, x, y, color?, title?)
                                run the SQL, build a Vega-Lite spec with
                                Altair, return {vega_lite_spec, summary}

Every Claude turn re-spawns us, so we re-read the CSV each time. That's
fine for the <= 25 MB files the template caps at; optimization is
out of scope for the MVP.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import sys
import traceback
from pathlib import Path
from typing import Any

import altair as alt
import duckdb
import pandas as pd
import mcp.types as types
from mcp.server import Server
from mcp.server.stdio import stdio_server


def _load_config() -> dict[str, Any]:
    cfg_path = os.environ.get("DATA_SERVER_CONFIG")
    if not cfg_path:
        print("DATA_SERVER_CONFIG env var is required", file=sys.stderr)
        sys.exit(2)
    try:
        with open(cfg_path, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception as e:
        print(f"failed to read DATA_SERVER_CONFIG {cfg_path}: {e}", file=sys.stderr)
        sys.exit(2)


def _load_dataframe(path: Path, fmt: str) -> pd.DataFrame:
    if fmt == "csv":
        return pd.read_csv(path)
    if fmt == "json":
        return pd.read_json(path)
    raise ValueError(f"unsupported format '{fmt}' (expected csv or json)")


CFG = _load_config()
DATASET_PATH = Path(CFG["dataset_path"])
DATASET_FORMAT = CFG.get("format", "csv")
FILENAME = CFG.get("filename", DATASET_PATH.name)

try:
    DF: pd.DataFrame = _load_dataframe(DATASET_PATH, DATASET_FORMAT)
except Exception as e:
    print(f"failed to load dataset {DATASET_PATH}: {e}", file=sys.stderr)
    sys.exit(2)

CON = duckdb.connect(":memory:")
CON.register("data", DF)


# ---------------------------------------------------------------------------
# Tool surface.
# ---------------------------------------------------------------------------

TOOL_DESCRIBE = "describe_dataset"
TOOL_QUERY = "query_dataset"
TOOL_CREATE_CHART = "create_chart"

VEGA_TYPE_ENUM = ["nominal", "ordinal", "quantitative", "temporal"]
CHART_MARKS = ["line", "bar", "area", "point", "tick"]

TOOLS: list[types.Tool] = [
    types.Tool(
        name=TOOL_DESCRIBE,
        description=(
            "Return the uploaded dataset's schema (columns + dtypes + null counts), "
            "row count, and 5 sample rows. Call this early when you need a refresher "
            "on what columns exist or how values look."
        ),
        inputSchema={
            "type": "object",
            "properties": {},
            "additionalProperties": False,
        },
    ),
    types.Tool(
        name=TOOL_QUERY,
        description=(
            "Run a read-only SQL query against the uploaded dataset. The table is "
            "named 'data'. Only SELECT/WITH statements are allowed. Results are "
            "capped at 500 rows (aggregate in SQL if you need more). Use this for "
            "numeric answers that don't need a chart."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "sql": {
                    "type": "string",
                    "description": "A single SELECT or WITH query against table 'data'.",
                },
            },
            "required": ["sql"],
            "additionalProperties": False,
        },
    ),
    types.Tool(
        name=TOOL_CREATE_CHART,
        description=(
            "Render a Vega-Lite chart from a SQL query over the dataset. The tool "
            "runs the SQL, then builds a chart using the declared mark and "
            "encodings. Returns {vega_lite_spec, summary}; the UI renders the spec "
            "inline in the chat bubble. Prefer aggregated queries — charts with >300 "
            "points get noisy."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "sql": {
                    "type": "string",
                    "description": "SELECT/WITH query against table 'data'. Its rows feed the chart.",
                },
                "mark": {
                    "type": "string",
                    "enum": CHART_MARKS,
                    "description": "Vega-Lite mark type.",
                },
                "x": {
                    "type": "object",
                    "properties": {
                        "field": {"type": "string"},
                        "type": {"type": "string", "enum": VEGA_TYPE_ENUM},
                    },
                    "required": ["field", "type"],
                    "additionalProperties": False,
                },
                "y": {
                    "type": "object",
                    "properties": {
                        "field": {"type": "string"},
                        "type": {"type": "string", "enum": VEGA_TYPE_ENUM},
                    },
                    "required": ["field", "type"],
                    "additionalProperties": False,
                },
                "color": {
                    "type": "object",
                    "properties": {
                        "field": {"type": "string"},
                        "type": {"type": "string", "enum": VEGA_TYPE_ENUM},
                    },
                    "required": ["field", "type"],
                    "additionalProperties": False,
                },
                "title": {"type": "string"},
            },
            "required": ["sql", "mark", "x", "y"],
            "additionalProperties": False,
        },
    ),
]


# ---------------------------------------------------------------------------
# SQL safety.
# ---------------------------------------------------------------------------

_FORBIDDEN_WORDS = [
    "INSERT",
    "UPDATE",
    "DELETE",
    "DROP",
    "ALTER",
    "CREATE",
    "ATTACH",
    "COPY",
    "PRAGMA",
    "EXPORT",
    "IMPORT",
    "CALL",
    "EXECUTE",
]


def _guard_sql(sql: str) -> None:
    stripped = sql.strip()
    if not stripped:
        raise ValueError("empty SQL")
    upper = stripped.upper()
    if not (upper.startswith("SELECT") or upper.startswith("WITH")):
        raise ValueError("only SELECT or WITH queries are allowed")
    # Split ~naively on non-identifier chars to catch e.g. `;DROP TABLE data`.
    tokens = set(re.split(r"[^A-Za-z_]+", upper))
    for word in _FORBIDDEN_WORDS:
        if word in tokens:
            raise ValueError(f"'{word}' statements are not allowed")


# ---------------------------------------------------------------------------
# Handlers.
# ---------------------------------------------------------------------------


def _records(df: pd.DataFrame) -> list[dict[str, Any]]:
    """JSON-safe records from a DataFrame (ISO date strings, no NaT/NaN)."""
    return json.loads(df.to_json(orient="records", date_format="iso"))


def _describe_dataset() -> dict[str, Any]:
    columns = []
    for name in DF.columns:
        series = DF[name]
        columns.append(
            {
                "name": str(name),
                "dtype": str(series.dtype),
                "null_count": int(series.isna().sum()),
                "unique_count_approx": int(series.nunique(dropna=True)),
            }
        )
    return {
        "filename": FILENAME,
        "row_count": int(len(DF)),
        "columns": columns,
        "sample_rows": _records(DF.head(5)),
    }


def _query_dataset(sql: str) -> dict[str, Any]:
    _guard_sql(sql)
    result = CON.sql(sql).df()
    truncated = False
    if len(result) > 500:
        result = result.head(500)
        truncated = True
    return {
        "row_count": int(len(result)),
        "rows": _records(result),
        "truncated": truncated,
    }


def _create_chart(
    sql: str,
    mark: str,
    x: dict[str, str],
    y: dict[str, str],
    color: dict[str, str] | None = None,
    title: str | None = None,
) -> dict[str, Any]:
    _guard_sql(sql)
    if mark not in CHART_MARKS:
        raise ValueError(f"unsupported mark '{mark}'. Use one of {CHART_MARKS}.")
    result = CON.sql(sql).df()
    if result.empty:
        raise ValueError("query returned 0 rows; nothing to chart")

    # Build an Altair chart, then emit a plain Vega-Lite spec.
    chart_ctor = getattr(alt.Chart(result), f"mark_{mark}")
    chart = chart_ctor(tooltip=True)
    encodings = {
        "x": alt.X(field=x["field"], type=x["type"]),
        "y": alt.Y(field=y["field"], type=y["type"]),
    }
    if color:
        encodings["color"] = alt.Color(
            field=color["field"], type=color.get("type", "nominal")
        )
    chart = chart.encode(**encodings).properties(width="container", height=320)
    if title:
        chart = chart.properties(title=title)

    spec = chart.to_dict()

    # Altair sometimes emits the data via a name reference rather than
    # inline values; ensure we always ship inline values so the UI can
    # render without a second round-trip.
    if not isinstance(spec.get("data"), dict) or "values" not in spec["data"]:
        spec["data"] = {"values": _records(result)}

    y_label = y.get("field", "value")
    x_label = x.get("field", "index")
    summary = f"{mark} chart of {y_label} vs {x_label}"
    if title:
        summary = f"{title}: {summary}"

    return {
        "vega_lite_spec": spec,
        "summary": summary,
        "row_count": int(len(result)),
    }


# ---------------------------------------------------------------------------
# MCP wiring.
# ---------------------------------------------------------------------------

server = Server("viz-tools")


@server.list_tools()
async def _list_tools() -> list[types.Tool]:
    return TOOLS


@server.call_tool()
async def _call_tool(name: str, arguments: dict[str, Any] | None) -> list[types.TextContent]:
    args = arguments or {}
    try:
        if name == TOOL_DESCRIBE:
            result = _describe_dataset()
        elif name == TOOL_QUERY:
            result = _query_dataset(args["sql"])
        elif name == TOOL_CREATE_CHART:
            result = _create_chart(
                sql=args["sql"],
                mark=args["mark"],
                x=args["x"],
                y=args["y"],
                color=args.get("color"),
                title=args.get("title"),
            )
        else:
            raise ValueError(f"unknown tool '{name}'")
        payload = json.dumps(result, default=str)
        return [types.TextContent(type="text", text=payload)]
    except Exception as e:
        traceback.print_exc(file=sys.stderr)
        err_payload = json.dumps({"error": str(e), "tool": name}, default=str)
        return [types.TextContent(type="text", text=err_payload)]


async def _main() -> None:
    async with stdio_server() as (read_stream, write_stream):
        await server.run(
            read_stream,
            write_stream,
            server.create_initialization_options(),
        )


if __name__ == "__main__":
    asyncio.run(_main())
