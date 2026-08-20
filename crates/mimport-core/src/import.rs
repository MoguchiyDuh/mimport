//! Munkres/Hungarian matching of local files against a chosen release's tracks.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::ogg::{OggPictureStorage, VorbisComments};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::probe::read_from_path;
use lofty::tag::{Accessor, ItemKey, Tag, TagExt, TagType};
use pathfinding::prelude::{kuhn_munkres_min, Matrix};
use serde::{Deserialize, Serialize};

use crate::audio::is_audio;
use crate::coverart::CoverArt;
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
    /// Original (pre-romanization) title, set only when `title` was swapped.
    pub title_native: Option<String>,
    pub recording_id: Option<String>,
    /// The matched track's raw (possibly non-numeric) MB track number, carried
    /// from match time so tag/filename derivation never re-looks-up by title.
    pub raw_position: Option<String>,
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

    /// Blocks only on unmatched files; missing release tracks (a partial
    /// album) are allowed and merely reported, not fatal.
    pub fn blocked_unmatched(&self) -> bool {
        return !self.unmatched_files.is_empty();
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
    let (filename_title, filename_track) = filename_hint(path);
    return Ok(LocalTrack {
        path: path.to_path_buf(),
        title: title.or(filename_title),
        artist,
        track_number: track_number.or(filename_track),
        duration_secs,
        musicbrainz_recording_id,
    });
}

fn filename_hint(path: &Path) -> (Option<String>, Option<u32>) {
    let stem = path
        .file_stem()
        .and_then(|s| return s.to_str())
        .unwrap_or("");
    let (num, title) = match stem.find('-') {
        Some(i) if stem[..i].trim().parse::<u32>().is_ok() => {
            let num = stem[..i].trim().parse::<u32>().ok();
            (num, stem[i + 1..].trim())
        }
        _ => (None, stem),
    };
    return (if title.is_empty() { None } else { Some(title.to_string()) }, num);
}

struct Distance {
    total: f64,
    reasons: BTreeMap<&'static str, f64>,
}

