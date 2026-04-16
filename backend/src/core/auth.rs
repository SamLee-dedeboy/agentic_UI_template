//! Shared-secret auth for the template's web server.
//!
//! Two layers:
//!  - [`AuthConfig`]: resolved once at startup from the `APP_AUTH_TOKEN` env
//!    var + the chosen bind host. When the server binds a non-loopback
//!    address, a token is *required* — the server refuses to start without
//!    one to prevent accidental LAN exposure of `--dangerously-skip-permissions`.
//!  - [`require_auth`] Axum middleware: when a token is configured, every
//!    REST request must carry `Authorization: Bearer <token>`. WebSocket
//!    upgrades must include `?token=<token>` in the URL.
//!
//! Forks with richer needs (OAuth, per-user accounts) should replace this
//! module wholesale — it's intentionally just enough for a small team.

use std::net::IpAddr;

use axum::{
    body::Body,
    extract::{Query, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};
use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct AuthConfig {
    /// `None` means "open access" — only allowed on loopback binds.
    pub token: Option<String>,
}

impl AuthConfig {
    /// Resolve from environment. Returns `Err` when the combination of host
    /// and env var is unsafe (non-loopback without a token).
    pub fn resolve(host: &IpAddr) -> Result<Self, String> {
        let token = std::env::var("APP_AUTH_TOKEN")
            .ok()
            .filter(|t| !t.trim().is_empty());

        if token.is_none() && !is_loopback(host) {
            return Err(format!(
                "refusing to bind {} without APP_AUTH_TOKEN set. \
                 Set the env var to a strong secret, or bind 127.0.0.1 instead.",
                host
            ));
        }
        Ok(Self { token })
    }

    pub fn is_open(&self) -> bool {
        self.token.is_none()
    }
}

fn is_loopback(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

#[derive(Deserialize)]
pub struct TokenQuery {
    pub token: Option<String>,
}

/// Axum middleware: enforces `Authorization: Bearer <token>` on REST routes.
/// WebSocket routes accept the token via `?token=...` (their upgrade handlers
/// call [`check_ws_token`] directly since the middleware runs before the
/// upgrade).
pub async fn require_auth(
    State(cfg): State<AuthConfig>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(expected) = cfg.token.as_deref() else {
        return Ok(next.run(req).await);
    };

    // WebSocket upgrade requests are authenticated via query string, not
    // header (browsers can't set Authorization on `new WebSocket(...)`).
    let path = req.uri().path();
    if path.starts_with("/ws/") {
        let qs = req.uri().query().unwrap_or("");
        if query_token(qs).as_deref() == Some(expected) {
            return Ok(next.run(req).await);
        }
        return Err(StatusCode::UNAUTHORIZED);
    }

    let header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if header.strip_prefix("Bearer ") == Some(expected) {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Used by WebSocket handlers that receive the `?token=...` as an Axum
/// `Query` extractor. Returns `Ok(())` when the token matches (or auth is
/// open); `Err(StatusCode::UNAUTHORIZED)` otherwise.
pub fn check_ws_token(cfg: &AuthConfig, q: &Query<TokenQuery>) -> Result<(), StatusCode> {
    match cfg.token.as_deref() {
        None => Ok(()),
        Some(expected) if q.token.as_deref() == Some(expected) => Ok(()),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

fn query_token(qs: &str) -> Option<String> {
    for pair in qs.split('&') {
        if let Some(rest) = pair.strip_prefix("token=") {
            return Some(urldecode(rest));
        }
    }
    None
}

fn urldecode(s: &str) -> String {
    // Tiny %XX decoder — sufficient for our needs and avoids pulling
    // `percent-encoding` into the template.
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn open_allowed_on_loopback() {
        std::env::remove_var("APP_AUTH_TOKEN");
        let cfg = AuthConfig::resolve(&IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        assert!(cfg.is_open());
    }

    #[test]
    fn open_refused_on_wildcard() {
        std::env::remove_var("APP_AUTH_TOKEN");
        let err = AuthConfig::resolve(&IpAddr::V4(Ipv4Addr::UNSPECIFIED)).unwrap_err();
        assert!(err.contains("APP_AUTH_TOKEN"), "{err}");
    }

    #[test]
    fn token_allowed_on_wildcard() {
        std::env::set_var("APP_AUTH_TOKEN", "secret");
        let cfg = AuthConfig::resolve(&IpAddr::V4(Ipv4Addr::UNSPECIFIED)).unwrap();
        assert_eq!(cfg.token.as_deref(), Some("secret"));
        std::env::remove_var("APP_AUTH_TOKEN");
    }

    #[test]
    fn query_token_extracts() {
        assert_eq!(query_token("foo=1&token=abc&bar=2").as_deref(), Some("abc"));
        assert_eq!(query_token("token=hello%20world").as_deref(), Some("hello world"));
        assert_eq!(query_token("nope=1"), None);
    }
}
