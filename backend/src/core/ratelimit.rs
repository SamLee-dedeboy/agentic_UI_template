//! Per-cookie in-memory rate limiting.
//!
//! Two budgets, both configurable via env var:
//!
//! - **Messages per minute** (`APP_MAX_MSGS_PER_MIN`, default 30) — every
//!   user prompt submitted over WebSocket counts. Exceeded requests are
//!   rejected with a `rate_limited` event and the Claude spawn skipped.
//! - **Concurrent conversations** (`APP_MAX_CONCURRENT_CONVS`, default 5)
//!   — how many Claude subprocesses one guest can have running
//!   simultaneously.
//!
//! Both live in-process; a multi-instance deployment needs external state
//! (Redis, etc). For a single-binary self-host this is enough.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const DEFAULT_MSGS_PER_MIN: usize = 30;
const DEFAULT_CONCURRENT_CONVS: usize = 5;
const WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<Inner>>,
    pub max_msgs_per_min: usize,
    pub max_concurrent: usize,
}

struct Inner {
    /// Sliding-window timestamps per cookie. Oldest entries get evicted on
    /// access; the vec length is the message count for the last `WINDOW`.
    msg_times: HashMap<String, VecDeque<Instant>>,
    /// Active (not-yet-finished) conversation count per cookie.
    active_counts: HashMap<String, usize>,
}

impl RateLimiter {
    pub fn from_env() -> Self {
        let max_msgs_per_min = std::env::var("APP_MAX_MSGS_PER_MIN")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MSGS_PER_MIN);
        let max_concurrent = std::env::var("APP_MAX_CONCURRENT_CONVS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_CONCURRENT_CONVS);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                msg_times: HashMap::new(),
                active_counts: HashMap::new(),
            })),
            max_msgs_per_min,
            max_concurrent,
        }
    }

    /// Record one message from `cookie`. Returns `Err` with a human message
    /// when the budget is exhausted.
    pub async fn try_record_message(&self, cookie: &str) -> Result<(), String> {
        let now = Instant::now();
        let mut inner = self.inner.lock().await;
        let q = inner.msg_times.entry(cookie.to_string()).or_default();
        while let Some(front) = q.front() {
            if now.duration_since(*front) > WINDOW {
                q.pop_front();
            } else {
                break;
            }
        }
        if q.len() >= self.max_msgs_per_min {
            return Err(format!(
                "rate limited: max {} messages per minute",
                self.max_msgs_per_min
            ));
        }
        q.push_back(now);
        Ok(())
    }

    /// Claim a concurrent-conversation slot. Returns `Err` when the cap is
    /// already hit. Caller must invoke [`release_conversation`] on exit
    /// (success or failure) to free the slot.
    pub async fn try_claim_conversation(&self, cookie: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        let count = inner.active_counts.entry(cookie.to_string()).or_insert(0);
        if *count >= self.max_concurrent {
            return Err(format!(
                "rate limited: max {} concurrent conversations",
                self.max_concurrent
            ));
        }
        *count += 1;
        Ok(())
    }

    pub async fn release_conversation(&self, cookie: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(count) = inner.active_counts.get_mut(cookie) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                inner.active_counts.remove(cookie);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(msgs: usize, conc: usize) -> RateLimiter {
        RateLimiter {
            inner: Arc::new(Mutex::new(Inner {
                msg_times: HashMap::new(),
                active_counts: HashMap::new(),
            })),
            max_msgs_per_min: msgs,
            max_concurrent: conc,
        }
    }

    #[tokio::test]
    async fn msg_budget_blocks_after_limit() {
        let r = limiter(3, 10);
        for _ in 0..3 {
            r.try_record_message("c").await.unwrap();
        }
        assert!(r.try_record_message("c").await.is_err());
    }

    #[tokio::test]
    async fn concurrent_budget_tracks_claims_and_releases() {
        let r = limiter(100, 2);
        r.try_claim_conversation("c").await.unwrap();
        r.try_claim_conversation("c").await.unwrap();
        assert!(r.try_claim_conversation("c").await.is_err());
        r.release_conversation("c").await;
        assert!(r.try_claim_conversation("c").await.is_ok());
    }

    #[tokio::test]
    async fn separate_cookies_separate_budgets() {
        let r = limiter(1, 1);
        r.try_record_message("a").await.unwrap();
        r.try_record_message("b").await.unwrap();
        assert!(r.try_record_message("a").await.is_err());
        assert!(r.try_record_message("b").await.is_err());
    }
}
