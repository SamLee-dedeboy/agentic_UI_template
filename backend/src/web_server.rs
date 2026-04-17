use axum::extract::ws::{Message, WebSocket};
use axum::extract::{DefaultBodyLimit, Multipart};
use axum::http::{Method, StatusCode};
use axum::{
    extract::{Path, State as AxumState, WebSocketUpgrade},
    response::{Html, IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use which;

use crate::core::datasets::{DatasetBinding, DatasetRecord, DatasetStore};


// Find Claude binary for web mode - use bundled binary first
fn find_claude_binary_web() -> Result<String, String> {
    // First try the bundled binary (same location as Tauri app uses)
    let bundled_binary = "src-tauri/binaries/claude-code-x86_64-unknown-linux-gnu";
    if std::path::Path::new(bundled_binary).exists() {
        println!(
            "[find_claude_binary_web] Using bundled binary: {}",
            bundled_binary
        );
        return Ok(bundled_binary.to_string());
    }

    // Fall back to system installation paths
    let home_path = format!(
        "{}/.local/bin/claude",
        std::env::var("HOME").unwrap_or_default()
    );
    let candidates = vec![
        "claude",
        "claude-code",
        "/usr/local/bin/claude",
        "/usr/bin/claude",
        "/opt/homebrew/bin/claude",
        &home_path,
    ];

    for candidate in candidates {
        if which::which(candidate).is_ok() {
            println!(
                "[find_claude_binary_web] Using system binary: {}",
                candidate
            );
            return Ok(candidate.to_string());
        }
    }

    Err("Claude binary not found in bundled location or system paths".to_string())
}

#[derive(Clone)]
pub struct AppState {
    // Track active WebSocket sessions for Claude execution.
    pub active_sessions:
        Arc<Mutex<std::collections::HashMap<String, tokio::sync::mpsc::Sender<String>>>>,
    // For each running Claude subprocess, a one-shot channel the cancel
    // endpoint sends on to request termination. Keyed by the session ID
    // that owns the subprocess (client-supplied where available, else the
    // WebSocket connection UUID).
    pub cancel_channels:
        Arc<Mutex<std::collections::HashMap<String, tokio::sync::mpsc::Sender<()>>>>,
    // Conversation + message persistence keyed by guest-cookie session ID.
    pub store: crate::core::conversations::ConversationStore,
    // Per-cookie message-rate and concurrent-conversation budgets.
    pub rate_limiter: crate::core::ratelimit::RateLimiter,
    // The tool registry — dispatched via /__tools/dispatch from the
    // MCP bridge that runs next to each Claude subprocess.
    pub tools: crate::core::tools::ToolRegistry,
    // Per-spawn bridge secret → owning client session ID. Populated when a
    // tool-bridge is spawned, removed when the Claude subprocess exits.
    // /__tools/dispatch looks up the secret here to authorize and to know
    // which conversation a client-tool call belongs to so we can route
    // the `tool_call_for_ui` message to the right WebSocket.
    pub active_bridge_secrets:
        Arc<Mutex<std::collections::HashMap<String, String>>>,
    // In-flight client tool calls awaiting a `tool_result_from_ui` reply
    // from the frontend. Keyed by the server-generated `tool_call_id`
    // (not the MCP tool_use_id, which we don't see at dispatch time).
    pub pending_client_tools: Arc<
        Mutex<
            std::collections::HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>,
        >,
    >,
    // The loopback URL the tool-bridge should POST to. Derived from the
    // `host:port` passed to `create_web_server`.
    pub self_url: String,
    // Uploaded datasets (keyed by cookie for ownership checks) + the
    // per-session binding that tells the Claude spawn path which dataset,
    // if any, to attach the Python MCP sidecar for.
    pub datasets: DatasetStore,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct ClaudeExtraArgs {
    /// `--permission-mode <mode>`: acceptEdits | auto | bypassPermissions |
    /// default | dontAsk | plan.
    pub permission_mode: Option<String>,
    /// `--effort <level>`: low | medium | high | max.
    pub effort: Option<String>,
    /// `--max-budget-usd <amount>`.
    pub max_budget_usd: Option<f64>,
    /// `--fallback-model <model>`.
    pub fallback_model: Option<String>,
    /// `--append-system-prompt <prompt>`.
    pub append_system_prompt: Option<String>,
    /// `--include-partial-messages` (streaming deltas).
    pub include_partial_messages: Option<bool>,
    /// `--include-hook-events`.
    pub include_hook_events: Option<bool>,
    /// `--add-dir <dir>` (repeatable).
    pub add_dir: Option<Vec<String>>,
    /// `--mcp-config <config>` (repeatable). Each entry is a path or JSON.
    pub mcp_config: Option<Vec<String>>,
    /// `--allowed-tools <tools...>`.
    pub allowed_tools: Option<Vec<String>>,
    /// `--disallowed-tools <tools...>`.
    pub disallowed_tools: Option<Vec<String>>,
    /// `--tools <spec>` — the built-in tool surface Claude gets. Use `""` to
    /// disable all built-ins (default for customer-facing apps), `"default"`
    /// for everything, or a comma/space-separated list. When `None` and
    /// `dangerously_skip_permissions` is off, the template appends
    /// `--tools ""` so Claude can't touch the filesystem unless a fork
    /// opts in.
    pub tools: Option<String>,
    /// Override the server-wide default for `--dangerously-skip-permissions`.
    /// When `None`, the default is **off** — customer-facing apps should not
    /// expose Claude's filesystem tools. Set `APP_ALLOW_SKIP_PERMISSIONS=1`
    /// (or pass `true` here per-request) only for internal dev tools.
    pub dangerously_skip_permissions: Option<bool>,
}

/// Resolve whether `--dangerously-skip-permissions` should be applied for a
/// given request. Default is **off**; the `APP_ALLOW_SKIP_PERMISSIONS` env
/// var opts the whole server in, and a per-request `dangerously_skip_permissions`
/// flag can flip either way.
fn resolve_skip_permissions(extra: &ClaudeExtraArgs) -> bool {
    if let Some(explicit) = extra.dangerously_skip_permissions {
        return explicit;
    }
    match std::env::var("APP_ALLOW_SKIP_PERMISSIONS") {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "yes"),
        Err(_) => false,
    }
}

/// Decide the default `--tools` value. If the request doesn't pin one and
/// `--dangerously-skip-permissions` isn't on, lock Claude to zero built-in
/// tools so filesystem and bash stay out of customer-facing flows. Forks
/// that want the dev experience pass `tools: Some("default")` per-request
/// or set `APP_ALLOW_SKIP_PERMISSIONS=1`.
fn resolve_default_tools(extra: &ClaudeExtraArgs) -> Option<String> {
    if extra.tools.is_some() {
        return None; // caller is explicit; append_extra_args emits it
    }
    if resolve_skip_permissions(extra) {
        return None; // dev flow: let Claude see its full builtin set
    }
    Some(String::new()) // the CLI treats --tools "" as "disable all builtins"
}

/// Append optional CLI flags derived from the request to an existing argv.
fn append_extra_args(args: &mut Vec<String>, extra: &ClaudeExtraArgs) {
    if let Some(ref m) = extra.permission_mode {
        args.push("--permission-mode".into());
        args.push(m.clone());
    }
    if let Some(ref e) = extra.effort {
        args.push("--effort".into());
        args.push(e.clone());
    }
    if let Some(b) = extra.max_budget_usd {
        args.push("--max-budget-usd".into());
        args.push(b.to_string());
    }
    if let Some(ref fb) = extra.fallback_model {
        args.push("--fallback-model".into());
        args.push(fb.clone());
    }
    if let Some(ref p) = extra.append_system_prompt {
        args.push("--append-system-prompt".into());
        args.push(p.clone());
    }
    if extra.include_partial_messages.unwrap_or(false) {
        args.push("--include-partial-messages".into());
    }
    if extra.include_hook_events.unwrap_or(false) {
        args.push("--include-hook-events".into());
    }
    if let Some(ref dirs) = extra.add_dir {
        for d in dirs {
            args.push("--add-dir".into());
            args.push(d.clone());
        }
    }
    if let Some(ref cfgs) = extra.mcp_config {
        for c in cfgs {
            args.push("--mcp-config".into());
            args.push(c.clone());
        }
    }
    if let Some(ref tools) = extra.allowed_tools {
        if !tools.is_empty() {
            args.push("--allowed-tools".into());
            args.push(tools.join(","));
        }
    }
    if let Some(ref spec) = extra.tools {
        args.push("--tools".into());
        args.push(spec.clone());
    }
    if let Some(ref tools) = extra.disallowed_tools {
        if !tools.is_empty() {
            args.push("--disallowed-tools".into());
            args.push(tools.join(","));
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ClaudeExecutionRequest {
    pub project_path: String,
    pub prompt: String,
    pub model: Option<String>,
    /// For `resume`, this is the prior conversation UUID to pick up.
    pub session_id: Option<String>,
    /// For `execute`/`continue`, a client-generated UUID to bind this new
    /// conversation to. Passed through to `claude --session-id`, and used as
    /// the routing key for stream events back to the frontend. When absent,
    /// the server falls back to the WebSocket connection UUID (legacy).
    pub client_session_id: Option<String>,
    pub command_type: String, // "execute", "continue", or "resume"
    /// Optional CLI flag surface exposed to forks.
    #[serde(default)]
    pub extra: ClaudeExtraArgs,
}

#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(error: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
        }
    }
}

/// Serve the React frontend
async fn serve_frontend() -> Html<&'static str> {
    Html(include_str!("../../dist/index.html"))
}

/// API endpoint to get projects (equivalent to Tauri command)
/// List conversations owned by the requesting guest cookie (newest first).
async fn list_conversations(
    AxumState(state): AxumState<AppState>,
    axum::Extension(guest): axum::Extension<crate::core::cookies::GuestSession>,
) -> Json<ApiResponse<Vec<crate::core::conversations::ConversationRow>>> {
    match state.store.list_for_cookie(&guest.id).await {
        Ok(rows) => Json(ApiResponse::success(rows)),
        Err(e) => Json(ApiResponse::error(e.to_string())),
    }
}

/// Replay stored messages for `:conversation_id` if the guest cookie owns
/// it. Returns an empty list (not an error) if ownership doesn't match, so
/// we don't leak existence across cookies.
async fn load_conversation_messages(
    AxumState(state): AxumState<AppState>,
    axum::Extension(guest): axum::Extension<crate::core::cookies::GuestSession>,
    Path(conversation_id): Path<String>,
) -> Json<ApiResponse<Vec<crate::core::conversations::MessageRow>>> {
    match state.store.load_messages(&conversation_id, &guest.id).await {
        Ok(rows) => Json(ApiResponse::success(rows)),
        Err(e) => Json(ApiResponse::error(e.to_string())),
    }
}

#[derive(Debug, Deserialize)]
struct ToolDispatchBody {
    name: String,
    #[serde(default)]
    input: serde_json::Value,
}

/// Loopback-only endpoint called by the `tool-bridge` subprocess. The
/// bridge authenticates with a per-spawn shared secret (passed to it via
/// its config file at spawn time); the secret must match a currently
/// active spawn in [`AppState::active_bridge_secrets`] or the request is
/// rejected. Separate from the guest-cookie middleware: bridges aren't
/// browsers and can't carry cookies.
async fn tools_dispatch(
    AxumState(state): AxumState<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ToolDispatchBody>,
) -> Json<ApiResponse<serde_json::Value>> {
    let presented = match headers
        .get("x-tool-bridge-secret")
        .and_then(|v| v.to_str().ok())
    {
        Some(s) => s.to_string(),
        None => return Json(ApiResponse::error("missing X-Tool-Bridge-Secret".into())),
    };

    // Authorize + resolve the owning WebSocket session. We need the
    // session ID for client-tool calls so we can route the UI request
    // back over the right socket.
    let session_id = {
        let secrets = state.active_bridge_secrets.lock().await;
        match secrets.get(&presented) {
            Some(sid) => sid.clone(),
            None => return Json(ApiResponse::error("unknown bridge secret".into())),
        }
    };

    // Server tool → run the Rust handler inline.
    // Client tool → round-trip through the WebSocket.
    let runtime = state
        .tools
        .specs()
        .iter()
        .find(|s| s.name == body.name)
        .map(|s| s.runtime);
    match runtime {
        Some(crate::core::tools::ToolRuntime::Server) => {
            match state.tools.dispatch(&body.name, body.input).await {
                Ok(v) => Json(ApiResponse::success(v)),
                Err(e) => Json(ApiResponse::error(e.to_string())),
            }
        }
        Some(crate::core::tools::ToolRuntime::Client) => {
            match dispatch_client_tool(&state, &session_id, &body.name, body.input).await {
                Ok(v) => Json(ApiResponse::success(v)),
                Err(e) => Json(ApiResponse::error(e.to_string())),
            }
        }
        None => Json(ApiResponse::error(format!("unknown tool: {}", body.name))),
    }
}

/// Push a `tool_call_for_ui` message to the session's WebSocket, then
/// await the matching `tool_result_from_ui` (delivered via oneshot) with
/// a timeout. Returns the user-supplied result as the tool output.
async fn dispatch_client_tool(
    state: &AppState,
    session_id: &str,
    name: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let tool_call_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel::<serde_json::Value>();
    state
        .pending_client_tools
        .lock()
        .await
        .insert(tool_call_id.clone(), tx);

    send_to_session(
        state,
        session_id,
        json!({
            "type": "tool_call_for_ui",
            "tool_call_id": tool_call_id,
            "name": name,
            "input": input,
            "session_id": session_id,
        })
        .to_string(),
    )
    .await;

    let timeout = std::time::Duration::from_secs(
        std::env::var("APP_CLIENT_TOOL_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(120),
    );
    let outcome = tokio::time::timeout(timeout, rx).await;
    // Always try to clear the pending entry on exit.
    state
        .pending_client_tools
        .lock()
        .await
        .remove(&tool_call_id);

    match outcome {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err("frontend dropped the tool call before responding".into()),
        Err(_) => Err(format!(
            "timed out after {}s waiting for UI to respond to '{}'",
            timeout.as_secs(),
            name
        )),
    }
}

// ---------------------------------------------------------------------------
// Dataset upload + session binding.
// ---------------------------------------------------------------------------

/// Hard cap on the in-memory list of columns we return to the frontend.
/// Wider datasets still upload, but the UI chip only surfaces the first
/// 200 (the full set remains queryable via SQL).
const MAX_COLUMNS_IN_RESPONSE: usize = 200;

/// Multipart upload endpoint: reads a single `file` part, persists it to
/// `$TMPDIR/claude-ui-ds-<id>.<ext>`, parses the schema + a 5-row
/// sample, and registers a `DatasetRecord` owned by the requesting
/// guest cookie. Returns the id + schema so the frontend can render a
/// chip and later bind the dataset to a session.
async fn upload_dataset(
    AxumState(state): AxumState<AppState>,
    axum::Extension(guest): axum::Extension<crate::core::cookies::GuestSession>,
    mut multipart: Multipart,
) -> Response {
    let mut file_bytes: Option<(String, Vec<u8>)> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.name() == Some("file") {
                    let filename = field
                        .file_name()
                        .map(String::from)
                        .unwrap_or_else(|| "upload.csv".to_string());
                    match field.bytes().await {
                        Ok(bytes) => {
                            file_bytes = Some((filename, bytes.to_vec()));
                            break;
                        }
                        Err(e) => {
                            return (
                                StatusCode::BAD_REQUEST,
                                Json(json!({
                                    "success": false,
                                    "error": format!("reading upload body: {e}")
                                })),
                            )
                                .into_response();
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "success": false,
                        "error": format!("reading multipart: {e}")
                    })),
                )
                    .into_response();
            }
        }
    }

    let (filename, bytes) = match file_bytes {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": "expected a 'file' field in the multipart upload"
                })),
            )
                .into_response();
        }
    };

    // Detect format from the extension. Default to CSV.
    let lower = filename.to_ascii_lowercase();
    let format: &str = if lower.ends_with(".json") {
        "json"
    } else if lower.ends_with(".csv") || lower.ends_with(".tsv") {
        "csv"
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "only .csv and .json uploads are supported"
            })),
        )
            .into_response();
    };

    let dataset_id = uuid::Uuid::new_v4().simple().to_string();
    let ext = if format == "json" { "json" } else { "csv" };
    let tmp_path = std::env::temp_dir().join(format!("claude-ui-ds-{}.{}", dataset_id, ext));
    if let Err(e) = std::fs::write(&tmp_path, &bytes) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": format!("writing dataset temp file: {e}")
            })),
        )
            .into_response();
    }

    let parse_result = if format == "json" {
        crate::core::datasets::parse_json_array(&tmp_path)
    } else {
        crate::core::datasets::parse_csv(&tmp_path)
    };
    let (mut columns, row_count, sample_rows) = match parse_result {
        Ok(t) => t,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": format!("could not parse {format}: {e}")
                })),
            )
                .into_response();
        }
    };
    if columns.len() > MAX_COLUMNS_IN_RESPONSE {
        columns.truncate(MAX_COLUMNS_IN_RESPONSE);
    }

    let record = DatasetRecord {
        dataset_id: dataset_id.clone(),
        path: tmp_path,
        format: format.to_string(),
        filename: filename.clone(),
        columns: columns.clone(),
        row_count,
        sample_rows: sample_rows.clone(),
        cookie_id: guest.id.clone(),
    };
    state.datasets.insert(record).await;

    Json(json!({
        "success": true,
        "data": {
            "dataset_id": dataset_id,
            "filename": filename,
            "format": format,
            "row_count": row_count,
            "columns": columns,
            "sample_rows": sample_rows,
        }
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct BindDatasetBody {
    session_id: String,
    dataset_id: String,
}

#[derive(Debug, Deserialize)]
struct UnbindDatasetBody {
    session_id: String,
}

/// Drop a session's dataset binding. Called by the frontend on "New
/// chat". The underlying DatasetRecord is preserved so the user can
/// rebind it elsewhere.
async fn unbind_dataset(
    AxumState(state): AxumState<AppState>,
    axum::Extension(_guest): axum::Extension<crate::core::cookies::GuestSession>,
    Json(body): Json<UnbindDatasetBody>,
) -> Json<ApiResponse<()>> {
    state.datasets.unbind(&body.session_id).await;
    Json(ApiResponse::success(()))
}

/// Bind an uploaded dataset to a chat session. The Claude spawn code
/// reads this mapping to decide whether to attach the Python MCP sidecar.
/// Ownership is enforced by the guest cookie; a caller can only bind a
/// dataset their own cookie uploaded.
async fn bind_dataset(
    AxumState(state): AxumState<AppState>,
    axum::Extension(guest): axum::Extension<crate::core::cookies::GuestSession>,
    Json(body): Json<BindDatasetBody>,
) -> Json<ApiResponse<()>> {
    let record = match state
        .datasets
        .get_owned(&body.dataset_id, &guest.id)
        .await
    {
        Some(r) => r,
        None => {
            return Json(ApiResponse::error(
                "dataset not found or not owned by this guest".into(),
            ));
        }
    };
    let binding = DatasetBinding {
        dataset_id: record.dataset_id,
        path: record.path,
        format: record.format,
        filename: record.filename,
        columns: record.columns,
        row_count: record.row_count,
    };
    state.datasets.bind(&body.session_id, binding).await;
    Json(ApiResponse::success(()))
}

/// A guard returned by [`prepare_python_bridge`]. Holds the two
/// tempfiles that back the MCP config + the data-server config. Dropping
/// it removes them from disk. Keep one live for the entire Claude
/// subprocess lifetime.
struct PythonBridgeHandle {
    mcp_config_path: std::path::PathBuf,
    allowed_tools: Vec<String>,
    system_prompt: String,
    _data_server_config_file: tempfile::NamedTempFile,
    _mcp_config_file: tempfile::NamedTempFile,
}

fn resolve_python_bin() -> Result<String, String> {
    if let Ok(p) = std::env::var("APP_PYTHON_BINARY") {
        return Ok(p);
    }
    for candidate in ["python3", "python"] {
        if which::which(candidate).is_ok() {
            return Ok(candidate.to_string());
        }
    }
    Err(
        "python3 not found on PATH. Install Python 3.10+ and \
         `pip install -r data_server/requirements.txt`, or set APP_PYTHON_BINARY."
            .to_string(),
    )
}

fn resolve_data_server_script() -> Result<std::path::PathBuf, String> {
    if let Ok(p) = std::env::var("APP_DATA_SERVER_SCRIPT") {
        let pb = std::path::PathBuf::from(p);
        if pb.exists() {
            return Ok(pb);
        }
        return Err(format!(
            "APP_DATA_SERVER_SCRIPT={} does not exist",
            pb.display()
        ));
    }
    // Look relative to the current working directory first (matches the
    // typical `cd backend && cargo run` flow), then one level up. Forks
    // running the binary from a different location can set
    // APP_DATA_SERVER_SCRIPT.
    let candidates = [
        std::path::PathBuf::from("data_server/server.py"),
        std::path::PathBuf::from("../data_server/server.py"),
    ];
    for c in candidates {
        if c.exists() {
            return Ok(c.canonicalize().unwrap_or(c));
        }
    }
    Err(
        "data_server/server.py not found. Run the backend from the repo root \
         (or its `backend/` directory), or set APP_DATA_SERVER_SCRIPT."
            .to_string(),
    )
}

/// If the session has a bound dataset, write the data-server config +
/// MCP config tempfiles and return a handle describing the CLI flags to
/// add. Returns `Ok(None)` when no dataset is bound — plain chat mode.
async fn prepare_python_bridge(
    state: &AppState,
    session_id: &str,
) -> Result<Option<PythonBridgeHandle>, String> {
    let binding = match state.datasets.binding(session_id).await {
        Some(b) => b,
        None => return Ok(None),
    };
    let python_bin = resolve_python_bin()?;
    let script = resolve_data_server_script()?;

    let data_cfg = json!({
        "dataset_path": binding.path.to_string_lossy(),
        "format": binding.format,
        "filename": binding.filename,
    });
    let mut data_file = tempfile::NamedTempFile::new()
        .map_err(|e| format!("creating data-server config: {e}"))?;
    std::io::Write::write_all(&mut data_file, data_cfg.to_string().as_bytes())
        .map_err(|e| format!("writing data-server config: {e}"))?;

    let mcp_cfg = json!({
        "mcpServers": {
            "viz-tools": {
                "command": python_bin,
                "args": [script.to_string_lossy()],
                "env": {
                    "DATA_SERVER_CONFIG": data_file.path().to_string_lossy(),
                }
            }
        }
    });
    let mut mcp_file = tempfile::NamedTempFile::new()
        .map_err(|e| format!("creating mcp config: {e}"))?;
    std::io::Write::write_all(&mut mcp_file, mcp_cfg.to_string().as_bytes())
        .map_err(|e| format!("writing mcp config: {e}"))?;

    let allowed_tools = vec![
        "mcp__viz-tools__describe_dataset".into(),
        "mcp__viz-tools__query_dataset".into(),
        "mcp__viz-tools__create_chart".into(),
    ];

    let cols_str = binding
        .columns
        .iter()
        .take(50)
        .map(|c| format!("{} ({})", c.name, c.dtype))
        .collect::<Vec<_>>()
        .join(", ");
    let system_prompt = format!(
        "You are analyzing the dataset '{filename}' ({rows} rows). Columns: {cols}. \
         The dataset is available as a SQL table named 'data' via the Python MCP server 'viz-tools'. \
         Tool guide: \
         - describe_dataset() — schema + 5 sample rows. Call this before answering if you're unsure of types. \
         - query_dataset(sql) — read-only SELECT against table 'data'. Use for numeric answers. \
         - create_chart(sql, mark, x, y, color?, title?) — render a chart inline. Supported marks: line, bar, area, point, tick. \
         When the question is about a trend, distribution, or comparison, always produce a chart with create_chart. \
         Interleave short prose with charts: (1) restate the question, (2) call the tool, (3) interpret the result in 1-2 sentences. \
         Keep SQL simple and aggregate when possible; charts with more than a few hundred points are noisy.",
        filename = binding.filename,
        rows = binding.row_count,
        cols = cols_str,
    );

    Ok(Some(PythonBridgeHandle {
        mcp_config_path: mcp_file.path().to_path_buf(),
        allowed_tools,
        system_prompt,
        _data_server_config_file: data_file,
        _mcp_config_file: mcp_file,
    }))
}

/// Cancel a running Claude subprocess by session ID. Looks up the mpsc
/// cancel channel registered by the spawn function and sends on it; the
/// spawn task picks the signal up via `tokio::select!`, calls `start_kill()`
/// on the child, and emits a `cancelled` event to the WebSocket.
async fn cancel_claude_execution(
    Path(session_id): Path<String>,
    AxumState(state): AxumState<AppState>,
) -> Json<ApiResponse<()>> {
    let tx_opt = state
        .cancel_channels
        .lock()
        .await
        .get(&session_id)
        .cloned();
    match tx_opt {
        Some(tx) => {
            // `send` returns Err only if the receiver is gone, which means
            // the process already exited — treat as success either way.
            let _ = tx.send(()).await;
            Json(ApiResponse::success(()))
        }
        None => Json(ApiResponse::error(format!(
            "No active session to cancel: {}",
            session_id
        ))),
    }
}

/// WebSocket handler for Claude execution with streaming output
async fn claude_websocket(
    ws: WebSocketUpgrade,
    AxumState(state): AxumState<AppState>,
    axum::Extension(guest): axum::Extension<crate::core::cookies::GuestSession>,
) -> Response {
    let cookie_id = guest.id;
    ws.on_upgrade(move |socket| claude_websocket_handler(socket, state, cookie_id))
}

async fn claude_websocket_handler(socket: WebSocket, state: AppState, cookie_id: String) {
    let (mut sender, mut receiver) = socket.split();
    let session_id = uuid::Uuid::new_v4().to_string();

    println!(
        "[TRACE] WebSocket handler started - session_id: {}",
        session_id
    );

    // Channel for sending output to WebSocket
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(100);

    // Store session in state
    {
        let mut sessions = state.active_sessions.lock().await;
        sessions.insert(session_id.clone(), tx);
        println!(
            "[TRACE] Session stored in state - active sessions count: {}",
            sessions.len()
        );
    }

    // Task to forward channel messages to WebSocket
    let session_id_for_forward = session_id.clone();
    let forward_task = tokio::spawn(async move {
        println!(
            "[TRACE] Forward task started for session {}",
            session_id_for_forward
        );
        while let Some(message) = rx.recv().await {
            println!("[TRACE] Forwarding message to WebSocket: {}", message);
            if sender.send(Message::Text(message.into())).await.is_err() {
                println!("[TRACE] Failed to send message to WebSocket - connection closed");
                break;
            }
        }
        println!(
            "[TRACE] Forward task ended for session {}",
            session_id_for_forward
        );
    });

    // Handle incoming messages from WebSocket
    println!("[TRACE] Starting to listen for WebSocket messages");
    while let Some(msg) = receiver.next().await {
        println!("[TRACE] Received WebSocket message: {:?}", msg);
        if let Ok(msg) = msg {
            if let Message::Text(text) = msg {
                // Inbound messages are one of two shapes:
                //  - ClaudeExecutionRequest: user prompting Claude.
                //  - tool_result_from_ui: browser responding to a client
                //    tool that the backend is currently awaiting.
                // We peek at the `type` field first to decide.
                if let Ok(generic) = serde_json::from_str::<serde_json::Value>(&text) {
                    if generic.get("type").and_then(|v| v.as_str())
                        == Some("tool_result_from_ui")
                    {
                        let id = generic
                            .get("tool_call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let content = generic.get("content").cloned().unwrap_or(json!(null));
                        let tx_opt = state
                            .pending_client_tools
                            .lock()
                            .await
                            .remove(&id);
                        match tx_opt {
                            Some(tx) => {
                                let _ = tx.send(content);
                            }
                            None => println!(
                                "[TRACE] tool_result_from_ui for unknown id {}",
                                id
                            ),
                        }
                        continue;
                    }
                }

                println!(
                    "[TRACE] WebSocket text message received - length: {} chars",
                    text.len()
                );
                println!("[TRACE] WebSocket message content: {}", text);
                match serde_json::from_str::<ClaudeExecutionRequest>(&text) {
                    Ok(request) => {
                        println!("[TRACE] Successfully parsed request: {:?}", request);
                        println!("[TRACE] Command type: {}", request.command_type);
                        println!("[TRACE] Project path: {}", request.project_path);
                        println!("[TRACE] Prompt length: {} chars", request.prompt.len());

                        // Prefer the client-supplied session ID so that the
                        // Claude subprocess and the frontend agree on the
                        // conversation UUID (and therefore on the JSONL file
                        // it writes). Fall back to the per-connection UUID
                        // for legacy clients.
                        let session_id_clone = request
                            .client_session_id
                            .clone()
                            .unwrap_or_else(|| session_id.clone());
                        let client_session_id = request.client_session_id.clone();
                        let state_clone = state.clone();

                        // Mirror the WebSocket's mpsc sender under the
                        // client-supplied session key so `send_to_session`
                        // can route events when the executor uses that ID.
                        if let Some(ref cid) = client_session_id {
                            if cid != &session_id {
                                let tx_opt = state_clone
                                    .active_sessions
                                    .lock()
                                    .await
                                    .get(&session_id)
                                    .cloned();
                                if let Some(tx) = tx_opt {
                                    state_clone
                                        .active_sessions
                                        .lock()
                                        .await
                                        .insert(cid.clone(), tx);
                                }
                            }
                        }

                        // Rate limits: per-cookie message budget first, then
                        // concurrent-conversation slot. Failures become
                        // `rate_limited` WebSocket events; the spawn is
                        // skipped and the guest is told to back off.
                        if let Err(e) =
                            state_clone.rate_limiter.try_record_message(&cookie_id).await
                        {
                            send_to_session(
                                &state_clone,
                                &session_id_clone,
                                json!({
                                    "type": "rate_limited",
                                    "message": e,
                                    "session_id": session_id_clone,
                                })
                                .to_string(),
                            )
                            .await;
                            continue;
                        }
                        if let Err(e) = state_clone
                            .rate_limiter
                            .try_claim_conversation(&cookie_id)
                            .await
                        {
                            send_to_session(
                                &state_clone,
                                &session_id_clone,
                                json!({
                                    "type": "rate_limited",
                                    "message": e,
                                    "session_id": session_id_clone,
                                })
                                .to_string(),
                            )
                            .await;
                            continue;
                        }

                        // Persist: associate this conversation with the
                        // guest cookie (creates the row if new, rejects it
                        // if it belongs to a different cookie), then log
                        // the user prompt. On ownership conflict or DB
                        // error we abort the spawn rather than silently
                        // running for the wrong identity.
                        if let Err(e) = state_clone
                            .store
                            .ensure_conversation(&session_id_clone, &cookie_id)
                            .await
                        {
                            state_clone
                                .rate_limiter
                                .release_conversation(&cookie_id)
                                .await;
                            let err = format!(
                                "conversation {} not available: {}",
                                session_id_clone, e
                            );
                            send_to_session(
                                &state_clone,
                                &session_id_clone,
                                json!({
                                    "type": "error",
                                    "message": err,
                                    "session_id": session_id_clone,
                                })
                                .to_string(),
                            )
                            .await;
                            continue;
                        }
                        let _ = state_clone
                            .store
                            .append_message(
                                &session_id_clone,
                                "user",
                                &json!({
                                    "command_type": request.command_type,
                                    "prompt": request.prompt,
                                    "model": request.model,
                                }),
                            )
                            .await;

                        println!(
                            "[TRACE] Spawning task to execute command: {}",
                            request.command_type
                        );
                        // Clone the cookie for the spawn so it can release
                        // the concurrent-conversation slot when done.
                        let cookie_for_release = cookie_id.clone();
                        tokio::spawn(async move {
                            println!("[TRACE] Task started for command execution");
                            let result = match request.command_type.as_str() {
                                "execute" => {
                                    execute_claude_command(
                                        request.project_path,
                                        request.prompt,
                                        request.model.unwrap_or_default(),
                                        session_id_clone.clone(),
                                        client_session_id.clone(),
                                        request.extra.clone(),
                                        state_clone.clone(),
                                    )
                                    .await
                                }
                                "continue" => {
                                    continue_claude_command(
                                        request.project_path,
                                        request.prompt,
                                        request.model.unwrap_or_default(),
                                        session_id_clone.clone(),
                                        client_session_id.clone(),
                                        request.extra.clone(),
                                        state_clone.clone(),
                                    )
                                    .await
                                }
                                "resume" => {
                                    resume_claude_command(
                                        request.project_path,
                                        request.session_id.unwrap_or_default(),
                                        request.prompt,
                                        request.model.unwrap_or_default(),
                                        session_id_clone.clone(),
                                        request.extra.clone(),
                                        state_clone.clone(),
                                    )
                                    .await
                                }
                                _ => {
                                    println!(
                                        "[TRACE] Unknown command type: {}",
                                        request.command_type
                                    );
                                    Err("Unknown command type".to_string())
                                }
                            };

                            println!(
                                "[TRACE] Command execution finished with result: {:?}",
                                result
                            );

                            // Send completion message
                            if let Some(sender) = state_clone
                                .active_sessions
                                .lock()
                                .await
                                .get(&session_id_clone)
                            {
                                let completion_msg = match result {
                                    Ok(_) => json!({
                                        "type": "completion",
                                        "status": "success",
                                        "session_id": session_id_clone,
                                    }),
                                    Err(e) => json!({
                                        "type": "completion",
                                        "status": "error",
                                        "error": e,
                                        "session_id": session_id_clone,
                                    }),
                                };
                                println!("[TRACE] Sending completion message: {}", completion_msg);
                                let _ = sender.send(completion_msg.to_string()).await;
                            } else {
                                println!("[TRACE] Session not found in active sessions when sending completion");
                            }

                            // Always release the per-cookie concurrent
                            // conversation slot, regardless of whether the
                            // Claude run succeeded.
                            state_clone
                                .rate_limiter
                                .release_conversation(&cookie_for_release)
                                .await;
                        });
                    }
                    Err(e) => {
                        println!("[TRACE] Failed to parse WebSocket request: {}", e);
                        println!("[TRACE] Raw message that failed to parse: {}", text);

                        // Send error back to client
                        let error_msg = json!({
                            "type": "error",
                            "message": format!("Failed to parse request: {}", e)
                        });
                        if let Some(sender_tx) = state.active_sessions.lock().await.get(&session_id)
                        {
                            let _ = sender_tx.send(error_msg.to_string()).await;
                        }
                    }
                }
            } else if let Message::Close(_) = msg {
                println!("[TRACE] WebSocket close message received");
                break;
            } else {
                println!("[TRACE] Non-text WebSocket message received: {:?}", msg);
            }
        } else {
            println!("[TRACE] Error receiving WebSocket message");
        }
    }

    println!("[TRACE] WebSocket message loop ended");

    // Clean up session
    {
        let mut sessions = state.active_sessions.lock().await;
        sessions.remove(&session_id);
        println!(
            "[TRACE] Session {} removed from state - remaining sessions: {}",
            session_id,
            sessions.len()
        );
    }
    // NB: we deliberately do *not* drop the dataset binding here — the
    // frontend opens a fresh WebSocket for every turn, so unbinding on
    // close would lose the dataset between turns. Bindings are cleared
    // by the frontend calling POST /api/datasets/unbind on "New chat".

    forward_task.abort();
    println!("[TRACE] WebSocket handler ended for session {}", session_id);
}

/// A guard returned by [`prepare_tool_bridge`]. Holds the temp files that
/// back `--mcp-config` + the bridge's own config, and the active-secret
/// registration. Dropping it unregisters the secret and removes the temp
/// files. Keep one live for the entire Claude subprocess lifetime.
struct ToolBridgeHandle {
    /// Path passed to `claude --mcp-config`.
    mcp_config_path: std::path::PathBuf,
    /// Tool names to put on `--allowed-tools`.
    allowed_tools: Vec<String>,
    /// Keeps the tempfile alive.
    _bridge_config_file: tempfile::NamedTempFile,
    _mcp_config_file: tempfile::NamedTempFile,
    secret: String,
    state: AppState,
}

impl Drop for ToolBridgeHandle {
    fn drop(&mut self) {
        // Fire-and-forget: we don't have an async context in Drop, so
        // use try_lock — if the mutex is contended we leak the entry
        // until the next server restart. Acceptable for the template.
        if let Ok(mut guard) = self.state.active_bridge_secrets.try_lock() {
            guard.remove(&self.secret);
        }
    }
}

/// Locate the `tool-bridge` binary. By convention Cargo puts sibling bins
/// in the same target directory, so we look next to the current exe
/// first. If not found there (e.g. running `cargo run` for the main
/// binary but only the release bridge has been built, or vice versa) we
/// also check the sibling profile directory under `target/`. Forks
/// running the bridge from a custom path can set `APP_TOOL_BRIDGE_PATH`.
fn resolve_tool_bridge_path() -> Result<std::path::PathBuf, String> {
    if let Ok(explicit) = std::env::var("APP_TOOL_BRIDGE_PATH") {
        return Ok(std::path::PathBuf::from(explicit));
    }
    #[cfg(windows)]
    let bin_name = "tool-bridge.exe";
    #[cfg(not(windows))]
    let bin_name = "tool-bridge";

    let exe = std::env::current_exe()
        .map_err(|e| format!("resolving current exe for tool-bridge: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "current exe has no parent dir".to_string())?;

    let primary = dir.join(bin_name);
    if primary.exists() {
        return Ok(primary);
    }

    // Fallback: if we're in `target/<profile>/`, try the sibling profile.
    // Lets `cargo run --bin claude-ui-app` (debug) pick up a release
    // tool-bridge that the user already built, and vice versa.
    if let (Some(profile), Some(target_dir)) =
        (dir.file_name().and_then(|n| n.to_str()), dir.parent())
    {
        for sibling in ["release", "debug"] {
            if sibling == profile {
                continue;
            }
            let candidate = target_dir.join(sibling).join(bin_name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    Err(format!(
        "tool-bridge binary not found at {}. Build it with `cargo build --bin tool-bridge` (or `--release --bin tool-bridge`), or set APP_TOOL_BRIDGE_PATH.",
        primary.display()
    ))
}

/// Write the bridge config + MCP config temp files and register the
/// per-spawn secret against `session_id` so client-tool calls can be
/// routed back to the right WebSocket. If the registry is empty, returns
/// `Ok(None)` — forks with zero tools shouldn't get `--mcp-config`
/// appended at all.
async fn prepare_tool_bridge(
    state: &AppState,
    session_id: &str,
) -> Result<Option<ToolBridgeHandle>, String> {
    let specs = state.tools.specs();
    if specs.is_empty() {
        return Ok(None);
    }

    let bridge_path = resolve_tool_bridge_path()?;
    let secret = uuid::Uuid::new_v4().simple().to_string();

    // Bridge config: upstream URL + secret + tool manifest.
    let bridge_cfg = serde_json::json!({
        "upstream_url": state.self_url,
        "secret": secret,
        "tools": specs.iter().map(|s| serde_json::json!({
            "name": s.name,
            "description": s.description,
            "input_schema": s.input_schema,
        })).collect::<Vec<_>>(),
    });
    let mut bridge_file = tempfile::NamedTempFile::new()
        .map_err(|e| format!("creating bridge config tempfile: {e}"))?;
    std::io::Write::write_all(
        &mut bridge_file,
        serde_json::to_vec_pretty(&bridge_cfg).unwrap().as_slice(),
    )
    .map_err(|e| format!("writing bridge config: {e}"))?;

    // MCP config: tells Claude to spawn our bridge bin with the above
    // config path passed via env var.
    let mcp_cfg = serde_json::json!({
        "mcpServers": {
            crate::core::tools::MCP_SERVER_NAME: {
                "command": bridge_path.to_string_lossy(),
                "args": [],
                "env": {
                    "APP_TOOL_BRIDGE_CONFIG": bridge_file.path().to_string_lossy(),
                }
            }
        }
    });
    let mut mcp_file = tempfile::NamedTempFile::new()
        .map_err(|e| format!("creating mcp config tempfile: {e}"))?;
    std::io::Write::write_all(
        &mut mcp_file,
        serde_json::to_vec_pretty(&mcp_cfg).unwrap().as_slice(),
    )
    .map_err(|e| format!("writing mcp config: {e}"))?;

    // Register the secret so /__tools/dispatch will accept it and knows
    // which WebSocket to route client-tool calls back through.
    state
        .active_bridge_secrets
        .lock()
        .await
        .insert(secret.clone(), session_id.to_string());

    Ok(Some(ToolBridgeHandle {
        mcp_config_path: mcp_file.path().to_path_buf(),
        allowed_tools: state.tools.allowed_tool_names(),
        _bridge_config_file: bridge_file,
        _mcp_config_file: mcp_file,
        secret,
        state: state.clone(),
    }))
}

// Claude command execution functions for WebSocket streaming
async fn execute_claude_command(
    project_path: String,
    prompt: String,
    model: String,
    session_id: String,
    client_session_id: Option<String>,
    extra: ClaudeExtraArgs,
    state: AppState,
) -> Result<(), String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;

    println!("[TRACE] execute_claude_command called:");
    println!("[TRACE]   project_path: {}", project_path);
    println!("[TRACE]   prompt length: {} chars", prompt.len());
    println!("[TRACE]   model: {}", model);
    println!("[TRACE]   session_id: {}", session_id);
    println!("[TRACE]   client_session_id: {:?}", client_session_id);

    // Send initial message
    println!("[TRACE] Sending initial start message");
    send_to_session(
        &state,
        &session_id,
        json!({
            "type": "start",
            "message": "Starting Claude execution..."
        })
        .to_string(),
    )
    .await;

    // Find Claude binary (simplified for web mode)
    println!("[TRACE] Finding Claude binary...");
    let claude_path = find_claude_binary_web().map_err(|e| {
        let error = format!("Claude binary not found: {}", e);
        println!("[TRACE] Error finding Claude binary: {}", error);
        error
    })?;
    println!("[TRACE] Found Claude binary: {}", claude_path);

    // Create Claude command
    println!("[TRACE] Creating Claude command...");
    let mut cmd = Command::new(&claude_path);
    let mut args: Vec<String> = vec![
        "-p".into(),
        prompt.clone(),
        "--model".into(),
        model.clone(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
    ];
    if resolve_skip_permissions(&extra) {
        args.push("--dangerously-skip-permissions".into());
    }
    if let Some(default_tools) = resolve_default_tools(&extra) {
        args.push("--tools".into());
        args.push(default_tools);
    }
    if let Some(ref cid) = client_session_id {
        args.push("--session-id".into());
        args.push(cid.clone());
    }
    // Wire the Rust tool bridge (registered server/client tools) AND the
    // Python MCP sidecar (viz-tools: describe_dataset, query_dataset,
    // create_chart) when the session has a bound dataset. Both handles
    // stay alive for the duration of this function; their RAII guards
    // clean up temp files + secret registration on drop.
    let bridge = prepare_tool_bridge(&state, &session_id).await?;
    if let Some(ref b) = bridge {
        args.push("--mcp-config".into());
        args.push(b.mcp_config_path.to_string_lossy().into_owned());
        if !b.allowed_tools.is_empty() {
            args.push("--allowed-tools".into());
            args.push(b.allowed_tools.join(","));
        }
    }
    let py_bridge = prepare_python_bridge(&state, &session_id).await?;
    if let Some(ref pb) = py_bridge {
        args.push("--mcp-config".into());
        args.push(pb.mcp_config_path.to_string_lossy().into_owned());
        args.push("--allowed-tools".into());
        args.push(pb.allowed_tools.join(","));
        args.push("--append-system-prompt".into());
        args.push(pb.system_prompt.clone());
    }
    append_extra_args(&mut args, &extra);
    cmd.args(&args);
    if !project_path.is_empty() {
        cmd.current_dir(&project_path);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    println!(
        "[TRACE] Command: {} {:?} (cwd override: {})",
        claude_path,
        args,
        if project_path.is_empty() { "<none>" } else { project_path.as_str() }
    );

    // Spawn Claude process
    println!("[TRACE] Spawning Claude process...");
    let mut child = cmd.spawn().map_err(|e| {
        let error = format!("Failed to spawn Claude: {}", e);
        println!("[TRACE] Spawn error: {}", error);
        error
    })?;
    println!("[TRACE] Claude process spawned successfully");

    // Get stdout and stderr. stderr is drained on a side task so error output
    // surfaces to the UI rather than piling up in the pipe and deadlocking
    // the child when the buffer fills.
    let stdout = child.stdout.take().ok_or_else(|| {
        println!("[TRACE] Failed to get stdout from child process");
        "Failed to get stdout".to_string()
    })?;
    let stdout_reader = BufReader::new(stdout);

    if let Some(stderr) = child.stderr.take() {
        let stderr_state = state.clone();
        let stderr_session = session_id.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[CLAUDE STDERR] {}", line);
                send_to_session(
                    &stderr_state,
                    &stderr_session,
                    json!({
                        "type": "error",
                        "message": line,
                        "session_id": stderr_session,
                    })
                    .to_string(),
                )
                .await;
            }
        });
    }

    // Register a cancel channel so `/api/sessions/:id/cancel` can interrupt
    // this subprocess. The channel is deregistered when the loop exits.
    let (cancel_tx, mut cancel_rx) = tokio::sync::mpsc::channel::<()>(1);
    state
        .cancel_channels
        .lock()
        .await
        .insert(session_id.clone(), cancel_tx);

    println!("[TRACE] Starting to read Claude output...");
    let mut lines = stdout_reader.lines();
    let mut line_count = 0;
    let mut cancelled = false;
    loop {
        tokio::select! {
            line_res = lines.next_line() => {
                match line_res {
                    Ok(Some(line)) => {
                        line_count += 1;
                        let message = json!({
                            "type": "output",
                            "content": line,
                            "session_id": session_id,
                        })
                        .to_string();
                        send_to_session(&state, &session_id, message).await;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        println!("[TRACE] stdout read error: {}", e);
                        break;
                    }
                }
            }
            _ = cancel_rx.recv() => {
                println!("[TRACE] Cancel received for session {}", session_id);
                let _ = child.start_kill();
                cancelled = true;
                break;
            }
        }
    }

    // Drop the cancel registration before we wait on the child — the endpoint
    // should stop seeing this session as cancellable once we're past the read
    // loop.
    state.cancel_channels.lock().await.remove(&session_id);

    println!(
        "[TRACE] Finished reading Claude output ({} lines total, cancelled={})",
        line_count, cancelled
    );

    // Wait for process to complete (or die from our kill).
    let exit_status = child.wait().await.map_err(|e| {
        let error = format!("Failed to wait for Claude: {}", e);
        println!("[TRACE] Wait error: {}", error);
        error
    })?;

    if cancelled {
        send_to_session(
            &state,
            &session_id,
            json!({
                "type": "cancelled",
                "session_id": session_id,
            })
            .to_string(),
        )
        .await;
        return Ok(());
    }

    if !exit_status.success() {
        let error = format!(
            "Claude execution failed with exit code: {:?}",
            exit_status.code()
        );
        println!("[TRACE] Claude execution failed: {}", error);
        return Err(error);
    }

    println!("[TRACE] execute_claude_command completed successfully");
    Ok(())
}

async fn continue_claude_command(
    project_path: String,
    prompt: String,
    model: String,
    session_id: String,
    client_session_id: Option<String>,
    extra: ClaudeExtraArgs,
    state: AppState,
) -> Result<(), String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;

    send_to_session(
        &state,
        &session_id,
        json!({
            "type": "start",
            "message": "Continuing Claude session..."
        })
        .to_string(),
    )
    .await;

    // Find Claude binary
    let claude_path =
        find_claude_binary_web().map_err(|e| format!("Claude binary not found: {}", e))?;

    // Create continue command
    let mut cmd = Command::new(&claude_path);
    let mut args: Vec<String> = vec![
        "-c".into(), // Continue flag
        "-p".into(),
        prompt.clone(),
        "--model".into(),
        model.clone(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
    ];
    if resolve_skip_permissions(&extra) {
        args.push("--dangerously-skip-permissions".into());
    }
    if let Some(default_tools) = resolve_default_tools(&extra) {
        args.push("--tools".into());
        args.push(default_tools);
    }
    if let Some(ref cid) = client_session_id {
        args.push("--session-id".into());
        args.push(cid.clone());
    }
    let bridge = prepare_tool_bridge(&state, &session_id).await?;
    if let Some(ref b) = bridge {
        args.push("--mcp-config".into());
        args.push(b.mcp_config_path.to_string_lossy().into_owned());
        if !b.allowed_tools.is_empty() {
            args.push("--allowed-tools".into());
            args.push(b.allowed_tools.join(","));
        }
    }
    let py_bridge = prepare_python_bridge(&state, &session_id).await?;
    if let Some(ref pb) = py_bridge {
        args.push("--mcp-config".into());
        args.push(pb.mcp_config_path.to_string_lossy().into_owned());
        args.push("--allowed-tools".into());
        args.push(pb.allowed_tools.join(","));
        args.push("--append-system-prompt".into());
        args.push(pb.system_prompt.clone());
    }
    append_extra_args(&mut args, &extra);
    cmd.args(&args);
    if !project_path.is_empty() {
        cmd.current_dir(&project_path);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Spawn and stream output
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn Claude: {}", e))?;
    let stdout = child.stdout.take().ok_or("Failed to get stdout")?;
    let stdout_reader = BufReader::new(stdout);

    if let Some(stderr) = child.stderr.take() {
        let stderr_state = state.clone();
        let stderr_session = session_id.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[CLAUDE STDERR] {}", line);
                send_to_session(
                    &stderr_state,
                    &stderr_session,
                    json!({
                        "type": "error",
                        "message": line,
                        "session_id": stderr_session,
                    })
                    .to_string(),
                )
                .await;
            }
        });
    }

    let (cancel_tx, mut cancel_rx) = tokio::sync::mpsc::channel::<()>(1);
    state
        .cancel_channels
        .lock()
        .await
        .insert(session_id.clone(), cancel_tx);

    let mut lines = stdout_reader.lines();
    let mut cancelled = false;
    loop {
        tokio::select! {
            line_res = lines.next_line() => {
                match line_res {
                    Ok(Some(line)) => {
                        send_to_session(
                            &state,
                            &session_id,
                            json!({
                                "type": "output",
                                "content": line,
                                "session_id": session_id,
                            })
                            .to_string(),
                        )
                        .await;
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            _ = cancel_rx.recv() => {
                let _ = child.start_kill();
                cancelled = true;
                break;
            }
        }
    }

    state.cancel_channels.lock().await.remove(&session_id);

    let exit_status = child
        .wait()
        .await
        .map_err(|e| format!("Failed to wait for Claude: {}", e))?;

    if cancelled {
        send_to_session(
            &state,
            &session_id,
            json!({
                "type": "cancelled",
                "session_id": session_id,
            })
            .to_string(),
        )
        .await;
        return Ok(());
    }

    if !exit_status.success() {
        return Err(format!(
            "Claude execution failed with exit code: {:?}",
            exit_status.code()
        ));
    }

    Ok(())
}

async fn resume_claude_command(
    project_path: String,
    claude_session_id: String,
    prompt: String,
    model: String,
    session_id: String,
    extra: ClaudeExtraArgs,
    state: AppState,
) -> Result<(), String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;

    send_to_session(
        &state,
        &session_id,
        json!({
            "type": "start",
            "message": "Resuming Claude session..."
        })
        .to_string(),
    )
    .await;

    let claude_path =
        find_claude_binary_web().map_err(|e| format!("Claude binary not found: {}", e))?;

    let mut cmd = Command::new(&claude_path);
    let mut args: Vec<String> = vec![
        "--resume".into(),
        claude_session_id.clone(),
        "-p".into(),
        prompt.clone(),
        "--model".into(),
        model.clone(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
    ];
    if resolve_skip_permissions(&extra) {
        args.push("--dangerously-skip-permissions".into());
    }
    if let Some(default_tools) = resolve_default_tools(&extra) {
        args.push("--tools".into());
        args.push(default_tools);
    }
    let bridge = prepare_tool_bridge(&state, &session_id).await?;
    if let Some(ref b) = bridge {
        args.push("--mcp-config".into());
        args.push(b.mcp_config_path.to_string_lossy().into_owned());
        if !b.allowed_tools.is_empty() {
            args.push("--allowed-tools".into());
            args.push(b.allowed_tools.join(","));
        }
    }
    let py_bridge = prepare_python_bridge(&state, &session_id).await?;
    if let Some(ref pb) = py_bridge {
        args.push("--mcp-config".into());
        args.push(pb.mcp_config_path.to_string_lossy().into_owned());
        args.push("--allowed-tools".into());
        args.push(pb.allowed_tools.join(","));
        args.push("--append-system-prompt".into());
        args.push(pb.system_prompt.clone());
    }
    append_extra_args(&mut args, &extra);
    cmd.args(&args);
    if !project_path.is_empty() {
        cmd.current_dir(&project_path);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn Claude: {}", e))?;

    let stdout = child.stdout.take().ok_or("Failed to get stdout")?;
    let stdout_reader = BufReader::new(stdout);

    if let Some(stderr) = child.stderr.take() {
        let stderr_state = state.clone();
        let stderr_session = session_id.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[CLAUDE STDERR] {}", line);
                send_to_session(
                    &stderr_state,
                    &stderr_session,
                    json!({
                        "type": "error",
                        "message": line,
                        "session_id": stderr_session,
                    })
                    .to_string(),
                )
                .await;
            }
        });
    }

    let (cancel_tx, mut cancel_rx) = tokio::sync::mpsc::channel::<()>(1);
    state
        .cancel_channels
        .lock()
        .await
        .insert(session_id.clone(), cancel_tx);

    let mut lines = stdout_reader.lines();
    let mut cancelled = false;
    loop {
        tokio::select! {
            line_res = lines.next_line() => {
                match line_res {
                    Ok(Some(line)) => {
                        send_to_session(
                            &state,
                            &session_id,
                            json!({
                                "type": "output",
                                "content": line,
                                "session_id": session_id,
                            })
                            .to_string(),
                        )
                        .await;
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            _ = cancel_rx.recv() => {
                let _ = child.start_kill();
                cancelled = true;
                break;
            }
        }
    }

    state.cancel_channels.lock().await.remove(&session_id);

    let exit_status = child
        .wait()
        .await
        .map_err(|e| format!("Failed to wait for Claude: {}", e))?;

    if cancelled {
        send_to_session(
            &state,
            &session_id,
            json!({
                "type": "cancelled",
                "session_id": session_id,
            })
            .to_string(),
        )
        .await;
        return Ok(());
    }
    if !exit_status.success() {
        return Err(format!(
            "Claude execution failed with exit code: {:?}",
            exit_status.code()
        ));
    }

    Ok(())
}

async fn send_to_session(state: &AppState, session_id: &str, message: String) {
    // Persist the outbound message as a stream event so reloads can replay
    // the conversation. Best-effort: failures log but don't block delivery.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&message) {
        if let Err(e) = state
            .store
            .append_message(session_id, "stream", &v)
            .await
        {
            log::warn!("failed to persist stream event for {session_id}: {e}");
        }
    }

    let sessions = state.active_sessions.lock().await;
    if let Some(sender) = sessions.get(session_id) {
        if let Err(e) = sender.send(message).await {
            println!("[TRACE] Failed to send message: {}", e);
        }
    } else {
        println!("[TRACE] Session {} not found in active sessions", session_id);
    }
}

/// Create the web server.
///
/// `host` and `port` bind the listener; `cookies` supplies the HMAC signing
/// key for the guest-session cookie, which is the default identity layer
/// for customer-facing deployments. Forks that need admin-grade auth
/// (shared-secret or OAuth) should add their own layer on top of the
/// routes that need it — the guest cookie is the baseline, not the ceiling.
pub async fn create_web_server(
    host: std::net::IpAddr,
    port: u16,
    cookies: crate::core::cookies::CookieConfig,
    store: crate::core::conversations::ConversationStore,
    tools: crate::core::tools::ToolRegistry,
) -> Result<(), Box<dyn std::error::Error>> {
    // When binding 0.0.0.0 we still tell the bridge to call the server at
    // 127.0.0.1 — loopback-only dispatch is what makes the per-spawn secret
    // sufficient security.
    let loopback_host = if host.is_unspecified() {
        "127.0.0.1".to_string()
    } else {
        host.to_string()
    };
    let self_url = format!("http://{loopback_host}:{port}/__tools/dispatch");

    let state = AppState {
        active_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
        cancel_channels: Arc::new(Mutex::new(std::collections::HashMap::new())),
        store,
        rate_limiter: crate::core::ratelimit::RateLimiter::from_env(),
        tools,
        active_bridge_secrets: Arc::new(Mutex::new(std::collections::HashMap::new())),
        pending_client_tools: Arc::new(Mutex::new(std::collections::HashMap::new())),
        self_url,
        datasets: DatasetStore::default(),
    };

    let upload_limit_bytes: usize = std::env::var("APP_UPLOAD_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(25 * 1024 * 1024);

    // CORS policy: browser credentials (cookies) must travel with every
    // request, so we can't use `Any` origin + allow_credentials. Same-origin
    // is the right default for a single-binary deployment that serves both
    // the UI and the API. Forks behind a CDN or with a separate frontend
    // origin should override this (add an env-driven allowlist).
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(Any);

    // Router surface the template actually uses. Everything else is the
    // fork's to add. Kept deliberately narrow: customer-facing apps don't
    // want to expose project pickers, session history APIs, or anything
    // else that leaks host state.
    let app = Router::new()
        // Serve the Vite build.
        .route("/", get(serve_frontend))
        .route("/index.html", get(serve_frontend))
        // Guest-scoped conversation history (persisted under the signed
        // cookie; not yet consumed by the frontend — kept for forks that
        // want to wire history-replay in `useClaudeSession`).
        .route("/api/conversations", get(list_conversations))
        .route(
            "/api/conversations/{conversation_id}/messages",
            get(load_conversation_messages),
        )
        // Dataset upload + per-session binding. Upload has its own body
        // limit (default 25 MB); bind/unbind are small JSON requests.
        .route(
            "/api/datasets/upload",
            post(upload_dataset).layer(DefaultBodyLimit::max(upload_limit_bytes)),
        )
        .route("/api/datasets/bind", post(bind_dataset))
        .route("/api/datasets/unbind", post(unbind_dataset))
        // Internal: called by the tool-bridge subprocess via loopback only.
        // Protected by the per-spawn X-Tool-Bridge-Secret header.
        .route("/__tools/dispatch", axum::routing::post(tools_dispatch))
        // Cancel a running turn from the browser (used by
        // `useClaudeSession.cancel` + `reset`).
        .route(
            "/api/sessions/{sessionId}/cancel",
            get(cancel_claude_execution),
        )
        // WebSocket endpoint for real-time Claude execution.
        .route("/ws/claude", get(claude_websocket))
        // Serve static assets.
        .nest_service("/assets", ServeDir::new("../dist/assets"))
        .nest_service("/vite.svg", ServeDir::new("../dist/vite.svg"))
        .layer(cors)
        .layer(axum::middleware::from_fn_with_state(
            cookies.clone(),
            crate::core::cookies::guest_cookie_layer,
        ))
        .with_state(state);

    let addr = SocketAddr::from((host, port));
    println!(
        "🌐 Listening on http://{}:{} (guest-cookie sessions)",
        host, port
    );

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Convenience entrypoint used by `main.rs`. Resolves the cookie signing
/// key + db path from the environment and hands off to
/// [`create_web_server`]. `tools` is the fork-supplied registry.
pub async fn start_web_mode(
    host: std::net::IpAddr,
    port: u16,
    tools: crate::core::tools::ToolRegistry,
) -> Result<(), Box<dyn std::error::Error>> {
    let cookies = crate::core::cookies::CookieConfig::from_env();
    let db_path = crate::core::conversations::resolve_db_path();
    println!("💾 Conversation store at {}", db_path.display());
    let store = crate::core::conversations::ConversationStore::open(&db_path)
        .map_err(|e| -> Box<dyn std::error::Error> { format!("db open: {e}").into() })?;
    create_web_server(host, port, cookies, store, tools).await
}
