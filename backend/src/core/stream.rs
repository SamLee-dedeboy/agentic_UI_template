//! Typed decoder for Claude Code's `--output-format stream-json` output.
//!
//! Captured against Claude Code 2.1.109. The schema is additive — new fields
//! appear over time — so every struct here uses `#[serde(default)]` on optional
//! fields and retains the raw JSON via [`StreamMessage::raw`] so callers can
//! forward unrecognized content without losing fidelity.
//!
//! One line of `claude`'s stdout = one JSON object. Call [`parse_line`] per
//! line. Errors are surfaced (rather than swallowed) so the caller can log and
//! continue.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single stream-json line. The `type` discriminator drives the variant; the
/// raw JSON is preserved so callers can pass it through to UIs that want to
/// render fields not yet modeled here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamMessage {
    #[serde(flatten)]
    pub kind: StreamMessageKind,

    /// Every message in modern stream-json carries `session_id` at the top
    /// level. Older init messages also carried it; keeping it here as the
    /// canonical access point for routing events to the right session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Per-message UUID (stable across retries for the same logical event).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,

    /// The original JSON, preserved so the frontend can render fields not yet
    /// modeled in this decoder. Populated by [`parse_line`].
    #[serde(skip)]
    pub raw: Option<Value>,
}

/// Discriminated union keyed on `type`. Unknown types route to
/// [`StreamMessageKind::Unknown`] rather than failing the parse — Claude Code
/// adds new message types periodically.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamMessageKind {
    /// `type: "system"` — includes `subtype: "init"` carrying session metadata.
    System(SystemMessage),
    /// `type: "assistant"` — model output. `message.content` is an array of
    /// content blocks ([`ContentBlock`]).
    Assistant(ModelMessage),
    /// `type: "user"` — echoed user turn, or tool_result carrier.
    User(ModelMessage),
    /// `type: "result"` — terminal message with cost and usage totals.
    Result(ResultMessage),
    /// `type: "rate_limit_event"` — rate-limit state updates that can arrive
    /// mid-stream. Not fatal; surface to the UI for display.
    RateLimitEvent(RateLimitEvent),
    /// Anything else. Captured verbatim so the frontend can still render it.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerStatus>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, rename = "permissionMode")]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub slash_commands: Vec<String>,
    #[serde(default, rename = "apiKeySource")]
    pub api_key_source: Option<String>,
    #[serde(default)]
    pub claude_code_version: Option<String>,
    #[serde(default)]
    pub output_style: Option<String>,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub plugins: Vec<Value>,
    #[serde(default)]
    pub memory_paths: Option<Value>,
    #[serde(default)]
    pub fast_mode_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
}

