//! `api.lidarr.audio` proxy client. Confirmed live (2026-08-10): no UA requirement, no
//! auth, `X-RateLimit-*` headers are noise (non-monotonic, split across canary/internal
//! backend pools — do not drive pacing off them). Real failure mode is a bare `HTTP 500`
//! once concurrency exceeds ~10-15 parallel requests, with no `Retry-After`. This client
//! stays strictly serial (a mutex-guarded pace gate) and retries 500/502/503/504.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

use crate::cache::DiskCache;
use crate::config::LidarrConfig;
use crate::error::{Error, Result};

pub struct LidarrClient {
    http: reqwest::blocking::Client,
    base_url: String,
    cache: DiskCache,
    max_retries: u32,
    pace: Mutex<Instant>,
}

/// Self-imposed courtesy pacing — not derived from any header (they're unreliable), just
/// a conservative fixed floor so mimport never contributes to the concurrency that
/// triggers bare 500s.
const MIN_INTERVAL: Duration = Duration::from_millis(350);

impl LidarrClient {
    pub fn new(cfg: &LidarrConfig) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;
        return Ok(LidarrClient {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            cache: DiskCache::new(cfg.cache_dir.clone(), cfg.cache_ttl_secs),
            max_retries: cfg.max_retries,
            pace: Mutex::new(Instant::now() - MIN_INTERVAL),
        });
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str, extra_query: &[(&str, &str)]) -> Result<T> {
        let mut url = format!("{}{}", self.base_url, path);
        if !extra_query.is_empty() {
            url.push('?');
            let parts: Vec<String> = extra_query
                .iter()
                .map(|(k, v)| return format!("{k}={}", urlencode(v)))
                .collect();
            url.push_str(&parts.join("&"));
        }

        if let Some(cached) = self.cache.get(&url) {
            return Ok(serde_json::from_str(&cached)?);
        }

        let body = self.fetch_with_retry(&url)?;
        self.cache.put(&url, &body)?;
        return Ok(serde_json::from_str(&body)?);
    }

    fn wait_pace(&self) {
        let mut last = self.pace.lock().expect("pace mutex poisoned");
        let elapsed = last.elapsed();
        if elapsed < MIN_INTERVAL {
            std::thread::sleep(MIN_INTERVAL - elapsed);
        }
        *last = Instant::now();
    }

    fn fetch_with_retry(&self, url: &str) -> Result<String> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            self.wait_pace();

            let resp = self.http.get(url).send()?;
            let status = resp.status();
            if status.is_success() {
                return Ok(resp.text()?);
            }

            // 500 is this backend's actual overload signal (confirmed live under
            // concurrency), not a client bug — treat it as retryable alongside the
            // standard gateway codes.
            let retryable = matches!(status.as_u16(), 500 | 502 | 503 | 504);
            if !retryable || attempt > self.max_retries {
                let body = resp.text().unwrap_or_default();
                if status.as_u16() == 404 {
                    return Err(Error::Lidarr {
                        what: "lookup",
                        status: 404,
                        body,
                    });
                }
                if retryable {
                    return Err(Error::LidarrUnavailable { attempts: attempt });
                }
                return Err(Error::Lidarr {
                    what: "request",
                    status: status.as_u16(),
                    body,
                });
            }

            let backoff = Duration::from_millis(500 * 2u64.pow(attempt.min(6)));
            std::thread::sleep(backoff);
        }
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    return out;
}