fn track_distance(local: &LocalTrack, track: &NormalizedTrack, release_artist: Option<&str>) -> Distance {
    let mut reasons = BTreeMap::new();
    // Normalize by the weight of signals that had data on both sides, not the
    // constant total: a sparsely-tagged file must not score artificially low.
    let mut present_weight = 0.0;

    // MBIDs are a hint: penalized only on an explicit conflict, never for missing data.
    // An empty/whitespace-only local MBID (yt-dlp emits blank MUSICBRAINZ_* fields) is
    // missing data, so it must not count as a conflict either.
    let recording_id_cost = match (&local.musicbrainz_recording_id, &track.recording_id) {
        (Some(local_id), Some(track_id)) if !local_id.trim().is_empty() => {
            present_weight += W_RECORDING_ID;
            if local_id != track_id { 1.0 } else { 0.0 }
        }
        _ => 0.0,
    };
    reasons.insert("recording_id", recording_id_cost * W_RECORDING_ID);

    let title_cost = match &local.title {
        Some(t) => {
            present_weight += W_TITLE;
            1.0 - text_similarity(t, &track.title)
        }
        None => 0.0,
    };
    reasons.insert("track_title", title_cost * W_TITLE);

    // 10s free slack, then scales to the full penalty at 40s off.
    let length_cost = match (local.duration_secs, track.length_ms) {
        (Some(local_secs), Some(mb_ms)) => {
            present_weight += W_LENGTH;
            let diff = (local_secs - mb_ms as f64 / 1000.0).abs();
            ((diff - 10.0).max(0.0) / 30.0).min(1.0)
        }
        _ => 0.0,
    };
    reasons.insert("track_length", length_cost * W_LENGTH);

    let artist_cost = match (&local.artist, release_artist) {
        (Some(local_artist), Some(expected)) => {
            present_weight += W_ARTIST;
            1.0 - text_similarity(local_artist, expected)
        }
        _ => 0.0,
    };
    reasons.insert("track_artist", artist_cost * W_ARTIST);

    let index_cost = match (local.track_number, track.position) {
        (Some(a), Some(b)) => {
            present_weight += W_INDEX;
            if a != b { 1.0 } else { 0.0 }
        }
        _ => 0.0,
    };
    reasons.insert("track_index", index_cost * W_INDEX);

    let denom = if present_weight > 0.0 { present_weight } else { TOTAL_WEIGHT };
    let total: f64 = reasons.values().sum::<f64>() / denom;
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
                title_native: track.title_native.clone(),
                recording_id: track.recording_id.clone(),
                raw_position: track.raw_position.clone(),
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
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ForceTarget {
    Pos(u32),
    Full { position: u32, disc: Option<u32> },
}

pub fn apply_force_mapping(locals: &[LocalTrack], release: &NormalizedRelease, mapping_path: &Path) -> Result<MatchReport> {
    let text = std::fs::read_to_string(mapping_path).map_err(|e| return Error::io(mapping_path, e))?;
    let mapping: BTreeMap<String, ForceTarget> =
        serde_json::from_str(&text).map_err(|e| return Error::ForceMapping(format!("{}: {e}", mapping_path.display())))?;

    let mut matched = Vec::new();
    let mut matched_tracks: HashSet<usize> = HashSet::new();
    let mut matched_files: HashSet<PathBuf> = HashSet::new();

    for (file_str, target) in &mapping {
        let file_path = PathBuf::from(file_str);
        let local = locals.iter().find(|l| return l.path == file_path).ok_or_else(|| {
            return Error::ForceMapping(format!("{file_str:?} is not one of the scanned local files"));
        })?;
        let (position, disc) = match target {
            ForceTarget::Pos(position) => (*position, None),
            ForceTarget::Full { position, disc } => (*position, *disc),
        };
        let mut candidates = release.tracks.iter().enumerate().filter(|(_, t)| {
            return t.position == Some(position)
                && match disc {
                    Some(d) => t.medium_position == Some(d),
                    None => true,
                };
        });
        let first = candidates.next();
        let second = candidates.next();
        let (j, track) = match (first, second) {
            (Some((j, t)), None) => (j, t),
            (Some(_), Some(_)) => {
                return Err(Error::ForceMapping(format!(
                    "position {position} is ambiguous across discs; specify a disc"
                )));
            }
            (None, _) => {
                return Err(Error::ForceMapping(format!("no track at position {position} in the release")));
            }
        };
        if !matched_tracks.insert(j) {
            return Err(Error::ForceMapping(format!(
                "multiple files map to position {position} on disc {disc:?}"
            )));
        }
        matched.push(MatchedTrack {
            file: local.path.clone(),
            position: track.position,
            medium_position: track.medium_position,
            title: track.title.clone(),
            title_native: track.title_native.clone(),
            recording_id: track.recording_id.clone(),
            raw_position: track.raw_position.clone(),
            distance: 0.0,
            reasons: BTreeMap::new(),
        });
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
    /// move instead of copy; falls back to copy+remove across filesystems
    pub move_files: bool,
}

#[derive(Debug, Serialize)]
pub struct ImportedFile {
    pub source: PathBuf,
    pub dest: PathBuf,
}

fn place_file(source: &Path, dest: &Path, move_files: bool) -> Result<()> {
    if !move_files {
        std::fs::copy(source, dest).map_err(|e| return Error::io(dest, e))?;
        return Ok(());
    }
    match std::fs::rename(source, dest) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            std::fs::copy(source, dest).map_err(|e| return Error::io(dest, e))?;
            std::fs::remove_file(source).map_err(|e| return Error::io(source, e))?;
            return Ok(());
        }
        Err(e) => return Err(Error::io(dest, e)),
    }
}