/// Shared shape for `type: "assistant"` and `type: "user"` messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMessage {
    pub message: InnerMessage,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InnerMessage {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    /// Content may be a string (simple user turn) or an array of content
    /// blocks (every assistant turn, most user turns with tool results).
    #[serde(default)]
    pub content: MessageContent,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
    #[serde(default)]
    pub stop_details: Option<Value>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub context_management: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl Default for MessageContent {
    fn default() -> Self {
        MessageContent::Blocks(Vec::new())
    }
}

/// One element of `message.content[]`. Unknown block types are captured
/// verbatim so the UI can still render them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// Extended thinking block. `signature` is the model-produced signature
    /// that the API requires to be echoed back on subsequent turns.
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
        #[serde(default)]
        is_error: Option<bool>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Split of cache-creation tokens by TTL tier. Modern Claude Code reports
    /// both 5-minute and 1-hour ephemeral cache buckets separately; the
    /// aggregate `cache_creation_input_tokens` field remains for backward
    /// compatibility.
    #[serde(default)]
    pub cache_creation: Option<CacheCreationBreakdown>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub inference_geo: Option<String>,
    #[serde(default)]
    pub server_tool_use: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CacheCreationBreakdown {
    #[serde(default)]
    pub ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    pub ephemeral_1h_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultMessage {
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub duration_api_ms: u64,
    #[serde(default)]
    pub num_turns: u64,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// Terminal rollup cost. This is the authoritative number the template
    /// displays to users — no per-model pricing math in the app.
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub usage: Option<ResultUsage>,
    /// Per-model breakdown. Keys are full model IDs (e.g.
    /// `claude-sonnet-4-6`). Values include `costUSD`, context/output limits,
    /// and token counts.
    #[serde(default, rename = "modelUsage")]
    pub model_usage: std::collections::HashMap<String, ModelUsage>,
    #[serde(default)]
    pub permission_denials: Vec<Value>,
    #[serde(default)]
    pub terminal_reason: Option<String>,
    #[serde(default)]
    pub fast_mode_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResultUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation: Option<CacheCreationBreakdown>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub server_tool_use: Option<Value>,
    #[serde(default)]
    pub iterations: Vec<Value>,
    #[serde(default)]
    pub speed: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub web_search_requests: u64,
    #[serde(default, rename = "costUSD")]
    pub cost_usd: f64,
    #[serde(default)]
    pub context_window: u64,
    #[serde(default)]
    pub max_output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitEvent {
    #[serde(default)]
    pub rate_limit_info: Option<RateLimitInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitInfo {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "resetsAt")]
    pub resets_at: Option<i64>,
    #[serde(default, rename = "rateLimitType")]
    pub rate_limit_type: Option<String>,
    #[serde(default, rename = "overageStatus")]
    pub overage_status: Option<String>,
    #[serde(default, rename = "overageDisabledReason")]
    pub overage_disabled_reason: Option<String>,
    #[serde(default, rename = "isUsingOverage")]
    pub is_using_overage: bool,
}

/// Parse a single stream-json line. Returns the typed message with the raw
/// JSON attached for pass-through.
pub fn parse_line(line: &str) -> Result<StreamMessage, serde_json::Error> {
    let raw: Value = serde_json::from_str(line)?;
    let mut msg: StreamMessage = serde_json::from_value(raw.clone())?;
    msg.raw = Some(raw);
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_system_init() {
        let line = r#"{"type":"system","subtype":"init","cwd":"/tmp","session_id":"abc","tools":["Bash","Edit"],"model":"claude-haiku-4-5-20251001","permissionMode":"default","claude_code_version":"2.1.109","uuid":"u1"}"#;
        let m = parse_line(line).expect("parses");
        assert_eq!(m.session_id.as_deref(), Some("abc"));
        assert_eq!(m.uuid.as_deref(), Some("u1"));
        match m.kind {
            StreamMessageKind::System(s) => {
                assert_eq!(s.subtype.as_deref(), Some("init"));
                assert_eq!(s.model.as_deref(), Some("claude-haiku-4-5-20251001"));
                assert_eq!(s.tools, vec!["Bash".to_string(), "Edit".to_string()]);
            }
            _ => panic!("expected system"),
        }
    }

    #[test]
    fn parses_assistant_with_thinking_and_text() {
        let line = r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_1","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"ponder","signature":"sig"},{"type":"text","text":"Hi"}],"usage":{"input_tokens":10,"output_tokens":3,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":28938}}},"session_id":"abc","uuid":"u2"}"#;
        let m = parse_line(line).expect("parses");
        match m.kind {
            StreamMessageKind::Assistant(a) => {
                let usage = a.message.usage.expect("usage present");
                assert_eq!(usage.input_tokens, 10);
                let tier = usage.cache_creation.expect("tier present");
                assert_eq!(tier.ephemeral_1h_input_tokens, 28938);
                match a.message.content {
                    MessageContent::Blocks(blocks) => {
                        assert_eq!(blocks.len(), 2);
                        matches!(blocks[0], ContentBlock::Thinking { .. });
                        matches!(blocks[1], ContentBlock::Text { .. });
                    }
                    _ => panic!("expected blocks"),
                }
            }
            _ => panic!("expected assistant"),
        }
    }

    #[test]
    fn parses_result_with_model_usage_and_cost() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":2268,"num_turns":1,"result":"hi","total_cost_usd":0.037,"session_id":"abc","modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":356,"outputTokens":139,"cacheReadInputTokens":0,"cacheCreationInputTokens":28938,"webSearchRequests":0,"costUSD":0.037,"contextWindow":200000,"maxOutputTokens":32000}},"permission_denials":[],"terminal_reason":"completed","uuid":"u3"}"#;
        let m = parse_line(line).expect("parses");
        match m.kind {
            StreamMessageKind::Result(r) => {
                assert_eq!(r.total_cost_usd, Some(0.037));
                let mu = r
                    .model_usage
                    .get("claude-haiku-4-5-20251001")
                    .expect("model usage");
                assert_eq!(mu.cost_usd, 0.037);
                assert_eq!(mu.context_window, 200_000);
            }
            _ => panic!("expected result"),
        }
    }

    #[test]
    fn parses_rate_limit_event() {
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1776290400,"rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"org_level_disabled","isUsingOverage":false},"uuid":"u4","session_id":"abc"}"#;
        let m = parse_line(line).expect("parses");
        match m.kind {
            StreamMessageKind::RateLimitEvent(ev) => {
                let info = ev.rate_limit_info.expect("info");
                assert_eq!(info.status.as_deref(), Some("allowed"));
                assert_eq!(info.rate_limit_type.as_deref(), Some("five_hour"));
            }
            _ => panic!("expected rate_limit_event"),
        }
    }

    #[test]
    fn unknown_type_does_not_fail() {
        let line = r#"{"type":"some_future_type","session_id":"abc","uuid":"u5"}"#;
        let m = parse_line(line).expect("parses");
        matches!(m.kind, StreamMessageKind::Unknown);
        assert_eq!(m.session_id.as_deref(), Some("abc"));
    }
}
