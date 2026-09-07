//! Library index + query language over `library_tracks`, plus track editing
//! (index row + file tags + optional rename).

use std::path::Path;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::ogg::OpusFile;
use lofty::probe::read_from_path;
use lofty::tag::{Accessor, ItemKey, Tag};
use rusqlite::types::Value;
use rusqlite::{Connection, params, params_from_iter};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::import::{ImportedFile, MatchedTrack, sanitize};
use crate::release::NormalizedRelease;
use crate::scorer::text_similarity;

/// `~fuzzy` hit threshold on `text_similarity`.
const FUZZY_THRESHOLD: f64 = 0.7;

#[derive(Debug, Clone, Serialize)]
pub struct LibraryTrack {
    pub id: i64,
    pub job_id: Option<i64>,
    pub release_mbid: Option<String>,
    pub recording_id: Option<String>,
    pub artist: String,
    pub album: String,
    pub title: String,
    pub track_position: Option<i64>,
    pub disc_position: Option<i64>,
    pub year: Option<String>,
    pub path: String,
    pub format: Option<String>,
    pub imported_at: String,
}

pub(crate) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS library_tracks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id INTEGER REFERENCES jobs(id),
            release_mbid TEXT,
            recording_id TEXT,
            artist TEXT NOT NULL,
            album TEXT NOT NULL,
            title TEXT NOT NULL,
            track_position INTEGER,
            disc_position INTEGER,
            year TEXT,
            path TEXT NOT NULL UNIQUE,
            format TEXT,
            imported_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        CREATE INDEX IF NOT EXISTS library_tracks_artist ON library_tracks(artist);
        CREATE INDEX IF NOT EXISTS library_tracks_album ON library_tracks(album);",
    )?;
    return Ok(());
}

/// Upsert one imported file, keyed by `path`.
pub fn insert_track(
    conn: &Connection,
    job_id: Option<i64>,
    release: &NormalizedRelease,
    matched: &MatchedTrack,
    imported: &ImportedFile,
) -> Result<i64> {
    let artist = release
        .artist_credit
        .clone()
        .unwrap_or_else(|| return "Unknown Artist".to_string());
    let format = imported
        .dest
        .extension()
        .and_then(|e| return e.to_str())
        .map(|e| return e.to_string());
    let path = imported.dest.to_string_lossy().to_string();

    conn.execute(
        "INSERT INTO library_tracks
            (job_id, release_mbid, recording_id, artist, album, title, track_position, disc_position, year, path, format)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(path) DO UPDATE SET
            job_id = COALESCE(excluded.job_id, library_tracks.job_id),
            release_mbid = COALESCE(excluded.release_mbid, library_tracks.release_mbid),
            recording_id = COALESCE(excluded.recording_id, library_tracks.recording_id),
            artist = excluded.artist,
            album = excluded.album,
            title = excluded.title,
            track_position = excluded.track_position,
            disc_position = excluded.disc_position,
            year = excluded.year,
            format = excluded.format,
            imported_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![
            job_id,
            if release.id.is_empty() { None } else { Some(release.id.as_str()) },
            matched.recording_id,
            artist,
            release.title,
            matched.title,
            matched.position,
            matched.medium_position,
            release.year(),
            path,
            format,
        ],
    )?;
    return Ok(conn.query_row(
        "SELECT id FROM library_tracks WHERE path = ?1",
        params![path],
        |row| return row.get(0),
    )?);
}

pub fn get_track(conn: &Connection, id: i64) -> Result<LibraryTrack> {
    return conn
        .query_row(
            "SELECT * FROM library_tracks WHERE id = ?1",
            params![id],
            row_to_track,
        )
        .map_err(|e| {
            return match e {
                rusqlite::Error::QueryReturnedNoRows => Error::TrackNotFound { id },
                other => Error::Db(other),
            };
        });
}

/// Deletes rows by id; does not touch files on disk.
pub fn remove_tracks(conn: &Connection, ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = ids.iter().map(|_| return "?").collect::<Vec<_>>().join(",");
    conn.execute(
        &format!("DELETE FROM library_tracks WHERE id IN ({placeholders})"),
        params_from_iter(ids.iter()),
    )?;
    return Ok(());
}

