//! `api.lidarr.audio` proxy client. Serial pacing; retries 500/502/503/504.

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

/// Fixed pacing floor; the backend 500s under concurrency.
const MIN_INTERVAL: Duration = Duration::from_millis(350);
const USER_AGENT: &str = "mimport";
const MAX_RETRY_AFTER_SECS: u64 = 60;

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

        let body = self.fetch_with_retry(path, &url)?;
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

    fn fetch_with_retry(&self, path: &str, url: &str) -> Result<String> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            self.wait_pace();

            let resp = match self
                .http
                .get(url)
                .header(reqwest::header::USER_AGENT, USER_AGENT)
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

            // bare 500 is this backend's overload signal — retryable
            let retryable = matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504);
            if !retryable || attempt > self.max_retries {
                let body = resp.text().unwrap_or_default();
                if status.as_u16() == 404 {
                    return Err(not_found(path));
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
        "album" => "album",
        _ => "entity",
    };
    let mbid = segments.next().unwrap_or("").to_string();
    return Error::LidarrNotFound { entity, mbid };
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
