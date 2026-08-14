//! Direct MusicBrainz HTTP client. Confirmed live (2026-08-10): UA is mandatory (503
//! without one), `X-RateLimit-*` is real and decrementing (unlike the lidarr proxy) —
//! self-paced limiter here is a courtesy floor, not a workaround for a broken counter.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

use crate::cache::DiskCache;
use crate::config::MbConfig;
use crate::error::{Error, Result};

pub struct MbClient {
    http: reqwest::blocking::Client,
    base_url: String,
    user_agent: String,
    cache: DiskCache,
    max_retries: u32,
    limiter: Limiter,
}

struct Limiter {
    min_interval: Duration,
    last: Mutex<Instant>,
}

impl Limiter {
    fn new(rate_per_sec: f64) -> Self {
        let min_interval = Duration::from_secs_f64(1.0 / rate_per_sec.max(0.01));
        return Limiter {
            min_interval,
            last: Mutex::new(Instant::now() - min_interval),
        };
    }

    fn wait(&self) {
        let mut last = self.last.lock().expect("limiter mutex poisoned");
        let elapsed = last.elapsed();
        if elapsed < self.min_interval {
            std::thread::sleep(self.min_interval - elapsed);
        }
        *last = Instant::now();
    }
}

impl MbClient {
    pub fn new(cfg: &MbConfig) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        return Ok(MbClient {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            user_agent: cfg.user_agent.clone(),
            cache: DiskCache::new(cfg.cache_dir.clone(), cfg.cache_ttl_secs),
            max_retries: cfg.max_retries,
            limiter: Limiter::new(cfg.rate_limit_per_sec),
        });
    }

    /// GET `{base_url}{path}?fmt=json&{extra_query}`, cached, rate-limited, retried on
    /// 429/502/503/504 with exponential backoff.
    pub fn get<T: DeserializeOwned>(&self, path: &str, extra_query: &[(&str, &str)]) -> Result<T> {
        let mut url = format!("{}{}?fmt=json", self.base_url, path);
        for (k, v) in extra_query {
            url.push('&');
            url.push_str(k);
            url.push('=');
            url.push_str(&urlencode(v));
        }

        if let Some(cached) = self.cache.get(&url) {
            return Ok(serde_json::from_str(&cached)?);
        }

        let body = self.fetch_with_retry(&url)?;
        self.cache.put(&url, &body)?;
        return Ok(serde_json::from_str(&body)?);
    }

    fn fetch_with_retry(&self, url: &str) -> Result<String> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            self.limiter.wait();

            let resp = self
                .http
                .get(url)
                .header(reqwest::header::USER_AGENT, &self.user_agent)
                .send()?;

            let status = resp.status();
            if status.is_success() {
                return Ok(resp.text()?);
            }

            let retryable = matches!(status.as_u16(), 429 | 502 | 503 | 504);
            if !retryable || attempt > self.max_retries {
                let body = resp.text().unwrap_or_default();
                if status.as_u16() == 404 {
                    return Err(Error::Mb {
                        what: "lookup",
                        status: 404,
                        body,
                    });
                }
                if retryable {
                    return Err(Error::MbRateLimited { attempts: attempt });
                }
                return Err(Error::Mb {
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