/// All rows matching every clause (AND), sorted `artist, album, disc, track`.
///
/// Non-fuzzy clauses are pushed into SQL (ASCII case-folding) so the scan runs
/// in SQLite and can use the artist/album indexes; `~fuzzy` clauses can't be
/// expressed in SQL and are applied in memory over the reduced set.
pub fn list_tracks(conn: &Connection, query: &[Clause]) -> Result<Vec<LibraryTrack>> {
    let mut where_parts: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    let mut fuzzy: Vec<&Clause> = Vec::new();

    for clause in query {
        match clause.to_sql() {
            Some((frag, mut p)) => {
                where_parts.push(frag);
                params.append(&mut p);
            }
            None => fuzzy.push(clause),
        }
    }

    let mut sql = String::from("SELECT * FROM library_tracks");
    if !where_parts.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_parts.join(" AND "));
    }
    sql.push_str(" ORDER BY lower(artist), lower(album), disc_position, track_position");

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map(params_from_iter(params.iter()), row_to_track)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if !fuzzy.is_empty() {
        rows.retain(|t| return fuzzy.iter().all(|c| return c.matches(t)));
    }
    return Ok(rows);
}

fn row_to_track(row: &rusqlite::Row) -> rusqlite::Result<LibraryTrack> {
    return Ok(LibraryTrack {
        id: row.get("id")?,
        job_id: row.get("job_id")?,
        release_mbid: row.get("release_mbid")?,
        recording_id: row.get("recording_id")?,
        artist: row.get("artist")?,
        album: row.get("album")?,
        title: row.get("title")?,
        track_position: row.get("track_position")?,
        disc_position: row.get("disc_position")?,
        year: row.get("year")?,
        path: row.get("path")?,
        format: row.get("format")?,
        imported_at: row.get("imported_at")?,
    });
}

// --- Query language -------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Artist,
    Album,
    Title,
    Year,
    Track,
    Disc,
    Path,
    Release,
    Recording,
}

impl Field {
    fn from_prefix(s: &str) -> Option<Field> {
        return Some(match s.to_ascii_lowercase().as_str() {
            "artist" => Field::Artist,
            "album" => Field::Album,
            "title" => Field::Title,
            "year" => Field::Year,
            "track" => Field::Track,
            "disc" => Field::Disc,
            "path" => Field::Path,
            "release" => Field::Release,
            "recording" => Field::Recording,
            _ => return None,
        });
    }

    fn is_numeric(self) -> bool {
        return matches!(self, Field::Year | Field::Track | Field::Disc);
    }

    fn column(self) -> &'static str {
        return match self {
            Field::Artist => "artist",
            Field::Album => "album",
            Field::Title => "title",
            Field::Year => "year",
            Field::Track => "track_position",
            Field::Disc => "disc_position",
            Field::Path => "path",
            Field::Release => "release_mbid",
            Field::Recording => "recording_id",
        };
    }
}

#[derive(Debug, Clone)]
enum MatchKind {
    Substring(String),
    Exact(String),
    Fuzzy(String),
    Range(Option<i64>, Option<i64>),
}

#[derive(Debug, Clone)]
pub struct Clause {
    field: Option<Field>,
    negate: bool,
    kind: MatchKind,
}

impl Clause {
    fn matches(&self, t: &LibraryTrack) -> bool {
        let hit = match self.field {
            Some(f) => match_field(t, f, &self.kind),
            None => [&t.artist, &t.album, &t.title]
                .iter()
                .any(|v| return match_text(v, &self.kind)),
        };
        return hit != self.negate;
    }

