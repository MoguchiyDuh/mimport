//! Direct MusicBrainz HTTP client.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

use crate::cache::DiskCache;
use crate::config::MbConfig;
use crate::error::{Error, Result};

const MAX_RETRY_AFTER_SECS: u64 = 60;

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

    /// GET, cached + rate-limited, retried on 429/502/503/504.
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

        let body = self.fetch_with_retry(path, &url)?;
        self.cache.put(&url, &body)?;
        return Ok(serde_json::from_str(&body)?);
    }

    fn fetch_with_retry(&self, path: &str, url: &str) -> Result<String> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            self.limiter.wait();

            let resp = match self
                .http
                .get(url)
                .header(reqwest::header::USER_AGENT, &self.user_agent)
                .send()
            {
                Ok(resp) => resp,
                Err(e) => {
                    if attempt <= self.max_retries {
                        std::thread::sleep(backoff(attempt));
                        continue;
                    }
                    return Err(Error::Http(e));
                }
            };

            let status = resp.status();
            if status.is_success() {
                return match resp.text() {
                    Ok(body) => Ok(body),
                    Err(e) => {
                        if attempt <= self.max_retries {
                            std::thread::sleep(backoff(attempt));
                            continue;
                        }
                        Err(Error::Http(e))
                    }
                };
            }

            let retryable = matches!(status.as_u16(), 429 | 502 | 503 | 504);
            if !retryable || attempt > self.max_retries {
                let body = resp.text().unwrap_or_default();
                if status.as_u16() == 404 {
                    return Err(not_found(path));
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

            let sleep_for = retry_after(&resp).unwrap_or_else(|| return backoff(attempt));
            std::thread::sleep(sleep_for);
        }
    }
}

fn backoff(attempt: u32) -> Duration {
    return Duration::from_millis(500 * 2u64.pow(attempt.min(6)));
}

fn retry_after(resp: &reqwest::blocking::Response) -> Option<Duration> {
    let secs: u64 = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    return Some(Duration::from_secs(secs.min(MAX_RETRY_AFTER_SECS)));
}

fn not_found(path: &str) -> Error {
    let mut segments = path.trim_start_matches('/').split('/');
    let entity: &'static str = match segments.next().unwrap_or("") {
        "artist" => "artist",
        "release" => "release",
        "release-group" => "release-group",
        "recording" => "recording",
        _ => "entity",
    };
    let mbid = segments.next().unwrap_or("").to_string();
    return Error::MbNotFound { entity, mbid };
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
