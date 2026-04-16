//! Reusable core plumbing shared by every fork of this template.
//!
//! - `cookies`: HMAC-signed guest-session cookie middleware.
//! - `conversations`: SQLite-backed conversation + message store.
//! - `ratelimit`: per-cookie messages-per-minute and concurrent-session budgets.
//! - `tools`: the `ToolRegistry` builder and server/client runtime enum.

pub mod conversations;
pub mod cookies;
pub mod ratelimit;
pub mod tools;
