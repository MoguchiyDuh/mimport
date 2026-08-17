//! Library index + query language over `library_tracks`.

use std::path::Path;

use rusqlite::{params, params_from_iter, Connection};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::import::{ImportedFile, MatchedTrack};
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
pub fn insert_track(conn: &Connection, job_id: Option<i64>, release: &NormalizedRelease, matched: &MatchedTrack, imported: &ImportedFile) -> Result<i64> {
    let artist = release.artist_credit.clone().unwrap_or_else(|| return "Unknown Artist".to_string());
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
            job_id = excluded.job_id,
            release_mbid = excluded.release_mbid,
            recording_id = excluded.recording_id,
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
    return Ok(conn.query_row("SELECT id FROM library_tracks WHERE path = ?1", params![path], |row| return row.get(0))?);
}

pub fn get_track(conn: &Connection, id: i64) -> Result<LibraryTrack> {
    return conn
        .query_row("SELECT * FROM library_tracks WHERE id = ?1", params![id], row_to_track)
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
pub fn list_tracks(conn: &Connection, query: &[Clause]) -> Result<Vec<LibraryTrack>> {
    let mut stmt = conn.prepare("SELECT * FROM library_tracks")?;
    let mut rows = stmt
        .query_map([], row_to_track)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.retain(|t| return query.iter().all(|c| return c.matches(t)));
    rows.sort_by(|a, b| {
        return (a.artist.to_lowercase(), a.album.to_lowercase(), a.disc_position, a.track_position).cmp(&(
            b.artist.to_lowercase(),
            b.album.to_lowercase(),
            b.disc_position,
            b.track_position,
        ));
    });
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
            None => [&t.artist, &t.album, &t.title].iter().any(|v| return match_text(v, &self.kind)),
        };
        return hit != self.negate;
    }
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
        if let (Some(lo), Some(hi)) = (lo, hi) {
            if lo > hi {
                return Err(Error::QuerySyntax {
                    term: raw.to_string(),
                    reason: "range start exceeds end",
                });
            }
        }
        MatchKind::Range(lo, hi)
    } else {
        MatchKind::Substring(value.to_string())
    };

    return Ok(Clause { field, negate, kind });
}

/// Deletes each track's row and, if `delete_files`, its file (best-effort).
pub fn remove(conn: &Connection, tracks: &[LibraryTrack], delete_files: bool) -> Result<Vec<String>> {
    let mut deleted_files = Vec::new();
    if delete_files {
        for t in tracks {
            let path = Path::new(&t.path);
            match std::fs::remove_file(path) {
                Ok(()) => deleted_files.push(t.path.clone()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::io(path, e)),
            }
        }
    }
    let ids: Vec<i64> = tracks.iter().map(|t| return t.id).collect();
    remove_tracks(conn, &ids)?;
    return Ok(deleted_files);
}
