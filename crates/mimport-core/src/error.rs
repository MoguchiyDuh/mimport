//! Single error type for the whole core crate. Every public function returns `Result<T>`.

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

    #[error("http transport error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("db error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("not yet implemented: {0}")]
    NotImplemented(&'static str),

    #[error("probe/tag error at {path}: {reason}")]
    Probe { path: PathBuf, reason: String },
}

impl Error {
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        return Error::Io {
            path: path.as_ref().to_path_buf(),
            source,
        };
    }
}
