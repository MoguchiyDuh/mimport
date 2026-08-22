use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::flac::FlacFile;
use lofty::ogg::{OpusFile, VorbisComments};
use serde::Serialize;

use crate::audio::{self, AudioProperties, is_audio, is_temp_file};
use crate::error::{Error, Result};

pub const ALLOWLIST: &[&str] = &[
    "TITLE",
    "ARTIST",
    "ARTISTS",
    "ALBUMARTIST",
    "ALBUM_ARTIST",
    "ALBUM",
    "TRACK",
    "TRACKNUMBER",
    "TRACKTOTAL",
    "TOTALTRACKS",
    "DISC",
    "DISCNUMBER",
    "DISCTOTAL",
    "TOTALDISCS",
    "DATE",
    "ORIGINALDATE",
    "ORIGINALYEAR",
    "GENRE",
    "MEDIA",
    "LABEL",
    "PUBLISHER",
    "ORGANIZATION",
    "BPM",
    "LENGTH",
    "ISRC",
    "BARCODE",
    "RELEASECOUNTRY",
    "RELEASESTATUS",
    "RELEASETYPE",
    "SCRIPT",
    "COPYRIGHT",
    "COMPOSER",
    "LYRICS",
    "ARTISTSORT",
    "ALBUMARTISTSORT",
    "MUSICBRAINZ_TRACKID",
    "MUSICBRAINZ_ALBUMID",
    "MUSICBRAINZ_ARTISTID",
    "MUSICBRAINZ_ALBUMARTISTID",
    "MUSICBRAINZ_RELEASEGROUPID",
    "MUSICBRAINZ_RELEASETRACKID",
    "MUSICBRAINZ_ALBUMSTATUS",
    "MUSICBRAINZ_ALBUMTYPE",
];

pub fn is_allowed(key: &str) -> bool {
    return normalized_allowlist().contains(&normalize_key(key));
}

fn normalize_key(key: &str) -> String {
    return key
        .chars()
        .filter(|c| return *c != ' ' && *c != '_')
        .collect::<String>()
        .to_ascii_uppercase();
}

