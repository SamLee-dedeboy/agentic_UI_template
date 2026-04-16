//! `tool-bridge` — MCP stdio shim spawned by the main app server alongside
//! Claude.
//!
//! Claude sees this bridge as a regular MCP server. When Claude calls a
//! tool, the bridge forwards the call to the main app server over an
//! authenticated loopback HTTP channel and streams the result back.
//!
//! Wire protocol: newline-delimited JSON-RPC 2.0 on stdin/stdout. We
//! implement just enough of the MCP surface for tool discovery and
//! invocation:
//!
//! - `initialize` → server info + `tools` capability
//! - `tools/list`  → tool manifest from the config
//! - `tools/call`  → POST to `upstream_url/__tools/dispatch`
//!
//! The MCP specification requires responses to match the request id
//! exactly. Notifications (no `id`) are answered with silence.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Deserialize)]
struct Config {
    upstream_url: String,
    secret: String,
    tools: Vec<BridgeTool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BridgeTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cfg_path = std::env::var("APP_TOOL_BRIDGE_CONFIG")
        .context("APP_TOOL_BRIDGE_CONFIG env var is required")?;
    let cfg_bytes = std::fs::read(PathBuf::from(&cfg_path))
        .with_context(|| format!("reading bridge config {cfg_path}"))?;
    let cfg: Config =
        serde_json::from_slice(&cfg_bytes).context("parsing bridge config JSON")?;

    // Short-timeout client — dispatch is local (loopback), so 30s is
    // generous; it exists mainly to prevent a wedged upstream from
    // keeping Claude hanging forever.
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_line(&line, &cfg, &http).await;
        if let Some(resp) = response {
            let out = serde_json::to_string(&resp)? + "\n";
            stdout.write_all(out.as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

/// Returns `Some(response)` for a request, `None` for a notification.
async fn handle_line(line: &str, cfg: &Config, http: &reqwest::Client) -> Option<Value> {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(err_response(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            ));
        }
    };

    let id = req.get("id").cloned();
    let is_notification = id.is_none();
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    if is_notification {
        // MCP clients send `notifications/initialized` after init. We
        // acknowledge silently. Unknown notifications are also ignored
        // per JSON-RPC 2.0.
        return None;
    }

    let id = id.unwrap_or(Value::Null);
    let result: Result<Value> = match method {
        "initialize" => Ok(initialize_result()),
        "tools/list" => Ok(tools_list_result(cfg)),
        "tools/call" => tools_call(cfg, http, params).await,
        other => Err(anyhow!("method not found: {other}")),
    };

    Some(match result {
        Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
        Err(e) => err_response(id, -32603, &e.to_string()),
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "claude-ui-app template tools",
            "version": env!("CARGO_PKG_VERSION"),
        }
    })
}

fn tools_list_result(cfg: &Config) -> Value {
    let tools: Vec<Value> = cfg
        .tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    json!({ "tools": tools })
}

async fn tools_call(cfg: &Config, http: &reqwest::Client, params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("tools/call missing 'name'"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));

    let body = json!({"name": name, "input": arguments});
    let resp = http
        .post(&cfg.upstream_url)
        .header("X-Tool-Bridge-Secret", &cfg.secret)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("dispatching {name} to upstream"))?;

    if !resp.status().is_success() {
        return Err(anyhow!(
            "upstream returned {} when dispatching {name}",
            resp.status()
        ));
    }

    let payload: Value = resp.json().await.context("parsing upstream JSON")?;
    // Upstream responds as { success, data?, error? } — flatten to an MCP
    // tool-result shape. Claude's CLI expects `content` as an array of
    // content blocks; the simplest portable shape is a single text block
    // with the JSON stringified.
    let (is_error, payload_str) = if payload.get("success").and_then(|v| v.as_bool()) == Some(true)
    {
        (
            false,
            serde_json::to_string(&payload.get("data").cloned().unwrap_or(Value::Null))
                .unwrap_or_else(|_| "null".into()),
        )
    } else {
        (
            true,
            payload
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown upstream error")
                .to_string(),
        )
    };

    Ok(json!({
        "content": [{"type": "text", "text": payload_str}],
        "isError": is_error,
    }))
}

fn err_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message},
    })
}
