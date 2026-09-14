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

const ITUNES_BASE_URL: &str = "https://itunes.apple.com";

pub struct CoverArt {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Downscales to `MAX_COVER_EDGE` on the long edge if larger (preserving
/// aspect ratio, no cropping) and re-encodes as JPEG so embedded art stays
/// small and format-consistent regardless of what the source file was.
fn finish_cover(img: image::DynamicImage) -> Result<CoverArt> {
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
                path: PathBuf::from("<memory>"),
                reason: e.to_string(),
            };
        })?;
    return Ok(CoverArt {
        mime: "image/jpeg".to_string(),
        bytes: out,
    });
}

/// Loads a local image file and normalizes it via [`finish_cover`].
pub fn from_local_file(path: &Path) -> Result<CoverArt> {
    let bytes = std::fs::read(path).map_err(|e| return Error::io(path, e))?;
    let img = image::load_from_memory(&bytes).map_err(|e| {
        return Error::CoverPreprocess {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };
    })?;
    return finish_cover(img);
}

fn norm_title(s: &str) -> String {
    return s
        .chars()
        .filter(|c| return c.is_alphanumeric())
        .flat_map(|c| return c.to_lowercase())
        .collect();
}

/// iTunes tokenizes the term as required words, so a multi-title release
/// (`A / B`) or an appended `- EP`/`- Single` suffix drives the result count
/// to zero. Keep only the first title and drop the format suffix.
fn itunes_album_term(album: &str) -> String {
    let head = album.split(" / ").next().unwrap_or(album).trim();
    let head = head
        .strip_suffix(" - EP")
        .or_else(|| return head.strip_suffix(" - Single"))
        .unwrap_or(head);
    return head.trim().to_string();
}

/// iTunes stores collab artists with `&`; libraries commonly use a standalone
/// `x`/`×`. Rewrite only whitespace-delimited single-char separators so an
/// intra-word `x` is left alone.
fn itunes_artist_term(artist: &str) -> String {
    return artist.replace(" x ", " & ").replace(" × ", " & ");
}

