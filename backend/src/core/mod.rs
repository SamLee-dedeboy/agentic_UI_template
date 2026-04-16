//! Reusable core plumbing shared by every fork of this template.
//!
//! Contents here are intentionally feature-agnostic: stream parsing, process
//! registry wrappers, auth middleware, and the binary-discovery helpers.
//! Opinionated features (usage aggregation, MCP UI state, etc.) live under
//! `features/` and are opt-in.

pub mod auth;
pub mod conversations;
pub mod cookies;
pub mod ratelimit;
pub mod stream;
pub mod tools;