/// Copies (or moves, with `ImportOptions::move_files`) each matched file into
/// the library layout and writes clean tags onto the placed copy.
pub fn write_and_copy(matched: &[MatchedTrack], release: &NormalizedRelease, opts: &ImportOptions, cover_art: Option<&CoverArt>) -> Result<Vec<ImportedFile>> {
    let multi_disc = release.tracks.iter().any(|t| return t.medium_position.unwrap_or(1) > 1);
    let artist_dir = sanitize(release.artist_credit.as_deref().unwrap_or("Unknown Artist"));
    let year = release.year().unwrap_or("");
    let album_dir = if year.is_empty() {
        sanitize(&release.title)
    } else {
        sanitize(&format!("{} ({year})", release.title))
    };

    // Resolve every (source, dest) up front so validation can fail before any
    // file is placed — critical for --move, where a mid-batch abort would
    // otherwise leave earlier sources already moved and unrecoverable.
    let mut plan: Vec<ImportedFile> = Vec::with_capacity(matched.len());
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for m in matched {
        let ext = m.file.extension().and_then(|e| return e.to_str()).unwrap_or("");
        let track_prefix = match m.position {
            Some(p) => format!("{p:02}"),
            None => m
                .raw_position
                .as_deref()
                .map(sanitize)
                .filter(|s| return !s.is_empty())
                .unwrap_or_else(|| return "00".to_string()),
        };
        let filename = if multi_disc {
            let disc = m.medium_position.unwrap_or(1);
            format!("{disc:02}-{track_prefix} - {}.{ext}", sanitize(&m.title))
        } else {
            format!("{track_prefix} - {}.{ext}", sanitize(&m.title))
        };
        let dest = opts.library_root.join(&artist_dir).join(&album_dir).join(filename);

        if !seen.insert(dest.clone()) {
            return Err(Error::io(
                &dest,
                std::io::Error::new(std::io::ErrorKind::AlreadyExists, "two matched files resolve to the same destination"),
            ));
        }
        if !opts.dry_run && dest.exists() && !same_file(&m.file, &dest) {
            return Err(Error::io(
                &dest,
                std::io::Error::new(std::io::ErrorKind::AlreadyExists, "destination already exists"),
            ));
        }
        plan.push(ImportedFile {
            source: m.file.clone(),
            dest,
        });
    }

    if opts.dry_run {
        return Ok(plan);
    }

    for (m, imported) in matched.iter().zip(plan.iter()) {
        if let Some(parent) = imported.dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| return Error::io(parent, e))?;
        }
        if !same_file(&imported.source, &imported.dest) {
            place_file(&imported.source, &imported.dest, opts.move_files)?;
        }
        write_tags(&imported.dest, m, release, cover_art)?;
    }
    return Ok(plan);
}

/// True if both paths exist and refer to the same file (already-imported /
/// re-import case); lets that file be re-tagged in place instead of erroring.
fn same_file(a: &Path, b: &Path) -> bool {
    return match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
}

/// Writes a fresh tag, not a patch onto whatever the source carried.
///
/// VorbisComments (FLAC + Ogg/Opus, i.e. this whole library) is written
/// directly rather than through lofty's generic `ItemKey`-mapped `Tag`:
/// `ItemKey::OriginalArtist`/`OriginalAlbumTitle` have no VorbisComments
/// mapping in lofty (`ItemKey::map_key` returns `&'static str`, so even
/// `ItemKey::Unknown` can never map to one), so anything routed through the
/// generic `Tag` for those two fields is silently dropped on save. Native
/// (CJK/etc) artist/album titles ride in raw `ORIGINALARTIST`/
/// `ORIGINALALBUMTITLE` vorbis fields instead.
fn write_tags(path: &Path, m: &MatchedTrack, release: &NormalizedRelease, cover_art: Option<&CoverArt>) -> Result<()> {
    let mut tagged = read_from_path(path).map_err(|e| {
        return Error::Probe {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };
    })?;
    let tag_type = tagged.primary_tag_type();

    if tag_type == TagType::VorbisComments {
        return write_vorbis_tags(path, m, release, cover_art);
    }

    let mut tag = Tag::new(tag_type);

    tag.set_title(m.title.clone());
    tag.set_album(release.title.clone());
    if let Some(artist) = &release.artist_credit {
        tag.set_artist(artist.clone());
    }
    if let Some(pos) = m.position {
        tag.set_track(pos);
    } else if let Some(raw) = &m.raw_position {
        let _ = tag.insert_text(ItemKey::TrackNumber, raw.clone());
    }
    let disc_total = disc_total_for(m, release);
    if disc_total > 0 {
        tag.set_track_total(disc_total as u32);
    }
    if let Some(disc) = m.medium_position {
        tag.set_disk(disc);
    }
    if let Some(date) = &release.date
        && !date.is_empty()
    {
        let _ = tag.insert_text(ItemKey::RecordingDate, date.clone());
    }
    if let Some(label) = &release.label {
        let _ = tag.insert_text(ItemKey::Label, label.clone());
    }
    if let Some(genre) = &release.genre {
        tag.set_genre(genre.clone());
    }
    if let Some(id) = &m.recording_id {
        let _ = tag.insert_text(ItemKey::MusicBrainzRecordingId, id.clone());
    }
    if let Some(native) = &release.artist_credit_native {
        let _ = tag.insert_text(ItemKey::OriginalArtist, native.clone());
    }
    if let Some(native) = &release.title_native {
        let _ = tag.insert_text(ItemKey::OriginalAlbumTitle, native.clone());
    }
    if let Some(native) = &m.title_native {
        let _ = tag.insert_text(ItemKey::Comment, format!("Original title: {native}"));
    }
    // release.id may be a non-mbid sentinel (yt fetch with no --release backfill)
    if looks_like_mbid(&release.id) {
        let _ = tag.insert_text(ItemKey::MusicBrainzReleaseId, release.id.clone());
    }

    if let Some(ca) = cover_art {
        let picture = Picture::unchecked(ca.bytes.clone())
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::from_str(&ca.mime))
            .build();
        tag.push_picture(picture);
    }

    let _ = tagged.insert_tag(tag);
    tagged.save_to_path(path, WriteOptions::default()).map_err(|e| {
        return Error::Probe {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };
    })?;
    return Ok(());
}

