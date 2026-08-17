use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::flac::FlacFile;
use lofty::ogg::{OpusFile, VorbisComments};
use serde::Serialize;

use crate::audio::{self, AudioProperties};
use crate::error::{Error, Result};

const AUDIO_EXTENSIONS: &[&str] = &["flac", "opus", "mp3", "m4a"];

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
        return !self.downsampled.is_empty() || !self.tags_purged.is_empty();
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
        } else if is_audio(&path) {
            out.push(path);
        }
    }
    return Ok(());
}

fn is_audio(path: &Path) -> bool {
    return path
        .extension()
        .and_then(|e| return e.to_str())
        .is_some_and(|e| return AUDIO_EXTENSIONS.iter().any(|a| return e.eq_ignore_ascii_case(a)));
}

pub fn quality_ok(props: &AudioProperties, target_rate: u32, target_depth: u16) -> bool {
    return !audio::needs_downsample(props, target_rate, target_depth);
}
