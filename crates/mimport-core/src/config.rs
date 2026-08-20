use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub paths: PathsConfig,
    pub musicbrainz: MbConfig,
    pub lidarr: LidarrConfig,
    pub slskd: SlskdConfig,
    #[serde(default)]
    pub quality: QualityConfig,
    #[serde(default)]
    pub scoring: Scoring,
    #[serde(default)]
    pub cover_art: CoverArtConfig,
    #[serde(default)]
    pub yt: YtConfig,
}

#[derive(Debug, Deserialize)]
pub struct PathsConfig {
    pub library: PathBuf,
    pub downloads: PathBuf,
    pub staging: PathBuf,
    pub database: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct MbConfig {
    #[serde(default = "default_mb_base_url")]
    pub base_url: String,
    pub user_agent: String,
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
pub struct CoverArtConfig {
    #[serde(default = "default_cover_art_base_url")]
    pub base_url: String,
    /// Disk cache for fetched front covers; defaults under the OS cache dir.
    #[serde(default)]
    pub cache_dir: Option<PathBuf>,
}

impl Default for CoverArtConfig {
    fn default() -> Self {
        return CoverArtConfig {
            base_url: default_cover_art_base_url(),
            cache_dir: None,
        };
    }
}

#[derive(Debug, Deserialize)]
pub struct YtConfig {
    #[serde(default = "default_yt_dlp_path")]
    pub yt_dlp_path: String,
}

impl Default for YtConfig {
    fn default() -> Self {
        return YtConfig {
            yt_dlp_path: default_yt_dlp_path(),
        };
    }
}

#[derive(Debug, Deserialize)]
pub struct SlskdConfig {
    #[serde(default = "default_slskd_base_url")]
    pub base_url: String,
    pub username: String,
    pub password: String,
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

impl Default for QualityConfig {
    fn default() -> Self {
        return QualityConfig {
            target_samplerate: default_samplerate(),
            target_bitdepth: default_bitdepth(),
        };
    }
}

fn default_mb_base_url() -> String {
    return "https://musicbrainz.org/ws/2".to_string();
}
fn default_lidarr_base_url() -> String {
    return "https://api.lidarr.audio/api/v0.4".to_string();
}
fn default_cover_art_base_url() -> String {
    return "https://coverartarchive.org".to_string();
}
fn default_yt_dlp_path() -> String {
    return "yt-dlp".to_string();
}
fn default_mb_rate_limit() -> f64 {
    return 1.0;
}
fn default_cache_ttl_secs() -> u64 {
    return 2_592_000;
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

#[derive(Debug, Clone, Deserialize)]
pub struct Scoring {
    #[serde(default)]
    pub status: StatusWeights,
    #[serde(default)]
    pub media: MediaWeights,
    #[serde(default)]
    pub release_group: ReleaseGroupWeights,
    #[serde(default)]
    pub bonus: BonusWeights,
    #[serde(default)]
    pub terms: TermWeights,
}

impl Default for Scoring {
    fn default() -> Self {
        return Scoring {
            status: StatusWeights::default(),
            media: MediaWeights::default(),
            release_group: ReleaseGroupWeights::default(),
            bonus: BonusWeights::default(),
            terms: TermWeights::default(),
        };
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusWeights {
    pub official: f64,
    pub other: f64,
}

impl Default for StatusWeights {
    fn default() -> Self {
        return StatusWeights {
            official: 100.0,
            other: -120.0,
        };
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaWeights {
    pub digital: f64,
    pub cd: f64,
    pub other_non_digital: f64,
    pub vinyl: f64,
    pub other_physical: f64,
}

impl Default for MediaWeights {
    fn default() -> Self {
        return MediaWeights {
            digital: 70.0,
            cd: -20.0,
            other_non_digital: -20.0,
            vinyl: -100.0,
            other_physical: -95.0,
        };
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TermWeights {
    pub strong_cost: f64,
    pub medium_cost: f64,
    pub weak_cost: f64,
    pub cap: f64,
    #[serde(default = "default_strong_terms")]
    pub strong: Vec<String>,
    #[serde(default = "default_medium_terms")]
    pub medium: Vec<String>,
    #[serde(default = "default_weak_terms")]
    pub weak: Vec<String>,
}

impl Default for TermWeights {
    fn default() -> Self {
        return TermWeights {
            strong_cost: 45.0,
            medium_cost: 25.0,
            weak_cost: 10.0,
            cap: 180.0,
            strong: default_strong_terms(),
            medium: default_medium_terms(),
            weak: default_weak_terms(),
        };
    }
}

fn scoring_terms(list: &[&str]) -> Vec<String> {
    return list.iter().map(|s| return (*s).to_string()).collect();
}

fn default_strong_terms() -> Vec<String> {
    return scoring_terms(&[
        "live",
        "remix",
        "karaoke",
        "commentary",
        "interview",
        "rehearsal",
    ]);
}

fn default_medium_terms() -> Vec<String> {
    return scoring_terms(&[
        "demo",
        "instrumental",
        "acoustic",
        "alternate take",
        "alt take",
        "session",
    ]);
}

fn default_weak_terms() -> Vec<String> {
    return scoring_terms(&["radio edit", "edit", "version"]);
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseGroupWeights {
    pub primary_album: f64,
    pub primary_ep: f64,
    pub primary_single: f64,
    pub secondary_penalty: f64,
    #[serde(default = "default_secondary_types")]
    pub penalised_secondary_types: Vec<String>,
}

impl Default for ReleaseGroupWeights {
    fn default() -> Self {
        return ReleaseGroupWeights {
            primary_album: 30.0,
            primary_ep: 15.0,
            primary_single: 0.0,
            secondary_penalty: -60.0,
            penalised_secondary_types: default_secondary_types(),
        };
    }
}

fn default_secondary_types() -> Vec<String> {
    return scoring_terms(&[
        "compilation",
        "live",
        "remix",
        "demo",
        "interview",
        "spokenword",
    ]);
}

#[derive(Debug, Clone, Deserialize)]
pub struct BonusWeights {
    pub canonical_track_count: f64,
    pub label_present: f64,
    #[serde(default = "default_edition_bonus")]
    pub edition_bonus: f64,
    /// Max tracklist term-penalty a deluxe/expanded edition may carry and still
    /// earn `edition_bonus`; above this it's treated as filler-heavy.
    #[serde(default = "default_edition_filler_cap")]
    pub edition_filler_cap: f64,
}

impl Default for BonusWeights {
    fn default() -> Self {
        return BonusWeights {
            canonical_track_count: 10.0,
            label_present: 8.0,
            edition_bonus: default_edition_bonus(),
            edition_filler_cap: default_edition_filler_cap(),
        };
    }
}

fn default_edition_bonus() -> f64 {
    return 15.0;
}

fn default_edition_filler_cap() -> f64 {
    return 25.0;
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| return Error::io(path, e))?;
        let cfg: Config = toml::from_str(&text).map_err(|e| return Error::Config(e.to_string()))?;
        if is_unset(&cfg.musicbrainz.user_agent) {
            return Err(Error::MbUserAgentUnset {
                value: cfg.musicbrainz.user_agent,
            });
        }
        for (field, value) in [
            ("username", &cfg.slskd.username),
            ("password", &cfg.slskd.password),
        ] {
            if is_unset(value) {
                return Err(Error::SlskdCredsUnset {
                    field,
                    value: value.clone(),
                });
            }
        }
        return Ok(cfg);
    }
}

const UNSET_SENTINEL: &str = "CHANGE_ME";

fn is_unset(value: &str) -> bool {
    let trimmed = value.trim();
    return trimmed.is_empty() || trimmed == UNSET_SENTINEL;
}
