use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("config error: {0}")]
    Config(String),

    #[error("musicbrainz {what} failed: HTTP {status}\n{body}")]
    Mb {
        what: &'static str,
        status: u16,
        body: String,
    },

    #[error("musicbrainz is still rate limited after {attempts} attempts")]
    MbRateLimited { attempts: u32 },

    #[error("musicbrainz has no {entity} with mbid {mbid}")]
    MbNotFound { entity: &'static str, mbid: String },

    #[error("musicbrainz.user_agent is not set meaningfully: {value:?}\nset it in config.toml")]
    MbUserAgentUnset { value: String },

    #[error("slskd.{field} is not set meaningfully: {value:?}\nset it in config.toml")]
    SlskdCredsUnset { field: &'static str, value: String },

    #[error("lidarr proxy {what} failed: HTTP {status}\n{body}")]
    Lidarr {
        what: &'static str,
        status: u16,
        body: String,
    },

    #[error("lidarr proxy has no {entity} with mbid {mbid}")]
    LidarrNotFound { entity: &'static str, mbid: String },

    #[error("lidarr proxy is still failing after {attempts} attempts")]
    LidarrUnavailable { attempts: u32 },

    #[error("slskd {what} failed: HTTP {status}\n{body}")]
    Slskd {
        what: &'static str,
        status: u16,
        body: String,
    },

    #[error("slskd login failed: HTTP {status}\n{body}")]
    SlskdAuth { status: u16, body: String },

    #[error("slskd has no user or peer resource at {what} (peer likely offline)")]
    SlskdNotFound { what: String },

    #[error("slskd transfer {username}/{id} timed out waiting for completion after {waited_secs}s (last state: {last_state})")]
    SlskdFetchTimeout {
        username: String,
        id: String,
        waited_secs: u64,
        last_state: String,
    },

    #[error("slskd fetch selector matched no {what}: {detail}")]
    SlskdSelectorNotFound { what: &'static str, detail: String },

    #[error("no job matches {target:?} (not a known job id and no job.title matches it)")]
    JobNotFound { target: String },

    #[error("http transport error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("db error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("probe/tag error at {path}: {reason}")]
    Probe { path: PathBuf, reason: String },

    #[error("required external tool not found on PATH: {tool}")]
    ToolMissing { tool: &'static str },

    #[error("{tool} failed ({status})\n{stderr}")]
    ToolFailed {
        tool: &'static str,
        status: String,
        stderr: String,
    },

    #[error("invalid config value: {0}")]
    ConfigInvalid(String),

    #[error("--force mapping invalid: {0}")]
    ForceMapping(String),

    #[error("no library track with id {id}")]
    TrackNotFound { id: i64 },

    #[error("invalid library query term {term:?}: {reason}")]
    QuerySyntax { term: String, reason: &'static str },

    #[error("`yt fetch --release` requires `--track` to pick which release track to backfill from")]
    YtReleaseNeedsTrack,

    #[error("release {release_mbid} has no track at position {position}")]
    YtTrackNotFound { release_mbid: String, position: u32 },

    #[error("`yt fetch` needs a title: pass --title, or --release/--track to backfill one")]
    YtTitleRequired,

    #[error("`yt fetch` fetched no tracks")]
    YtEmptyFetch,

    #[error("release has non-Latin fields with no MusicBrainz romanization alias; supply --tags/--track-title or pass --allow-native:\n{0}")]
    UnresolvedTitles(String),

    #[error("--tags file invalid: {0}")]
    TagOverrides(String),

    #[error("failed to preprocess cover image at {path}: {reason}")]
    CoverPreprocess { path: PathBuf, reason: String },
}

impl Error {
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        return Error::Io {
            path: path.as_ref().to_path_buf(),
            source,
        };
    }
}
