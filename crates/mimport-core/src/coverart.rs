use std::path::Path;
use std::time::Duration;

use image::imageops::FilterType;
use image::ImageFormat;

use crate::config::CoverArtConfig;
use crate::error::{Error, Result};

/// Matches the Cover Art Archive `front-500` convention so manually supplied
/// covers embed at the same size as ones fetched from CAA.
const MAX_COVER_EDGE: u32 = 500;

pub struct CoverArt {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Loads a local image file, downscales it to `MAX_COVER_EDGE` on the long
/// edge if larger (preserving aspect ratio, no cropping), and re-encodes as
/// JPEG so embedded art stays small and format-consistent regardless of what
/// the source file was.
pub fn from_local_file(path: &Path) -> Result<CoverArt> {
    let bytes = std::fs::read(path).map_err(|e| return Error::io(path, e))?;
    let img = image::load_from_memory(&bytes).map_err(|e| {
        return Error::CoverPreprocess {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };
    })?;

    let resized = if img.width().max(img.height()) > MAX_COVER_EDGE {
        img.resize(MAX_COVER_EDGE, MAX_COVER_EDGE, FilterType::Lanczos3)
    } else {
        img
    };

    let mut out = Vec::new();
    resized
        .write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Jpeg)
        .map_err(|e| {
            return Error::CoverPreprocess {
                path: path.to_path_buf(),
                reason: e.to_string(),
            };
        })?;
    return Ok(CoverArt {
        mime: "image/jpeg".to_string(),
        bytes: out,
    });
}

pub struct CoverArtClient {
    http: reqwest::blocking::Client,
    base_url: String,
    user_agent: String,
}

impl CoverArtClient {
    pub fn new(cfg: &CoverArtConfig, user_agent: &str) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        return Ok(CoverArtClient {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            user_agent: user_agent.to_string(),
        });
    }

    pub fn front_cover(&self, release_mbid: &str) -> Result<Option<CoverArt>> {
        let url = format!("{}/release/{}/front-500", self.base_url, release_mbid);
        let resp = match self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .send()
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!("cover art fetch failed for {release_mbid}: {e}; importing without art");
                return Ok(None);
            }
        };
        let status = resp.status();
        if !status.is_success() {
            if status.as_u16() != 404 {
                let body = resp.text().unwrap_or_default();
                tracing::warn!("cover art fetch failed for {release_mbid}: HTTP {status} {body}; importing without art");
            }
            return Ok(None);
        }
        let mime = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| return v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        let bytes = resp.bytes()?.to_vec();
        return Ok(Some(CoverArt { mime, bytes }));
    }
}