pub struct CoverArtClient {
    http: reqwest::blocking::Client,
    base_url: String,
    user_agent: String,
    cache_dir: PathBuf,
    positive_ttl: Duration,
    negative_ttl: Duration,
    itunes_fallback: bool,
    itunes_country: String,
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
            itunes_fallback: cfg.itunes_fallback,
            itunes_country: cfg.itunes_country.clone(),
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
        let cover = self.fetch_caa(&format!("release/{release_mbid}"), release_mbid)?;
        if cover.is_none() {
            self.write_negative(release_mbid);
        }
        return Ok(cover);
    }

    /// Release front cover with an iTunes Search API fallback for releases the
    /// archive has no art for. The fallback result is cached under a distinct
    /// key so stale CAA-only negatives from earlier runs don't mask it.
    pub fn front_cover_with_fallback(
        &self,
        release_mbid: &str,
        artist: &str,
        album: &str,
    ) -> Result<Option<CoverArt>> {
        // An empty release id (synthetic YT release) has no CAA entry; the
        // request would be a malformed `release//front-500`, so skip to iTunes.
        if !release_mbid.is_empty() {
            let key = format!("{release_mbid}.fb");
            match self.read_cache(&key) {
                Cached::Positive(c) => return Ok(Some(c)),
                Cached::Negative => return Ok(None),
                Cached::Miss => {}
            }

            match self.fetch_caa(&format!("release/{release_mbid}"), release_mbid) {
                Ok(Some(c)) => return Ok(Some(c)),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("CAA fetch failed for {release_mbid}: {e}; trying iTunes");
                }
            }
        }
        let key = format!("{release_mbid}.fb");

        match self.fetch_itunes(artist, album)? {
            Some(c) => {
                self.write_cache(&key, &c.bytes);
                return Ok(Some(c));
            }
            None => {
                self.write_negative(&key);
                return Ok(None);
            }
        }
    }

    pub fn itunes_fallback_enabled(&self) -> bool {
        return self.itunes_fallback;
    }

    /// Looks up an album cover via the iTunes Search API (relevance-ranked;
    /// the first result whose normalized collection name matches the album
    /// wins, otherwise the first result). Not cached; callers decide.
    pub fn fetch_itunes(&self, artist: &str, album: &str) -> Result<Option<CoverArt>> {
        if artist.is_empty() || album.is_empty() {
            return Ok(None);
        }

        let term = format!("{} {}", itunes_artist_term(artist), itunes_album_term(album));
        let search: serde_json::Value = self
            .http
            .get(format!("{ITUNES_BASE_URL}/search"))
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .query(&[
                ("term", term.clone()),
                ("entity", "album".to_string()),
                ("country", self.itunes_country.clone()),
                ("limit", "5".to_string()),
            ])
            .send()
            .map_err(|e| {
                return Error::CoverFetch {
                    url: format!("{ITUNES_BASE_URL}/search?term={artist}+{album}"),
                    reason: e.to_string(),
                };
            })?
            .json()
            .map_err(|e| {
                return Error::CoverFetch {
                    url: format!("{ITUNES_BASE_URL}/search?term={artist}+{album}"),
                    reason: e.to_string(),
                };
            })?;

        let results = search
            .get("results")
            .and_then(|r| return r.as_array())
            .cloned()
            .unwrap_or_default();
        let album_norm = norm_title(album);
        let artist_norm = norm_title(&itunes_artist_term(artist));
        let mut art_url: Option<String> = None;
        for res in &results {
            let name = res
                .get("collectionName")
                .and_then(|v| return v.as_str())
                .unwrap_or_default();
            let norm = norm_title(name);
            if !album_norm.is_empty()
                && !norm.is_empty()
                && (norm.contains(&album_norm) || album_norm.contains(&norm))
            {
                art_url = res
                    .get("artworkUrl100")
                    .and_then(|v| return v.as_str())
                    .map(|v| return v.to_string());
                break;
            }
        }
        // No collection-name match (common when the storefront localizes the
        // title): fall back to the first result only when its artist plausibly
        // matches, so a noisy query can't embed an unrelated album's cover.
        if art_url.is_none() {
            art_url = results
                .first()
                .filter(|r| {
                    let a = norm_title(
                        r.get("artistName")
                            .and_then(|v| return v.as_str())
                            .unwrap_or_default(),
                    );
                    return artist_norm.is_empty()
                        || a.is_empty()
                        || a.contains(&artist_norm)
                        || artist_norm.contains(&a);
                })
                .and_then(|r| return r.get("artworkUrl100"))
                .and_then(|v| return v.as_str())
                .map(|v| return v.to_string());
        }
        let Some(art_url) = art_url else {
            return Ok(None);
        };
        // artworkUrl100 is a 100x100 thumbnail; requesting a larger square
        // from the same CDN path yields up to the original resolution.
        let big = art_url.replace("100x100bb", "1000x1000bb");

        let resp = self
            .http
            .get(&big)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .send()
            .map_err(|e| {
                return Error::CoverFetch {
                    url: big.clone(),
                    reason: e.to_string(),
                };
            })?;
        if !resp.status().is_success() {
            tracing::warn!(
                "itunes artwork fetch failed ({big}): HTTP {}",
                resp.status()
            );
            return Ok(None);
        }
        let bytes = resp.bytes()?.to_vec();
        let img = match image::load_from_memory(&bytes) {
            Ok(img) => img,
            Err(e) => {
                tracing::warn!("itunes artwork not an image ({big}): {e}");
                return Ok(None);
            }
        };
        tracing::info!("cover art from iTunes fallback: {big}");
        return finish_cover(img).map(|c| return Some(c));
    }

    pub fn front_cover_release_group(&self, release_group_mbid: &str) -> Result<Option<CoverArt>> {
        let key = format!("rg-{release_group_mbid}");
        let cover = self.fetch_caa(&format!("release-group/{release_group_mbid}"), &key)?;
        if cover.is_none() {
            self.write_negative(&key);
        }
        return Ok(cover);
    }

    fn fetch_caa(&self, path: &str, cache_key: &str) -> Result<Option<CoverArt>> {
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
        // FLAC: pictures are file-level PICTURE blocks; clear them all so a
        // source rip's art doesn't survive next to the new one.
        "flac" => {
            let mut f = FlacFile::read_from(&mut file, ParseOptions::new()).map_err(save)?;
            f.remove_pictures();
            f.insert_picture(picture, None).map_err(|e| {
                return Error::Probe {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                };
            })?;
            f.save_to_path(path, WriteOptions::default())
                .map_err(save)?;
        }
        // Opus: pictures live inside the VorbisComments.
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
