use std::path::{Path, PathBuf};
use std::time::Duration;

use image::imageops::FilterType;
use image::ImageFormat;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::ogg::{OggPictureStorage, OpusFile};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::probe::read_from_path;

use crate::cache::cache_dir_default;
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
    cache_dir: PathBuf,
}

impl CoverArtClient {
    pub fn new(cfg: &CoverArtConfig, user_agent: &str) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        let cache_dir = cfg
            .cache_dir
            .clone()
            .unwrap_or_else(|| return cache_dir_default("coverart"));
        return Ok(CoverArtClient {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            user_agent: user_agent.to_string(),
            cache_dir,
        });
    }

    fn cache_path(&self, release_mbid: &str) -> PathBuf {
        return self.cache_dir.join(format!("{release_mbid}.front-500"));
    }

    fn read_cache(&self, release_mbid: &str) -> Option<CoverArt> {
        let bytes = std::fs::read(self.cache_path(release_mbid)).ok()?;
        if bytes.is_empty() {
            return None;
        }
        return Some(CoverArt {
            mime: "image/jpeg".to_string(),
            bytes,
        });
    }

    fn write_cache(&self, release_mbid: &str, bytes: &[u8]) {
        if std::fs::create_dir_all(&self.cache_dir).is_err() {
            return;
        }
        let _ = std::fs::write(self.cache_path(release_mbid), bytes);
    }

    pub fn front_cover(&self, release_mbid: &str) -> Result<Option<CoverArt>> {
        if let Some(cached) = self.read_cache(release_mbid) {
            return Ok(Some(cached));
        }

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
        self.write_cache(release_mbid, &bytes);
        return Ok(Some(CoverArt { mime, bytes }));
    }
}

/// True if the file already carries at least one embedded picture.
pub fn has_embedded_cover(path: &Path) -> bool {
    let Ok(tagged) = read_from_path(path) else {
        return false;
    };
    let Some(tag) = tagged.primary_tag() else {
        return false;
    };
    return !tag.pictures().is_empty();
}

/// Embeds `cover` into an existing file's tag in place, preserving all other
/// tags and replacing any previously embedded pictures. Only FLAC/Opus
/// (VorbisComments) are supported; other formats are a no-op.
pub fn embed_cover(path: &Path, cover: &CoverArt) -> Result<()> {
    let picture = Picture::unchecked(cover.bytes.clone())
        .pic_type(PictureType::CoverFront)
        .mime_type(MimeType::from_str(&cover.mime))
        .build();

    let ext = path
        .extension()
        .and_then(|e| return e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let mut file = std::fs::File::open(path).map_err(|e| return Error::io(path, e))?;
    let save = |err: lofty::error::LoftyError| {
        return Error::Probe {
            path: path.to_path_buf(),
            reason: err.to_string(),
        };
    };

    match ext.as_str() {
        "flac" => {
            let mut f = FlacFile::read_from(&mut file, ParseOptions::new()).map_err(save)?;
            if let Some(vc) = f.vorbis_comments_mut() {
                let _ = vc.remove_pictures();
                let _ = vc.insert_picture(picture, None);
            }
            f.save_to_path(path, WriteOptions::default()).map_err(save)?;
        }
        "opus" => {
            let mut f = OpusFile::read_from(&mut file, ParseOptions::new()).map_err(save)?;
            f.vorbis_comments_mut().remove_pictures();
            let _ = f.vorbis_comments_mut().insert_picture(picture, None);
            f.save_to_path(path, WriteOptions::default()).map_err(save)?;
        }
        _ => return Ok(()),
    }
    return Ok(());
}
