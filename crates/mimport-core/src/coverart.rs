use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use image::ImageFormat;
use image::imageops::FilterType;
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
    positive_ttl: Duration,
    negative_ttl: Duration,
}

enum Cached {
    Positive(CoverArt),
    Negative,
    Miss,
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
            positive_ttl: Duration::from_secs(cfg.cache_ttl_secs),
            negative_ttl: Duration::from_secs(cfg.negative_ttl_secs),
        });
    }

    fn cache_path(&self, key: &str) -> PathBuf {
        return self.cache_dir.join(format!("{key}.front-500"));
    }

    fn read_cache(&self, key: &str) -> Cached {
        let path = self.cache_path(key);
        let Ok(meta) = std::fs::metadata(&path) else {
            return Cached::Miss;
        };
        let age = match meta.modified() {
            Ok(m) => SystemTime::now().duration_since(m).unwrap_or_default(),
            Err(_) => Duration::ZERO,
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return Cached::Miss,
        };
        if bytes.is_empty() {
            return if age <= self.negative_ttl {
                Cached::Negative
            } else {
                Cached::Miss
            };
        }
        if age > self.positive_ttl {
            return Cached::Miss;
        }
        match sniff_mime(&bytes) {
            Some(mime) => Cached::Positive(CoverArt {
                mime: mime.to_string(),
                bytes,
            }),
            None => {
                let _ = std::fs::remove_file(&path);
                Cached::Miss
            }
        }
    }

    fn write_cache(&self, key: &str, bytes: &[u8]) {
        if std::fs::create_dir_all(&self.cache_dir).is_err() {
            return;
        }
        let _ = std::fs::write(self.cache_path(key), bytes);
    }

    fn write_negative(&self, key: &str) {
        if std::fs::create_dir_all(&self.cache_dir).is_err() {
            return;
        }
        let _ = std::fs::write(self.cache_path(key), b"");
    }

    pub fn front_cover(&self, release_mbid: &str) -> Result<Option<CoverArt>> {
        return self.fetch(&format!("release/{release_mbid}"), release_mbid);
    }

    pub fn front_cover_release_group(&self, release_group_mbid: &str) -> Result<Option<CoverArt>> {
        return self.fetch(
            &format!("release-group/{release_group_mbid}"),
            &format!("rg-{release_group_mbid}"),
        );
    }

    fn fetch(&self, path: &str, cache_key: &str) -> Result<Option<CoverArt>> {
        match self.read_cache(cache_key) {
            Cached::Positive(c) => return Ok(Some(c)),
            Cached::Negative => return Ok(None),
            Cached::Miss => {}
        }

        let url = format!("{}/{}/front-500", self.base_url, path);
        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .send()
            .map_err(|e| {
                return Error::CoverFetch {
                    url: url.clone(),
                    reason: e.to_string(),
                };
            })?;

        let status = resp.status();
        if status.as_u16() == 404 {
            self.write_negative(cache_key);
            return Ok(None);
        }
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(Error::CoverFetch {
                url,
                reason: format!("HTTP {status} {body}"),
            });
        }

        let bytes = resp.bytes()?.to_vec();
        match sniff_mime(&bytes) {
            Some(mime) => {
                self.write_cache(cache_key, &bytes);
                return Ok(Some(CoverArt {
                    mime: mime.to_string(),
                    bytes,
                }));
            }
            None => {
                tracing::warn!(
                    "cover art fetch for {cache_key}: non-image body ({} bytes), not caching",
                    bytes.len()
                );
                return Ok(None);
            }
        }
    }
}

/// Sniffs the image format from magic bytes so the embedded PICTURE block
/// declares the correct MIME regardless of what the HTTP Content-Type header
/// claimed (CAA has served mislabeled bodies) or what's on disk.
fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some("image/png");
    }
    if bytes.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    return None;
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
            f.save_to_path(path, WriteOptions::default())
                .map_err(save)?;
        }
        "opus" => {
            let mut f = OpusFile::read_from(&mut file, ParseOptions::new()).map_err(save)?;
            f.vorbis_comments_mut().remove_pictures();
            let _ = f.vorbis_comments_mut().insert_picture(picture, None);
            f.save_to_path(path, WriteOptions::default())
                .map_err(save)?;
        }
        _ => return Ok(()),
    }
    return Ok(());
}
