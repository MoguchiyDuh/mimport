//! Munkres/Hungarian matching of local files against a chosen release's tracks.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::read_from_path;
use lofty::tag::{Accessor, ItemKey, Tag};
use pathfinding::prelude::{kuhn_munkres_min, Matrix};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::release::{NormalizedRelease, NormalizedTrack};
use crate::scorer::text_similarity;

/// Per-track distance above this is rejected even if Munkres assigned it.
pub const REJECT_THRESHOLD: f64 = 0.40;

const W_RECORDING_ID: f64 = 10.0;
const W_TITLE: f64 = 3.0;
const W_LENGTH: f64 = 2.0;
const W_ARTIST: f64 = 2.0;
const W_INDEX: f64 = 1.0;
const TOTAL_WEIGHT: f64 = W_RECORDING_ID + W_TITLE + W_LENGTH + W_ARTIST + W_INDEX;

const SCALE: i64 = 1_000_000;
/// Dummy row/column cost when padding to square; equals the max real distance
/// so real matches win and hopeless ones can go unmatched.
const DUMMY_COST_SCALED: i64 = SCALE;

const AUDIO_EXTENSIONS: &[&str] = &["flac", "mp3", "m4a", "mp4", "ogg", "opus", "wav", "ape", "wv"];

pub struct LocalTrack {
    pub path: PathBuf,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub track_number: Option<u32>,
    pub duration_secs: Option<f64>,
    pub musicbrainz_recording_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MatchedTrack {
    pub file: PathBuf,
    pub position: Option<u32>,
    pub medium_position: Option<u32>,
    pub title: String,
    pub recording_id: Option<String>,
    pub distance: f64,
    pub reasons: BTreeMap<&'static str, f64>,
}

#[derive(Debug, Serialize)]
pub struct MissingTrack {
    pub position: Option<u32>,
    pub medium_position: Option<u32>,
    pub title: String,
    pub recording_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MatchReport {
    pub matched: Vec<MatchedTrack>,
    pub unmatched_files: Vec<PathBuf>,
    pub missing_tracks: Vec<MissingTrack>,
}

impl MatchReport {
    /// Blocks the copy/tag step whenever either set is non-empty.
    pub fn blocked(&self) -> bool {
        return !self.unmatched_files.is_empty() || !self.missing_tracks.is_empty();
    }
}

/// Reads local audio files (a dir or a single file) into `LocalTrack`s.
pub fn scan_local_tracks(dir: &Path) -> Result<Vec<LocalTrack>> {
    let mut paths = Vec::new();
    if dir.is_file() {
        if is_audio(dir) {
            paths.push(dir.to_path_buf());
        }
    } else {
        walk(dir, &mut paths)?;
    }
    paths.sort();

    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        out.push(read_local_track(&path)?);
    }
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

fn read_local_track(path: &Path) -> Result<LocalTrack> {
    let tagged = read_from_path(path).map_err(|e| {
        return Error::Probe {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };
    })?;
    let duration_secs = Some(tagged.properties().duration().as_secs_f64());
    let (title, artist, track_number, musicbrainz_recording_id) = match tagged.primary_tag() {
        Some(t) => (
            t.title().map(|c| return c.to_string()),
            t.artist().map(|c| return c.to_string()),
            t.track(),
            t.get_string(ItemKey::MusicBrainzRecordingId).map(|s| return s.to_string()),
        ),
        None => (None, None, None, None),
    };
    return Ok(LocalTrack {
        path: path.to_path_buf(),
        title,
        artist,
        track_number,
        duration_secs,
        musicbrainz_recording_id,
    });
}

struct Distance {
    total: f64,
    reasons: BTreeMap<&'static str, f64>,
}

fn track_distance(local: &LocalTrack, track: &NormalizedTrack, release_artist: Option<&str>) -> Distance {
    let mut reasons = BTreeMap::new();

    // MBIDs are a hint: penalized only on an explicit conflict, never for missing data.
    let recording_id_cost = match (&local.musicbrainz_recording_id, &track.recording_id) {
        (Some(local_id), Some(track_id)) if local_id != track_id => 1.0,
        _ => 0.0,
    };
    reasons.insert("recording_id", recording_id_cost * W_RECORDING_ID);

    let title_cost = match &local.title {
        Some(t) => 1.0 - text_similarity(t, &track.title),
        None => 0.0,
    };
    reasons.insert("track_title", title_cost * W_TITLE);

    // 10s free slack, then scales to the full penalty at 40s off.
    let length_cost = match (local.duration_secs, track.length_ms) {
        (Some(local_secs), Some(mb_ms)) => {
            let diff = (local_secs - mb_ms as f64 / 1000.0).abs();
            ((diff - 10.0).max(0.0) / 30.0).min(1.0)
        }
        _ => 0.0,
    };
    reasons.insert("track_length", length_cost * W_LENGTH);

    let artist_cost = match (&local.artist, release_artist) {
        (Some(local_artist), Some(expected)) => 1.0 - text_similarity(local_artist, expected),
        _ => 0.0,
    };
    reasons.insert("track_artist", artist_cost * W_ARTIST);

    let index_cost = match (local.track_number, track.position) {
        (Some(a), Some(b)) if a != b => 1.0,
        _ => 0.0,
    };
    reasons.insert("track_index", index_cost * W_INDEX);

    let total: f64 = reasons.values().sum::<f64>() / TOTAL_WEIGHT;
    return Distance { total, reasons };
}

/// Munkres assignment; padded to square so files/tracks can go unmatched
/// rather than force-pair with the nearest leftover.
pub fn match_tracks(locals: &[LocalTrack], release: &NormalizedRelease) -> MatchReport {
    let n = locals.len();
    let m = release.tracks.len();
    let k = n.max(m);

    if k == 0 {
        return MatchReport {
            matched: Vec::new(),
            unmatched_files: Vec::new(),
            missing_tracks: Vec::new(),
        };
    }

    let mut real_distances: Vec<Vec<Distance>> = Vec::with_capacity(n);
    for local in locals {
        let mut row = Vec::with_capacity(m);
        for track in &release.tracks {
            row.push(track_distance(local, track, release.artist_credit.as_deref()));
        }
        real_distances.push(row);
    }

    let mut cost_rows: Vec<Vec<i64>> = Vec::with_capacity(k);
    for i in 0..k {
        let real_row = real_distances.get(i);
        let mut row = Vec::with_capacity(k);
        for j in 0..k {
            let cost = match real_row.and_then(|r| return r.get(j)) {
                Some(d) => (d.total * SCALE as f64).round() as i64,
                None => DUMMY_COST_SCALED,
            };
            row.push(cost);
        }
        cost_rows.push(row);
    }
    let weights = Matrix::from_rows(cost_rows).expect("cost matrix is square by construction");
    let (_, assignment) = kuhn_munkres_min(&weights);

    let mut matched = Vec::new();
    let mut unmatched_files = Vec::new();
    let mut matched_tracks: HashSet<usize> = HashSet::new();

    for (i, &j) in assignment.iter().enumerate().take(n) {
        if j < m && real_distances[i][j].total <= REJECT_THRESHOLD {
            let d = &real_distances[i][j];
            let track = &release.tracks[j];
            matched.push(MatchedTrack {
                file: locals[i].path.clone(),
                position: track.position,
                medium_position: track.medium_position,
                title: track.title.clone(),
                recording_id: track.recording_id.clone(),
                distance: d.total,
                reasons: d.reasons.clone(),
            });
            matched_tracks.insert(j);
            continue;
        }
        unmatched_files.push(locals[i].path.clone());
    }

    let missing_tracks = release
        .tracks
        .iter()
        .enumerate()
        .filter(|(j, _)| return !matched_tracks.contains(j))
        .map(|(_, t)| {
            return MissingTrack {
                position: t.position,
                medium_position: t.medium_position,
                title: t.title.clone(),
                recording_id: t.recording_id.clone(),
            };
        })
        .collect();

    return MatchReport {
        matched,
        unmatched_files,
        missing_tracks,
    };
}

/// Parses a `{"<file path>": <track position>}` mapping into a report,
/// bypassing matching. Unmapped files/tracks still report unmatched/missing.
pub fn apply_force_mapping(locals: &[LocalTrack], release: &NormalizedRelease, mapping_path: &Path) -> Result<MatchReport> {
    let text = std::fs::read_to_string(mapping_path).map_err(|e| return Error::io(mapping_path, e))?;
    let mapping: BTreeMap<String, u32> =
        serde_json::from_str(&text).map_err(|e| return Error::ForceMapping(format!("{}: {e}", mapping_path.display())))?;

    let mut matched = Vec::new();
    let mut matched_tracks: HashSet<usize> = HashSet::new();
    let mut matched_files: HashSet<PathBuf> = HashSet::new();

    for (file_str, position) in &mapping {
        let file_path = PathBuf::from(file_str);
        let local = locals.iter().find(|l| return l.path == file_path).ok_or_else(|| {
            return Error::ForceMapping(format!("{file_str:?} is not one of the scanned local files"));
        })?;
        let (j, track) = release
            .tracks
            .iter()
            .enumerate()
            .find(|(_, t)| return t.position == Some(*position))
            .ok_or_else(|| {
                return Error::ForceMapping(format!("no track at position {position} in the release"));
            })?;
        matched.push(MatchedTrack {
            file: local.path.clone(),
            position: track.position,
            medium_position: track.medium_position,
            title: track.title.clone(),
            recording_id: track.recording_id.clone(),
            distance: 0.0,
            reasons: BTreeMap::new(),
        });
        matched_tracks.insert(j);
        matched_files.insert(local.path.clone());
    }

    let unmatched_files = locals
        .iter()
        .filter(|l| return !matched_files.contains(&l.path))
        .map(|l| return l.path.clone())
        .collect();
    let missing_tracks = release
        .tracks
        .iter()
        .enumerate()
        .filter(|(j, _)| return !matched_tracks.contains(j))
        .map(|(_, t)| {
            return MissingTrack {
                position: t.position,
                medium_position: t.medium_position,
                title: t.title.clone(),
                recording_id: t.recording_id.clone(),
            };
        })
        .collect();

    return Ok(MatchReport {
        matched,
        unmatched_files,
        missing_tracks,
    });
}

pub struct ImportOptions {
    pub dry_run: bool,
    pub library_root: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct ImportedFile {
    pub source: PathBuf,
    pub dest: PathBuf,
}

/// Copies each matched file into the library layout and writes clean tags onto
/// the copy; the source is never touched or moved.
pub fn write_and_copy(matched: &[MatchedTrack], release: &NormalizedRelease, opts: &ImportOptions) -> Result<Vec<ImportedFile>> {
    let multi_disc = release.tracks.iter().any(|t| return t.medium_position.unwrap_or(1) > 1);
    let artist_dir = sanitize(release.artist_credit.as_deref().unwrap_or("Unknown Artist"));
    let year = release.year().unwrap_or("");
    let album_dir = if year.is_empty() {
        sanitize(&release.title)
    } else {
        sanitize(&format!("{} ({year})", release.title))
    };

    let mut out = Vec::with_capacity(matched.len());
    for m in matched {
        let ext = m.file.extension().and_then(|e| return e.to_str()).unwrap_or("");
        let track_num = m.position.unwrap_or(0);
        let filename = if multi_disc {
            let disc = m.medium_position.unwrap_or(1);
            format!("{disc:02}-{track_num:02} - {}.{ext}", sanitize(&m.title))
        } else {
            format!("{track_num:02} - {}.{ext}", sanitize(&m.title))
        };
        let dest = opts.library_root.join(&artist_dir).join(&album_dir).join(filename);

        if opts.dry_run {
            out.push(ImportedFile {
                source: m.file.clone(),
                dest,
            });
            continue;
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| return Error::io(parent, e))?;
        }
        std::fs::copy(&m.file, &dest).map_err(|e| return Error::io(&dest, e))?;
        write_tags(&dest, m, release)?;
        out.push(ImportedFile {
            source: m.file.clone(),
            dest,
        });
    }
    return Ok(out);
}

/// Writes a fresh tag, not a patch onto whatever the source carried.
fn write_tags(path: &Path, m: &MatchedTrack, release: &NormalizedRelease) -> Result<()> {
    let mut tagged = read_from_path(path).map_err(|e| {
        return Error::Probe {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };
    })?;
    let mut tag = Tag::new(tagged.primary_tag_type());

    tag.set_title(m.title.clone());
    tag.set_album(release.title.clone());
    if let Some(artist) = &release.artist_credit {
        tag.set_artist(artist.clone());
    }
    if let Some(pos) = m.position {
        tag.set_track(pos);
    }
    if release.track_count > 0 {
        tag.set_track_total(release.track_count);
    }
    if let Some(disc) = m.medium_position {
        tag.set_disk(disc);
    }
    if let Some(id) = &m.recording_id {
        let _ = tag.insert_text(ItemKey::MusicBrainzRecordingId, id.clone());
    }
    let _ = tag.insert_text(ItemKey::MusicBrainzReleaseId, release.id.clone());

    let _ = tagged.insert_tag(tag);
    tagged.save_to_path(path, WriteOptions::default()).map_err(|e| {
        return Error::Probe {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };
    })?;
    return Ok(());
}

fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| return if "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    return cleaned.trim().trim_end_matches('.').to_string();
}
