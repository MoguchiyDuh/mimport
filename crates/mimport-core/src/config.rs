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
    #[serde(default)]
    pub scoring: Scoring,
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

/// §7 edition-scoring weights. Ported from a prior build's `[scoring]` config (weights
/// hand-tuned against a real 405-album library), with everything the two backends don't
/// expose identically dropped rather than faked: no country/locale ladder (the Lidarr
/// proxy returns area *names*, not ISO codes, and has no artist-country field at all),
/// no `artist_home` bonus (same reason), no barcode/cover-art bonus (the proxy's release
/// object has neither field, only direct MB does). What's left works identically on
/// both backends: status, media format, canonical track count, title/tracklist term
/// penalties, deluxe-edition gating, release-group primary/secondary type, and a label
/// bonus (both backends carry a label name).
#[derive(Debug, Clone, Deserialize)]
pub struct Scoring {
    #[serde(default)]
    pub status: StatusWeights,
    #[serde(default)]
    pub media: MediaWeights,
    #[serde(default)]
    pub edition: EditionWeights,
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
            edition: EditionWeights::default(),
            release_group: ReleaseGroupWeights::default(),
            bonus: BonusWeights::default(),
            terms: TermWeights::default(),
        };
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusWeights {
    pub official: f64,
    /// Asymmetric on purpose: −120 puts a non-official release out of reach of any
    /// Official one rather than merely behind it, without hard-rejecting it outright —
    /// an album whose only releases are bootlegs must stay selectable.
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

/// A release with no stated format at all is not penalised — absence isn't evidence of
/// physical media.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaWeights {
    pub digital: f64,
    pub cd: f64,
    /// Non-digital but not a recognised physical format: hybrid SACD, DVD, unknown.
    pub other_non_digital: f64,
    pub vinyl: f64,
    /// Cassette, shellac, 8-track, wax cylinder.
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
pub struct EditionWeights {
    /// Modest by design: a deluxe edition is preferable, but not at the cost of taking
    /// a padded one — the bonus only applies while the tracklist term-penalty stays at
    /// or under `deluxe_gate`.
    pub deluxe_bonus: f64,
    pub deluxe_gate: f64,
    #[serde(default = "default_edition_terms")]
    pub terms: Vec<String>,
}

impl Default for EditionWeights {
    fn default() -> Self {
        return EditionWeights {
            deluxe_bonus: 15.0,
            deluxe_gate: 25.0,
            terms: default_edition_terms(),
        };
    }
}

fn default_edition_terms() -> Vec<String> {
    return ["deluxe", "expanded", "bonus"]
        .iter()
        .map(|s| return (*s).to_string())
        .collect();
}

/// Tiered title terms, applied to the release title and to every track title. Tiering
/// matters: a legitimate title containing "Version" costs 10, not 45.
#[derive(Debug, Clone, Deserialize)]
pub struct TermWeights {
    pub strong_cost: f64,
    pub medium_cost: f64,
    pub weak_cost: f64,
    /// Applies to the summed **tracklist** penalty only — otherwise a long live album
    /// accumulates an unbounded penalty and distorts every comparison. The title
    /// penalty is deliberately uncapped.
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
    /// Charged once per matching secondary type, so a Live + Remix group is charged
    /// twice.
    pub secondary_penalty: f64,
    /// Not every MusicBrainz secondary type — `Soundtrack`, `Audiobook`, `DJ-mix` and
    /// `Mixtape/Street` are absent because a soundtrack is usually the only release of
    /// its material and penalising it would leave nothing to select.
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
    /// Proximity to the release group's canonical track count (the statistical mode
    /// across its releases) — rewards the "real" tracklist over padded editions rather
    /// than rewarding larger tracklists outright.
    pub canonical_track_count: f64,
    /// A cheap proxy for a real commercial release rather than a stub MB entry. Present
    /// identically on both backends.
    pub label_present: f64,
}

impl Default for BonusWeights {
    fn default() -> Self {
        return BonusWeights {
            canonical_track_count: 10.0,
            label_present: 8.0,
        };
    }
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
