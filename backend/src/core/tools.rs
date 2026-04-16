//! Tool registry — the central seam for domain tools Claude can call.
//!
//! A fork populates a [`ToolRegistry`] in `main.rs`. Each tool carries:
//!
//! - a **name** (Claude sees it as `mcp__template-tools__<name>`),
//! - a **description** (verbatim to Claude, shapes when it'll call the tool),
//! - an **input schema** (JSON Schema; Claude fills it in when calling),
//! - a **runtime**: either a server-side Rust handler or a client-side
//!   marker, where the result comes from a React component's response.
//!
//! At spawn time the web server:
//!   1. Serializes the registry's tool specs + a per-spawn secret to a JSON
//!      config file.
//!   2. Points `claude --mcp-config` at a small shim that the template ships
//!      (`tool-bridge` bin). The bridge exposes all tools to Claude via the
//!      MCP stdio protocol and forwards calls back to the web server's
//!      `/__tools/dispatch` endpoint, which looks them up here.
//!   3. Passes `--allowed-tools mcp__template-tools__<name>,…` so Claude
//!      can auto-use the tools without prompting.
//!
//! Client tools aren't runnable until Phase 4 wires the WebSocket
//! round-trip; dispatching them returns a stubbed error for now.

use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// The MCP server name the template uses. Claude references tools as
/// `mcp__<SERVER_NAME>__<tool_name>` in `--allowed-tools` and in its
/// `tool_use` blocks, so this constant has to agree across the bridge,
/// the registry, and the spawn.
pub const MCP_SERVER_NAME: &str = "template-tools";

/// Public description of a tool, safe to hand to Claude or the bridge.
/// `input_schema` is a JSON Schema — forks can hand-write it or generate
/// it with `schemars` if they prefer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub runtime: ToolRuntime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolRuntime {
    /// Rust async handler.
    Server,
    /// Rendered by a React component; result comes from a user action.
    Client,
}

/// Shape of the server-side handler. Handlers are async and return JSON.
pub type ServerToolFn =
    Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value>> + Send + Sync + 'static>;

enum Handler {
    Server(ServerToolFn),
    Client,
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    inner: Arc<ToolRegistryInner>,
}

struct ToolRegistryInner {
    specs: Vec<ToolSpec>,
    handlers: HashMap<String, Handler>,
}

impl Default for ToolRegistryInner {
    fn default() -> Self {
        Self {
            specs: Vec::new(),
            handlers: HashMap::new(),
        }
    }
}

/// Mutable builder — forks use this in `main.rs` before sealing into the
/// shared [`ToolRegistry`].
#[derive(Default)]
pub struct ToolRegistryBuilder {
    specs: Vec<ToolSpec>,
    handlers: HashMap<String, Handler>,
}

impl ToolRegistryBuilder {
    /// Register an async Rust handler. The handler receives the tool
    /// arguments (validated against `input_schema` by Claude, not by us)
    /// and returns JSON that becomes the `tool_result` Claude sees.
    pub fn server_tool<F, Fut>(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        handler: F,
    ) -> &mut Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Value>> + Send + 'static,
    {
        let name = name.into();
        self.specs.push(ToolSpec {
            name: name.clone(),
            description: description.into(),
            input_schema,
            runtime: ToolRuntime::Server,
        });
        let h: ServerToolFn = Arc::new(move |v| Box::pin(handler(v)));
        self.handlers.insert(name, Handler::Server(h));
        self
    }

    /// Register a client-side tool (rendering handled by the frontend
    /// registry — see `src/core/tools/registry.ts`). The result comes
    /// from a WebSocket round-trip in Phase 4.
    pub fn client_tool(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> &mut Self {
        let name = name.into();
        self.specs.push(ToolSpec {
            name: name.clone(),
            description: description.into(),
            input_schema,
            runtime: ToolRuntime::Client,
        });
        self.handlers.insert(name, Handler::Client);
        self
    }

    pub fn build(self) -> ToolRegistry {
        ToolRegistry {
            inner: Arc::new(ToolRegistryInner {
                specs: self.specs,
                handlers: self.handlers,
            }),
        }
    }
}

impl ToolRegistry {
    pub fn builder() -> ToolRegistryBuilder {
        ToolRegistryBuilder::default()
    }

    /// Every registered tool's public spec. Used by the MCP bridge to
    /// answer `tools/list`, and by the web server to compute the
    /// `--allowed-tools` argv.
    pub fn specs(&self) -> &[ToolSpec] {
        &self.inner.specs
    }

    /// The `mcp__template-tools__*` names that belong on the
    /// `--allowed-tools` list so Claude auto-approves them.
    pub fn allowed_tool_names(&self) -> Vec<String> {
        self.inner
            .specs
            .iter()
            .map(|s| format!("mcp__{}__{}", MCP_SERVER_NAME, s.name))
            .collect()
    }

    /// Dispatch a tool call. Returns an error for client tools until
    /// Phase 4 implements the WebSocket round-trip.
    pub async fn dispatch(&self, name: &str, input: Value) -> Result<Value> {
        match self.inner.handlers.get(name) {
            Some(Handler::Server(f)) => f(input).await,
            Some(Handler::Client) => Err(anyhow!(
                "client tool '{}' dispatch not wired until Phase 4",
                name
            )),
            None => Err(anyhow!("unknown tool: {}", name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn server_tool_roundtrip() {
        let mut b = ToolRegistry::builder();
        b.server_tool(
            "echo",
            "Returns the input verbatim.",
            json!({"type": "object"}),
            |v| async move { Ok(v) },
        );
        let r = b.build();
        let out = r.dispatch("echo", json!({"x": 1})).await.unwrap();
        assert_eq!(out, json!({"x": 1}));
    }

    #[tokio::test]
    async fn client_tool_dispatch_errs_until_phase_four() {
        let mut b = ToolRegistry::builder();
        b.client_tool("pick_one", "", json!({}));
        let r = b.build();
        let err = r.dispatch("pick_one", json!({})).await.unwrap_err();
        assert!(err.to_string().contains("Phase 4"), "{err}");
    }

    #[test]
    fn allowed_tool_names_use_mcp_prefix() {
        let mut b = ToolRegistry::builder();
        b.server_tool("one", "", json!({}), |_| async { Ok(Value::Null) });
        b.client_tool("two", "", json!({}));
        let r = b.build();
        let names = r.allowed_tool_names();
        assert_eq!(
            names,
            vec![
                "mcp__template-tools__one".to_string(),
                "mcp__template-tools__two".to_string(),
            ]
        );
    }
}
