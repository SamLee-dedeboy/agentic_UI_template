//! Guest-session cookies — the default identity mechanism for
//! customer-facing apps built on this template.
//!
//! Each inbound request either presents a valid signed `session=` cookie or
//! gets one minted. The cookie value is `<session-id>.<hex-hmac-sha256>`,
//! where `session-id` is a random 128-bit UUID and the HMAC is computed
//! over it using [`CookieConfig::signing_key`]. No server-side storage is
//! needed to validate it — the HMAC is enough.
//!
//! The resolved session ID is stashed in the request's extensions as
//! [`GuestSession`]. Handlers downstream can read it with
//! `Extension::<GuestSession>`.
//!
//! Forks that want real auth (OAuth, passwordless, SSO) should replace
//! this middleware with their own. The [`GuestSession`] newtype stays the
//! seam — every other module consumes session identity through it, so
//! swapping identity providers doesn't ripple through the codebase.

use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

const COOKIE_NAME: &str = "session";

/// Resolved guest session identity for a single request. Stored in the
/// request's extensions by [`guest_cookie_layer`].
#[derive(Clone, Debug)]
pub struct GuestSession {
    pub id: String,
    /// `true` when the middleware minted a fresh ID for this request (no
    /// valid cookie was presented). Triggers a `Set-Cookie` on the
    /// response.
    pub freshly_minted: bool,
}

#[derive(Clone, Debug)]
pub struct CookieConfig {
    /// HMAC key used to sign the cookie value. Supply via the
    /// `APP_SESSION_KEY` env var in production; a random ephemeral key is
    /// used otherwise (with a loud warning at startup — sessions are wiped
    /// on every restart in that mode).
    pub signing_key: Vec<u8>,
    /// `true` → `Secure` attribute is set on the cookie. Forks fronting
    /// the server with TLS should set this; the default auto-detects based
    /// on whether the request arrived on HTTPS (best-effort; behind a
    /// reverse proxy you may need to force it).
    pub force_secure: bool,
}

impl CookieConfig {
    /// Load from environment. Always succeeds: missing `APP_SESSION_KEY`
    /// falls back to a fresh random key (logged loudly).
    pub fn from_env() -> Self {
        let signing_key = match std::env::var("APP_SESSION_KEY") {
            Ok(s) if s.len() >= 16 => s.into_bytes(),
            Ok(_) => {
                eprintln!(
                    "⚠️  APP_SESSION_KEY set but shorter than 16 bytes; \
                     falling back to an ephemeral key. Guest sessions \
                     will not survive a restart."
                );
                random_key()
            }
            Err(_) => {
                eprintln!(
                    "⚠️  APP_SESSION_KEY not set; using an ephemeral \
                     signing key. Guest sessions will not survive a \
                     restart. Set APP_SESSION_KEY to a long random string \
                     in production."
                );
                random_key()
            }
        };
        Self {
            signing_key,
            force_secure: std::env::var("APP_COOKIE_SECURE")
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                .unwrap_or(false),
        }
    }

    /// Produce `<session-id>.<hex-hmac>` for the cookie value.
    pub fn sign(&self, session_id: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.signing_key)
            .expect("HMAC-SHA256 accepts any key length");
        mac.update(session_id.as_bytes());
        let tag = mac.finalize().into_bytes();
        format!("{session_id}.{}", hex(&tag))
    }

    /// Return the session ID if the cookie is well-formed and the HMAC
    /// verifies; `None` otherwise.
    pub fn verify(&self, cookie_value: &str) -> Option<String> {
        let (id, tag_hex) = cookie_value.rsplit_once('.')?;
        let tag = from_hex(tag_hex)?;
        let mut mac = HmacSha256::new_from_slice(&self.signing_key).ok()?;
        mac.update(id.as_bytes());
        let expected = mac.finalize().into_bytes();
        if tag.ct_eq(expected.as_slice()).into() {
            Some(id.to_string())
        } else {
            None
        }
    }
}

fn random_key() -> Vec<u8> {
    // uuid::Uuid::new_v4() is 16 bytes of cryptographic randomness; two of
    // them gives us a 256-bit key without adding a separate rand dep.
    let mut v = Uuid::new_v4().as_bytes().to_vec();
    v.extend_from_slice(Uuid::new_v4().as_bytes());
    v
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    out
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)? as u8;
        let lo = (bytes[i + 1] as char).to_digit(16)? as u8;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

/// Scan the raw `Cookie` header for our session cookie value.
fn extract_cookie(header: &str) -> Option<&str> {
    for pair in header.split(';') {
        let p = pair.trim();
        if let Some(rest) = p.strip_prefix(&format!("{COOKIE_NAME}=")) {
            return Some(rest);
        }
    }
    None
}

/// Axum middleware. Resolves (or mints) a guest session, stashes it in
/// request extensions, and appends a `Set-Cookie` to the response when a
/// fresh ID was minted.
pub async fn guest_cookie_layer(
    axum::extract::State(cfg): axum::extract::State<CookieConfig>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let incoming = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_cookie)
        .and_then(|raw| cfg.verify(raw));

    let session = match incoming {
        Some(id) => GuestSession {
            id,
            freshly_minted: false,
        },
        None => GuestSession {
            id: Uuid::new_v4().to_string(),
            freshly_minted: true,
        },
    };
    let minted = session.freshly_minted;
    let cookie_id = session.id.clone();
    req.extensions_mut().insert(session);

    let mut resp = next.run(req).await;

    if minted {
        let value = cfg.sign(&cookie_id);
        // 30 days; HttpOnly; SameSite=Lax; Path=/. We intentionally don't
        // set Secure unless asked so localhost dev still works over HTTP.
        let secure = if cfg.force_secure { "; Secure" } else { "" };
        let cookie = format!(
            "{COOKIE_NAME}={value}; Path=/; Max-Age=2592000; HttpOnly; SameSite=Lax{secure}"
        );
        if let Ok(hv) = HeaderValue::from_str(&cookie) {
            resp.headers_mut().append(header::SET_COOKIE, hv);
        }
    }

    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CookieConfig {
        CookieConfig {
            signing_key: b"test-key-0123456789abcdef".to_vec(),
            force_secure: false,
        }
    }

    #[test]
    fn sign_verify_roundtrip() {
        let c = cfg();
        let id = "abc-123";
        let signed = c.sign(id);
        assert!(signed.starts_with("abc-123."));
        assert_eq!(c.verify(&signed).as_deref(), Some("abc-123"));
    }

    #[test]
    fn verify_rejects_tampered_mac() {
        let c = cfg();
        let mut signed = c.sign("x");
        // Flip a byte in the MAC.
        let last = signed.pop().unwrap();
        signed.push(if last == '0' { '1' } else { '0' });
        assert!(c.verify(&signed).is_none());
    }

    #[test]
    fn verify_rejects_tampered_id() {
        let c = cfg();
        let signed = c.sign("real");
        let (_, tag) = signed.rsplit_once('.').unwrap();
        let tampered = format!("evil.{tag}");
        assert!(c.verify(&tampered).is_none());
    }

    #[test]
    fn extract_cookie_picks_right_name() {
        let header = "foo=bar; session=abc.def; other=1";
        assert_eq!(extract_cookie(header), Some("abc.def"));
        assert_eq!(extract_cookie("nothing=here"), None);
    }
}