fn disc_total_for(m: &MatchedTrack, release: &NormalizedRelease) -> usize {
    return match m.medium_position {
        Some(disc) => release
            .tracks
            .iter()
            .filter(|t| return t.medium_position == Some(disc))
            .count(),
        None => release.track_count as usize,
    };
}

fn write_vorbis_tags(path: &Path, m: &MatchedTrack, release: &NormalizedRelease, cover_art: Option<&CoverArt>) -> Result<()> {
    let mut vc = VorbisComments::new();

    vc.set_title(m.title.clone());
    vc.set_album(release.title.clone());
    if let Some(artist) = &release.artist_credit {
        vc.set_artist(artist.clone());
    }
    if let Some(pos) = m.position {
        vc.set_track(pos);
    } else if let Some(raw) = &m.raw_position {
        vc.insert("TRACKNUMBER".to_string(), raw.clone());
    }
    let disc_total = disc_total_for(m, release);
    if disc_total > 0 {
        vc.set_track_total(disc_total as u32);
    }
    if let Some(disc) = m.medium_position {
        vc.set_disk(disc);
    }
    if let Some(date) = &release.date
        && !date.is_empty()
    {
        vc.insert("DATE".to_string(), date.clone());
    }
    if let Some(label) = &release.label {
        vc.insert("LABEL".to_string(), label.clone());
    }
    if let Some(genre) = &release.genre {
        vc.set_genre(genre.clone());
    }
    if let Some(id) = &m.recording_id {
        vc.insert("MUSICBRAINZ_TRACKID".to_string(), id.clone());
    }
    if let Some(native) = &release.artist_credit_native {
        vc.insert("ORIGINALARTIST".to_string(), native.clone());
    }
    if let Some(native) = &release.title_native {
        vc.insert("ORIGINALALBUMTITLE".to_string(), native.clone());
    }
    if let Some(native) = &m.title_native {
        vc.insert("COMMENT".to_string(), format!("Original title: {native}"));
    }
    // release.id may be a non-mbid sentinel (yt fetch with no --release backfill)
    if looks_like_mbid(&release.id) {
        vc.insert("MUSICBRAINZ_ALBUMID".to_string(), release.id.clone());
    }

    if let Some(ca) = cover_art {
        let picture = Picture::unchecked(ca.bytes.clone())
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::from_str(&ca.mime))
            .build();
        let _ = vc.insert_picture(picture, None);
    }

    vc.save_to_path(path, WriteOptions::default()).map_err(|e| {
        return Error::Probe {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };
    })?;
    return Ok(());
}

fn looks_like_mbid(s: &str) -> bool {
    return s.len() == 36 && s.bytes().filter(|b| return *b == b'-').count() == 4;
}

fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| return if "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    return cleaned.trim().trim_end_matches('.').to_string();
}