    /// SQL fragment + bound params for this clause, or `None` for fuzzy
    /// (which SQL can't express and must stay in memory).
    fn to_sql(&self) -> Option<(String, Vec<Value>)> {
        if matches!(self.kind, MatchKind::Fuzzy(_)) {
            return None;
        }
        let cols: &[&str] = match self.field {
            Some(f) => &[f.column()],
            None => &["artist", "album", "title"],
        };
        let mut ors: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        for col in cols {
            let (frag, mut p) = kind_sql(col, &self.kind);
            ors.push(frag);
            params.append(&mut p);
        }
        let joined = if ors.len() == 1 {
            ors.pop().unwrap()
        } else {
            format!("({})", ors.join(" OR "))
        };
        let frag = if self.negate {
            format!("NOT ({joined})")
        } else {
            joined
        };
        return Some((frag, params));
    }
}

/// Builds a per-column SQL predicate for a non-fuzzy kind. Text matching is
/// ASCII case-insensitive via `lower()`; a NULL column never matches.
fn kind_sql(col: &str, kind: &MatchKind) -> (String, Vec<Value>) {
    return match kind {
        MatchKind::Substring(s) => (
            format!("{col} IS NOT NULL AND lower({col}) LIKE '%' || lower(?) || '%' ESCAPE '\\'"),
            vec![Value::Text(escape_like(s))],
        ),
        MatchKind::Exact(s) => (
            format!("{col} IS NOT NULL AND lower({col}) = lower(?)"),
            vec![Value::Text(s.clone())],
        ),
        MatchKind::Range(lo, hi) => {
            let expr = format!("CAST({col} AS INTEGER)");
            match (lo, hi) {
                (Some(lo), Some(hi)) => (
                    format!("{col} IS NOT NULL AND {expr} BETWEEN ? AND ?"),
                    vec![Value::Integer(*lo), Value::Integer(*hi)],
                ),
                (Some(lo), None) => (
                    format!("{col} IS NOT NULL AND {expr} >= ?"),
                    vec![Value::Integer(*lo)],
                ),
                (None, Some(hi)) => (
                    format!("{col} IS NOT NULL AND {expr} <= ?"),
                    vec![Value::Integer(*hi)],
                ),
                (None, None) => ("0".to_string(), Vec::new()),
            }
        }
        MatchKind::Fuzzy(_) => ("0".to_string(), Vec::new()),
    };
}

fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\\' || c == '%' || c == '_' {
            out.push('\\');
        }
        out.push(c);
    }
    return out;
}

fn match_text(value: &str, kind: &MatchKind) -> bool {
    return match kind {
        MatchKind::Substring(s) => value.to_lowercase().contains(&s.to_lowercase()),
        MatchKind::Exact(s) => value.eq_ignore_ascii_case(s),
        MatchKind::Fuzzy(s) => text_similarity(value, s) >= FUZZY_THRESHOLD,
        MatchKind::Range(..) => false, // ranges only apply to numeric fields
    };
}

fn match_field(t: &LibraryTrack, field: Field, kind: &MatchKind) -> bool {
    if let MatchKind::Range(lo, hi) = kind {
        let val = match field {
            Field::Year => t.year.as_deref().and_then(|y| return y.parse::<i64>().ok()),
            Field::Track => t.track_position,
            Field::Disc => t.disc_position,
            _ => None,
        };
        let Some(val) = val else { return false };
        return lo.is_none_or(|lo| return val >= lo) && hi.is_none_or(|hi| return val <= hi);
    }

    let text = match field {
        Field::Artist => Some(t.artist.clone()),
        Field::Album => Some(t.album.clone()),
        Field::Title => Some(t.title.clone()),
        Field::Year => t.year.clone(),
        Field::Track => t.track_position.map(|n| return n.to_string()),
        Field::Disc => t.disc_position.map(|n| return n.to_string()),
        Field::Path => Some(t.path.clone()),
        Field::Release => t.release_mbid.clone(),
        Field::Recording => t.recording_id.clone(),
    };
    return match text {
        Some(v) => match_text(&v, kind),
        None => false,
    };
}

/// Parses shell-tokenized terms. Per term: `-`/`^` negate, `field:` prefix,
/// `~`/`=`/`lo..hi`/substring value.
pub fn parse_query(terms: &[String]) -> Result<Vec<Clause>> {
    return terms.iter().map(|t| return parse_term(t)).collect();
}

