//! `config.toml` schema. Fresh design, not carried over from the VPS reference build —
//! per-backend sections reflect the mb/lidarr split decided in DESIGN.md §5.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub paths: PathsConfig,
    pub musicbrainz: MbConfig,
    pub lidarr: LidarrConfig,
    pub slskd: SlskdConfig,
    pub quality: QualityConfig,
}

#[derive(Debug, Deserialize)]
pub struct PathsConfig {
    /// The Navidrome-visible library tree. `import` copies into here.
    pub library: PathBuf,
    /// slskd's download landing dir.
    pub downloads: PathBuf,
    /// Working area for postfix/import between download and library placement.
    pub staging: PathBuf,
    /// SQLite jobs/library-index DB.
    pub database: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct MbConfig {
    #[serde(default = "default_mb_base_url")]
    pub base_url: String,
    /// MB requires a meaningful UA or returns 503 — confirmed live.
    pub user_agent: String,
    /// MB's own X-RateLimit-* headers are real/decrementing (confirmed live, ~1200/hr
    /// default), unlike the lidarr proxy's. Self-paced floor as a courtesy/backstop.
    #[serde(default = "default_mb_rate_limit")]
    pub rate_limit_per_sec: f64,
    pub cache_dir: PathBuf,
    #[serde(default = "default_cache_ttl_secs")]
    pub cache_ttl_secs: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

#[derive(Debug, Deserialize)]
pub struct LidarrConfig {
    #[serde(default = "default_lidarr_base_url")]
    pub base_url: String,
    pub cache_dir: PathBuf,
    #[serde(default = "default_cache_ttl_secs")]
    pub cache_ttl_secs: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

#[derive(Debug, Deserialize)]
pub struct SlskdConfig {
    /// Loopback — mimport is deployed onto the same host as the slskd container
    /// (confirmed live 2026-08-14: JWT-via-login auth, no API key on this instance).
    #[serde(default = "default_slskd_base_url")]
    pub base_url: String,
    pub username: String,
    pub password: String,
    /// `search` deliberately has no timeout knob — it never sends `searchTimeout` to
    /// the API, so slskd's own server-side default applies untouched.
    ///
    /// `fetch` is the one command that owns a timeout: how long mimport waits for an
    /// enqueued transfer to finish before giving up (the transfer itself keeps running
    /// on slskd regardless). `timeout = fetch_timeout_base_secs + (size_mb *
    /// fetch_timeout_per_mb_secs)`.
    #[serde(default = "default_fetch_timeout_base_secs")]
    pub fetch_timeout_base_secs: u64,
    #[serde(default = "default_fetch_timeout_per_mb_secs")]
    pub fetch_timeout_per_mb_secs: f64,
    #[serde(default = "default_slskd_request_timeout")]
    pub request_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct QualityConfig {
    #[serde(default = "default_samplerate")]
    pub target_samplerate: u32,
    #[serde(default = "default_bitdepth")]
    pub target_bitdepth: u16,
}

fn default_mb_base_url() -> String {
    return "https://musicbrainz.org/ws/2".to_string();
}
fn default_lidarr_base_url() -> String {
    return "https://api.lidarr.audio/api/v0.4".to_string();
}
fn default_mb_rate_limit() -> f64 {
    return 1.0;
}
fn default_cache_ttl_secs() -> u64 {
    return 2_592_000; // 30 days
}
fn default_max_retries() -> u32 {
    return 5;
}
fn default_slskd_base_url() -> String {
    return "http://127.0.0.1:8111".to_string();
}
fn default_fetch_timeout_base_secs() -> u64 {
    return 30;
}
fn default_fetch_timeout_per_mb_secs() -> f64 {
    return 1.0;
}
fn default_slskd_request_timeout() -> u64 {
    return 30;
}
fn default_samplerate() -> u32 {
    return 44_100;
}
fn default_bitdepth() -> u16 {
    return 16;
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| return Error::io(path, e))?;
        let cfg: Config = toml::from_str(&text).map_err(|e| return Error::Config(e.to_string()))?;
        if cfg.musicbrainz.user_agent.trim().is_empty()
            || cfg.musicbrainz.user_agent.contains("CHANGE_ME")
        {
            return Err(Error::MbUserAgentUnset {
                value: cfg.musicbrainz.user_agent,
            });
        }
        return Ok(cfg);
    }
}