fn normalized_allowlist() -> &'static BTreeSet<String> {
    static SET: OnceLock<BTreeSet<String>> = OnceLock::new();
    return SET.get_or_init(|| {
        return ALLOWLIST.iter().map(|k| return normalize_key(k)).collect();
    });
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub files_scanned: usize,
    pub downsampled: Vec<PathBuf>,
    pub tags_purged: Vec<TagsPurged>,
    pub suspicious: Vec<SuspiciousFile>,
    pub stripped_id3v2: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct TagsPurged {
    pub path: PathBuf,
    pub keys: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SuspiciousFile {
    pub path: PathBuf,
    pub reason: String,
}

impl Report {
    pub fn changed(&self) -> bool {
        return !self.downsampled.is_empty()
            || !self.tags_purged.is_empty()
            || !self.stripped_id3v2.is_empty();
    }
}

pub struct Options {
    pub dry_run: bool,
    pub target_rate: u32,
    pub target_depth: u16,
}

pub fn run(root: &Path, opts: &Options) -> Result<Report> {
    if !root.exists() {
        return Err(Error::io(
            root,
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such path"),
        ));
    }

    let files = collect_audio(root)?;
    let mut report = Report {
        files_scanned: files.len(),
        ..Default::default()
    };

    for path in &files {
        if path
            .extension()
            .and_then(|e| return e.to_str())
            .is_some_and(|e| return e.eq_ignore_ascii_case("flac"))
        {
            match strip_id3v2_prefix(path, opts.dry_run) {
                Ok(Some(true)) => report.stripped_id3v2.push(path.clone()),
                Ok(Some(false)) => {
                    report.suspicious.push(SuspiciousFile {
                        path: path.clone(),
                        reason: "ID3v2 chunk before fLaC magic; no fLaC found to strip to"
                            .to_string(),
                    });
                    continue;
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("ID3v2 strip failed for {}: {e}", path.display()),
            }
        }

        let props = match audio::probe(path) {
            Ok(props) => props,
            Err(e) => {
                tracing::warn!("skipping unreadable file {}: {e}", path.display());
                continue;
            }
        };

        if let Some(s) = audio::suspect_lossy(&props) {
            report.suspicious.push(SuspiciousFile {
                path: path.clone(),
                reason: format!("{s:?}"),
            });
        }

        if audio::needs_downsample(&props, opts.target_rate, opts.target_depth) {
            report.downsampled.push(path.clone());
            if !opts.dry_run {
                audio::downsample(path, opts.target_rate, opts.target_depth)?;
            }
        }

        let purged = match purge_tags(path, opts.dry_run) {
            Ok(purged) => purged,
            Err(e) => {
                tracing::warn!("skipping tag purge for {}: {e}", path.display());
                continue;
            }
        };
        if !purged.is_empty() {
            report.tags_purged.push(TagsPurged {
                path: path.clone(),
                keys: purged,
            });
        }
    }

    return Ok(report);
}

fn purge_tags(path: &Path, dry_run: bool) -> Result<Vec<String>> {
    let ext = path
        .extension()
        .and_then(|e| return e.to_str())
        .unwrap_or_default();
    return match ext.to_ascii_lowercase().as_str() {
        "flac" => purge_vorbis_file::<FlacFile>(path, dry_run),
        "opus" => purge_vorbis_file::<OpusFile>(path, dry_run),
        _ => Ok(Vec::new()),
    };
}

trait VorbisCommentsFile: AudioFile {
    fn comments_mut(&mut self) -> Option<&mut VorbisComments>;
}

impl VorbisCommentsFile for FlacFile {
    fn comments_mut(&mut self) -> Option<&mut VorbisComments> {
        return self.vorbis_comments_mut();
    }
}

impl VorbisCommentsFile for OpusFile {
    fn comments_mut(&mut self) -> Option<&mut VorbisComments> {
        return Some(self.vorbis_comments_mut());
    }
}

fn purge_vorbis_file<F: VorbisCommentsFile>(path: &Path, dry_run: bool) -> Result<Vec<String>> {
    let mut file = std::fs::File::open(path).map_err(|e| return Error::io(path, e))?;
    let mut f = F::read_from(&mut file, ParseOptions::new()).map_err(|e| {
        return Error::Probe {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };
    })?;
    let Some(comments) = f.comments_mut() else {
        return Ok(Vec::new());
    };
    let junk = collect_and_strip(comments, dry_run);
    if !junk.is_empty() && !dry_run {
        f.save_to_path(path, WriteOptions::default()).map_err(|e| {
            return Error::Probe {
                path: path.to_path_buf(),
                reason: e.to_string(),
            };
        })?;
    }
    return Ok(junk);
}

fn collect_and_strip(comments: &mut VorbisComments, dry_run: bool) -> Vec<String> {
    let junk: BTreeSet<String> = comments
        .items()
        .filter(|(key, _)| return !is_allowed(key))
        .map(|(key, _)| return key.to_string())
        .collect();
    if junk.is_empty() || dry_run {
        return junk.into_iter().collect();
    }
    for key in &junk {
        let _ = comments.remove(key).count();
    }
    return junk.into_iter().collect();
}

fn collect_audio(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if root.is_file() {
        if is_audio(root) {
            out.push(root.to_path_buf());
        }
        return Ok(out);
    }
    walk(root, &mut out)?;
    out.sort();
    return Ok(out);
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| return Error::io(dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| return Error::io(dir, e))?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if is_audio(&path) && !is_temp_file(&path) {
            out.push(path);
        }
    }
    return Ok(());
}

pub fn quality_ok(props: &AudioProperties, target_rate: u32, target_depth: u16) -> bool {
    return !audio::needs_downsample(props, target_rate, target_depth);
}

/// Some FLACs carry a stray ID3v2 chunk before the `fLaC` stream magic (the
/// "Encountered an ID3v2 tag" lofty warning). Lofty refuses to rewrite such
/// files, which silently breaks `cover --fetch` and re-import tag writes.
/// Returns `None` if the file has no ID3v2 prefix; `Some(true)` if the prefix
/// was stripped (or would be, on dry-run); `Some(false)` if a prefix is
/// present but no `fLaC` magic follows within the first 2 MB.
fn strip_id3v2_prefix(path: &Path, dry_run: bool) -> Result<Option<bool>> {
    let mut file = std::fs::File::open(path).map_err(|e| return Error::io(path, e))?;
    let mut head = [0u8; 3];
    if file.read_exact(&mut head).is_err() {
        return Ok(None);
    }
    if head != *b"ID3" {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|e| return Error::io(path, e))?;
    let mut buf = Vec::new();
    file.take(2 * 1024 * 1024)
        .read_to_end(&mut buf)
        .map_err(|e| return Error::io(path, e))?;
    let Some(pos) = buf.windows(4).position(|w| return w == b"fLaC") else {
        return Ok(Some(false));
    };
    if pos == 0 || dry_run {
        return Ok(Some(true));
    }
    let tmp = path.with_extension("mimport-id3strip.tmp");
    {
        let mut src = std::fs::File::open(path).map_err(|e| return Error::io(path, e))?;
        src.seek(SeekFrom::Start(pos as u64))
            .map_err(|e| return Error::io(path, e))?;
        let mut dst = std::fs::File::create(&tmp).map_err(|e| return Error::io(&tmp, e))?;
        std::io::copy(&mut src, &mut dst).map_err(|e| return Error::io(&tmp, e))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| return Error::io(path, e))?;
    return Ok(Some(true));
}