fn parse_term(raw: &str) -> Result<Clause> {
    let mut s = raw;
    let mut negate = false;
    if s.starts_with("\\-") || s.starts_with("\\^") {
        s = &s[1..];
    } else if s.starts_with('-') || s.starts_with('^') {
        negate = true;
        s = &s[1..];
    }
    if s.is_empty() {
        return Err(Error::QuerySyntax {
            term: raw.to_string(),
            reason: "empty term",
        });
    }

    let (field, value) = match s.split_once(':') {
        Some((prefix, rest)) if !rest.is_empty() => match Field::from_prefix(prefix) {
            Some(f) => (Some(f), rest),
            None => {
                return Err(Error::QuerySyntax {
                    term: raw.to_string(),
                    reason: "unknown field prefix",
                });
            }
        },
        _ => (None, s),
    };

    let kind = if let Some(fuzzy) = value.strip_prefix('~') {
        MatchKind::Fuzzy(fuzzy.to_string())
    } else if let Some(exact) = value.strip_prefix('=') {
        MatchKind::Exact(exact.to_string())
    } else if field.is_some_and(|f| return f.is_numeric()) && value.contains("..") {
        let (lo, hi) = value.split_once("..").unwrap();
        let parse_bound = |b: &str| -> Result<Option<i64>> {
            if b.is_empty() {
                return Ok(None);
            }
            return b.parse::<i64>().map(Some).map_err(|_| {
                return Error::QuerySyntax {
                    term: raw.to_string(),
                    reason: "range bound is not an integer",
                };
            });
        };
        let lo = parse_bound(lo)?;
        let hi = parse_bound(hi)?;
        if lo.is_none() && hi.is_none() {
            return Err(Error::QuerySyntax {
                term: raw.to_string(),
                reason: "empty range",
            });
        }
        if let (Some(lo), Some(hi)) = (lo, hi)
            && lo > hi
        {
            return Err(Error::QuerySyntax {
                term: raw.to_string(),
                reason: "range start exceeds end",
            });
        }
        MatchKind::Range(lo, hi)
    } else {
        MatchKind::Substring(value.to_string())
    };

    return Ok(Clause {
        field,
        negate,
        kind,
    });
}

/// Deletes each track's row (atomically, before touching any file) and, if
/// `delete_files`, its file best-effort — a file that can't be removed is
/// warned and skipped rather than aborting, since the row is already gone.
/// Successfully deleted files' now-empty parent directories are pruned up to
/// (and excluding) `library_root` when given.
pub fn remove(
    conn: &Connection,
    tracks: &[LibraryTrack],
    delete_files: bool,
    library_root: Option<&Path>,
) -> Result<Vec<String>> {
    let ids: Vec<i64> = tracks.iter().map(|t| return t.id).collect();
    remove_tracks(conn, &ids)?;
    let mut deleted_files = Vec::new();
    if delete_files {
        for t in tracks {
            let path = Path::new(&t.path);
            match std::fs::remove_file(path) {
                Ok(()) => {
                    deleted_files.push(t.path.clone());
                    if let Some(parent) = path.parent() {
                        prune_empty_dirs(parent, library_root);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!("failed to delete {}: {e}", t.path),
            }
        }
    }
    return Ok(deleted_files);
}

fn prune_empty_dirs(mut dir: &Path, stop: Option<&Path>) {
    loop {
        if stop == Some(dir) {
            return;
        }
        if std::fs::remove_dir(dir).is_err() {
            return;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return,
        }
    }
}

// --- Edit -------------------------------------------------------------------

#[derive(Debug, Default, Serialize)]
pub struct TrackEdits {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<String>,
    pub track: Option<i64>,
    pub disc: Option<i64>,
}

impl TrackEdits {
    pub fn is_empty(&self) -> bool {
        return self.title.is_none()
            && self.artist.is_none()
            && self.album.is_none()
            && self.year.is_none()
            && self.track.is_none()
            && self.disc.is_none();
    }
}

/// Parses `field=value` pairs. Editable fields are exactly the indexed ones
/// that also live in the file's tags: `title`, `artist`, `album`, `year`,
/// `track`, `disc`.
pub fn parse_edits(pairs: &[String]) -> Result<TrackEdits> {
    let mut edits = TrackEdits::default();
    for pair in pairs {
        let Some((field, value)) = pair.split_once('=') else {
            return Err(Error::QuerySyntax {
                term: pair.clone(),
                reason: "expected field=value",
            });
        };
        if value.is_empty() {
            return Err(Error::QuerySyntax {
                term: pair.clone(),
                reason: "empty value",
            });
        }
        match field.to_ascii_lowercase().as_str() {
            "title" => edits.title = Some(value.to_string()),
            "artist" => edits.artist = Some(value.to_string()),
            "album" => edits.album = Some(value.to_string()),
            "year" => edits.year = Some(value.to_string()),
            "track" => edits.track = Some(parse_edit_number(pair, value)?),
            "disc" => edits.disc = Some(parse_edit_number(pair, value)?),
            _ => {
                return Err(Error::QuerySyntax {
                    term: pair.clone(),
                    reason: "unknown field (title/artist/album/year/track/disc)",
                });
            }
        }
    }
    return Ok(edits);
}

fn parse_edit_number(term: &str, value: &str) -> Result<i64> {
    return value.parse::<i64>().map_err(|_| {
        return Error::QuerySyntax {
            term: term.to_string(),
            reason: "not a number",
        };
    });
}

/// Applies the edits to the index row (and `new_path` when the file was
/// renamed). Returns the updated row.
pub fn edit_track(
    conn: &Connection,
    id: i64,
    edits: &TrackEdits,
    new_path: Option<&str>,
) -> Result<LibraryTrack> {
    let mut sets: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    let mut set = |col: &str, v: Value| {
        sets.push(format!("{col} = ?{}", values.len() + 1));
        values.push(v);
    };
    if let Some(v) = &edits.title {
        set("title", Value::Text(v.clone()));
    }
    if let Some(v) = &edits.artist {
        set("artist", Value::Text(v.clone()));
    }
    if let Some(v) = &edits.album {
        set("album", Value::Text(v.clone()));
    }
    if let Some(v) = &edits.year {
        set("year", Value::Text(v.clone()));
    }
    if let Some(v) = edits.track {
        set("track_position", Value::Integer(v));
    }
    if let Some(v) = edits.disc {
        set("disc_position", Value::Integer(v));
    }
    if let Some(p) = new_path {
        set("path", Value::Text(p.to_string()));
    }
    if !sets.is_empty() {
        values.push(Value::Integer(id));
        let sql = format!(
            "UPDATE library_tracks SET {} WHERE id = ?{}",
            sets.join(", "),
            values.len()
        );
        conn.execute(&sql, params_from_iter(values.iter()))?;
    }
    return get_track(conn, id);
}

/// Rewrites the edited fields in the file's tags — a patch, not a fresh tag.
/// VorbisComments (FLAC/Opus, i.e. this whole library) is written directly,
/// same reason import writes it directly (lofty's generic `ItemKey` path has
/// no mapping for several Vorbis fields); other formats patch their primary
/// tag through the generic API.
pub fn rewrite_tags(path: &Path, edits: &TrackEdits) -> Result<()> {
    if edits.is_empty() {
        return Ok(());
    }
    let ext = path
        .extension()
        .and_then(|e| return e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "flac" => {
            let mut file = std::fs::File::open(path).map_err(|e| return Error::io(path, e))?;
            let mut f = FlacFile::read_from(&mut file, ParseOptions::new()).map_err(|e| {
                return Error::Probe {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                };
            })?;
            if let Some(vc) = f.vorbis_comments_mut() {
                fill_vorbis_edits(vc, edits);
            }
            f.save_to_path(path, WriteOptions::default()).map_err(|e| {
                return Error::Probe {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                };
            })?;
        }
        "opus" => {
            let mut file = std::fs::File::open(path).map_err(|e| return Error::io(path, e))?;
            let mut f = OpusFile::read_from(&mut file, ParseOptions::new()).map_err(|e| {
                return Error::Probe {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                };
            })?;
            fill_vorbis_edits(f.vorbis_comments_mut(), edits);
            f.save_to_path(path, WriteOptions::default()).map_err(|e| {
                return Error::Probe {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                };
            })?;
        }
        _ => {
            let mut tagged = read_from_path(path).map_err(|e| {
                return Error::Probe {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                };
            })?;
            match tagged.primary_tag_mut() {
                Some(tag) => fill_generic_edits(tag, edits),
                None => {
                    let mut tag = Tag::new(tagged.primary_tag_type());
                    fill_generic_edits(&mut tag, edits);
                    let _ = tagged.insert_tag(tag);
                }
            }
            tagged.save_to_path(path, WriteOptions::default()).map_err(|e| {
                return Error::Probe {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                };
            })?;
        }
    }
    return Ok(());
}

fn fill_vorbis_edits(vc: &mut lofty::ogg::VorbisComments, edits: &TrackEdits) {
    if let Some(v) = &edits.title {
        vc.set_title(v.clone());
    }
    if let Some(v) = &edits.artist {
        vc.set_artist(v.clone());
        // Keep grouping fields in sync so players don't surface a stale
        // ALBUMARTIST/ARTISTS next to the edited ARTIST (see import::fill_vorbis).
        vc.insert("ALBUMARTIST".to_string(), v.clone());
        vc.insert("ALBUM ARTIST".to_string(), v.clone());
        vc.insert("ARTISTS".to_string(), v.clone());
    }
    if let Some(v) = &edits.album {
        vc.set_album(v.clone());
    }
    if let Some(v) = &edits.year {
        vc.insert("DATE".to_string(), v.clone());
    }
    if let Some(v) = edits.track {
        vc.set_track(v.max(0) as u32);
    }
    if let Some(v) = edits.disc {
        vc.set_disk(v.max(0) as u32);
    }
}

fn fill_generic_edits(tag: &mut Tag, edits: &TrackEdits) {
    if let Some(v) = &edits.title {
        tag.set_title(v.clone());
    }
    if let Some(v) = &edits.artist {
        tag.set_artist(v.clone());
        let _ = tag.insert_text(ItemKey::AlbumArtist, v.clone());
        let _ = tag.insert_text(ItemKey::TrackArtists, v.clone());
    }
    if let Some(v) = &edits.album {
        tag.set_album(v.clone());
    }
    if let Some(v) = &edits.year {
        let _ = tag.insert_text(ItemKey::RecordingDate, v.clone());
    }
    if let Some(v) = edits.track {
        tag.set_track(v.max(0) as u32);
    }
    if let Some(v) = edits.disc {
        tag.set_disk(v.max(0) as u32);
    }
}

/// Filename the track would carry under the naming scheme, as a full path in
/// the same directory; `None` when it can't be derived (no track position) or
/// already matches. Preserves the `<disc>-<track>` prefix style when the
/// current filename uses it.
pub fn planned_rename(track: &LibraryTrack) -> Option<String> {
    let path = Path::new(&track.path);
    let file_name = path.file_name()?.to_str()?;
    let (stem, ext) = file_name.rsplit_once('.')?;
    let track_num = track.track_position?;
    if track_num <= 0 || track_num > 99 {
        return None;
    }

    let head = stem.split(" - ").next().unwrap_or("");
    let parts: Vec<&str> = head.split('-').collect();
    let current_is_multi = parts.len() == 2
        && parts
            .iter()
            .all(|p| return p.len() == 2 && p.bytes().all(|b| return b.is_ascii_digit()));
    let new_head = if current_is_multi {
        let disc = track.disc_position.unwrap_or(1).clamp(1, 99);
        format!("{disc:02}-{track_num:02}")
    } else {
        format!("{track_num:02}")
    };

    let new_file_name = format!("{new_head} - {}.{ext}", sanitize(&track.title));
    if new_file_name == file_name {
        return None;
    }
    return Some(path.with_file_name(new_file_name).to_string_lossy().to_string());
}
