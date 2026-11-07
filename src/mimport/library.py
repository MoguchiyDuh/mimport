from __future__ import annotations

import json
import logging
import re
import sqlite3
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Sequence

from .errors import MimportError, TrackNotFound

FUZZY_THRESHOLD = 0.7

TRACK_COLUMNS = (
    "id",
    "job_id",
    "release_mbid",
    "recording_id",
    "source_url",
    "artist",
    "album",
    "title",
    "track_position",
    "disc_position",
    "year",
    "path",
    "format",
    "imported_at",
)

_FIELD_COLUMNS = {
    "artist": "artist",
    "album": "album",
    "title": "title",
    "year": "year",
    "track": "track_position",
    "disc": "disc_position",
    "path": "path",
    "release": "release_mbid",
    "recording": "recording_id",
}

_NUMERIC_FIELDS = {"year", "track", "disc"}
_ASCII_UPPER = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
_ASCII_LOWER = "abcdefghijklmnopqrstuvwxyz"
_ASCII_FOLD = str.maketrans(_ASCII_UPPER, _ASCII_LOWER)
_I64_RE = re.compile(r"^[+-]?[0-9]+$")
_I64_MIN = -(2**63)
_I64_MAX = 2**63 - 1


def ensure_schema(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS library_tracks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id INTEGER REFERENCES jobs(id),
            release_mbid TEXT,
            recording_id TEXT,
            source_url TEXT,
            artist TEXT NOT NULL,
            album TEXT NOT NULL,
            title TEXT NOT NULL,
            track_position INTEGER,
            disc_position INTEGER,
            year TEXT,
            path TEXT NOT NULL UNIQUE,
            format TEXT,
            imported_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )
        """
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS library_tracks_artist ON library_tracks(artist)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS library_tracks_album ON library_tracks(album)"
    )
    present = conn.execute(
        "SELECT COUNT(*) FROM pragma_table_info('library_tracks') WHERE name = 'source_url'"
    ).fetchone()[0]
    if not present:
        conn.execute("ALTER TABLE library_tracks ADD COLUMN source_url TEXT;")
        _backfill_source_urls(conn)


def _backfill_source_urls(conn: sqlite3.Connection) -> None:
    from . import metadata

    rows = conn.execute("SELECT id, path FROM library_tracks").fetchall()
    filled = 0
    for track_id, path in rows:
        if Path(path).suffix.lower() not in (".opus", ".flac"):
            continue
        try:
            url = metadata.read_source_url(Path(path))
        except Exception:
            continue
        if url is None:
            continue
        conn.execute(
            "UPDATE library_tracks SET source_url = ? WHERE id = ?",
            (url, track_id),
        )
        filled += 1
    if filled:
        logging.info("backfilled source_url for %d library row(s) from file tags", filled)


def _row_to_track(row: Sequence[Any]) -> dict[str, Any]:
    return dict(zip(TRACK_COLUMNS, row))


def _rust_extension(name: str) -> str | None:
    if name in ("", ".", ".."):
        return None
    if name.startswith("."):
        rest = name[1:]
        if "." not in rest:
            return None
        return rest.rsplit(".", 1)[1]
    if "." not in name:
        return None
    return name.rsplit(".", 1)[1]


def _release_year(release: dict[str, Any]) -> str | None:
    date = release.get("date")
    if date is None:
        return None
    return str(date).split("-", 1)[0]


def insert_track(
    conn: sqlite3.Connection,
    job_id: int | None,
    release: dict[str, Any],
    matched: dict[str, Any],
    imported: dict[str, Any],
) -> int:
    artist = release.get("artist_credit")
    if artist is None:
        artist = "Unknown Artist"
    dest = Path(imported["dest"])
    path = str(dest)
    fmt = _rust_extension(dest.name)
    release_id = release.get("id")
    release_mbid = None if not release_id else str(release_id)
    conn.execute(
        """
        INSERT INTO library_tracks
            (job_id, release_mbid, recording_id, source_url, artist, album, title,
             track_position, disc_position, year, path, format)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(path) DO UPDATE SET
            job_id = COALESCE(excluded.job_id, library_tracks.job_id),
            release_mbid = COALESCE(excluded.release_mbid, library_tracks.release_mbid),
            recording_id = COALESCE(excluded.recording_id, library_tracks.recording_id),
            source_url = COALESCE(excluded.source_url, library_tracks.source_url),
            artist = excluded.artist,
            album = excluded.album,
            title = excluded.title,
            track_position = excluded.track_position,
            disc_position = excluded.disc_position,
            year = excluded.year,
            format = excluded.format,
            imported_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
        """,
        (
            job_id,
            release_mbid,
            matched.get("recording_id"),
            matched.get("source_url"),
            artist,
            release["title"],
            matched["title"],
            matched.get("position"),
            matched.get("medium_position"),
            _release_year(release),
            path,
            fmt,
        ),
    )
    row = conn.execute("SELECT id FROM library_tracks WHERE path = ?", (path,)).fetchone()
    if row is None:
        raise MimportError(f"library row for path disappeared after insert: {path}")
    return int(row[0])


def get_track(conn: sqlite3.Connection, id: int) -> dict[str, Any]:
    row = conn.execute(
        f"SELECT {', '.join(TRACK_COLUMNS)} FROM library_tracks WHERE id = ?",
        (id,),
    ).fetchone()
    if row is None:
        raise TrackNotFound(id)
    return _row_to_track(row)


def _remove_ids(conn: sqlite3.Connection, ids: Sequence[int]) -> None:
    if not ids:
        return
    placeholders = ",".join("?" for _ in ids)
    conn.execute(
        f"DELETE FROM library_tracks WHERE id IN ({placeholders})",
        tuple(ids),
    )


def _nfc(value: str) -> str:
    return unicodedata.normalize("NFC", value)


def _nfd(value: str) -> str:
    return unicodedata.normalize("NFD", value)


def _escape_like(value: str) -> str:
    out: list[str] = []
    for char in value:
        if char in ("\\", "%", "_"):
            out.append("\\")
        out.append(char)
    return "".join(out)


def _ascii_lower(value: str) -> str:
    return value.translate(_ASCII_FOLD)


def _try_i64(value: str) -> int | None:
    if not _I64_RE.fullmatch(value):
        return None
    number = int(value)
    if number < _I64_MIN or number > _I64_MAX:
        return None
    return number


def _text_similarity(a: str, b: str) -> float:
    from .scorer import text_similarity

    return text_similarity(a, b)


@dataclass(frozen=True)
class Clause:
    field: str | None
    negate: bool
    kind: str
    text: str | None = None
    lo: int | None = None
    hi: int | None = None

    def matches(self, track: dict[str, Any]) -> bool:
        if self.field is None:
            hit = any(
                _match_text(str(track[column]), self)
                for column in ("artist", "album", "title")
            )
        else:
            hit = _match_field(track, self.field, self)
        return hit != self.negate

    def to_sql(self) -> tuple[str, list[Any]] | None:
        if self.kind == "fuzzy":
            return None
        columns = (
            (_FIELD_COLUMNS[self.field],)
            if self.field is not None
            else ("artist", "album", "title")
        )
        fragments: list[str] = []
        params: list[Any] = []
        for column in columns:
            fragment, values = _kind_sql(column, self)
            fragments.append(fragment)
            params.extend(values)
        joined = fragments[0] if len(fragments) == 1 else f"({' OR '.join(fragments)})"
        if self.negate:
            return f"NOT ({joined})", params
        return joined, params


def _kind_sql(column: str, clause: Clause) -> tuple[str, list[Any]]:
    is_path = clause.field == "path"
    text = clause.text or ""
    if clause.kind == "substring":
        if is_path:
            return (
                f"{column} IS NOT NULL AND ({column} LIKE '%' || ? || '%' ESCAPE '\\' "
                f"OR {column} LIKE '%' || ? || '%' ESCAPE '\\')",
                [_escape_like(_nfc(text)), _escape_like(_nfd(text))],
            )
        return (
            f"{column} IS NOT NULL AND lower({column}) LIKE '%' || lower(?) || '%' ESCAPE '\\'",
            [_escape_like(text)],
        )
    if clause.kind == "exact":
        if is_path:
            return (
                f"{column} IS NOT NULL AND ({column} = ? OR {column} = ?)",
                [_nfc(text), _nfd(text)],
            )
        return (
            f"{column} IS NOT NULL AND lower({column}) = lower(?)",
            [text],
        )
    if clause.kind == "range":
        expression = f"CAST({column} AS INTEGER)"
        if clause.lo is not None and clause.hi is not None:
            return (
                f"{column} IS NOT NULL AND {expression} BETWEEN ? AND ?",
                [clause.lo, clause.hi],
            )
        if clause.lo is not None:
            return (
                f"{column} IS NOT NULL AND {expression} >= ?",
                [clause.lo],
            )
        if clause.hi is not None:
            return (
                f"{column} IS NOT NULL AND {expression} <= ?",
                [clause.hi],
            )
        return "0", []
    return "0", []


def _match_text(value: str, clause: Clause) -> bool:
    text = clause.text or ""
    if clause.kind == "substring":
        return text.lower() in value.lower()
    if clause.kind == "exact":
        return _ascii_lower(value) == _ascii_lower(text)
    if clause.kind == "fuzzy":
        return _text_similarity(value, text) >= FUZZY_THRESHOLD
    return False


def _match_path(value: str, clause: Clause) -> bool:
    text = clause.text or ""
    if clause.kind == "substring":
        return _nfc(text) in value or _nfd(text) in value
    if clause.kind == "exact":
        return value == _nfc(text) or value == _nfd(text)
    if clause.kind == "fuzzy":
        return _text_similarity(value, text) >= FUZZY_THRESHOLD
    return False


def _match_field(track: dict[str, Any], field: str, clause: Clause) -> bool:
    if clause.kind == "range":
        if field == "year":
            year = track["year"]
            value = None if year is None else _try_i64(str(year))
        elif field == "track":
            value = track["track_position"]
        elif field == "disc":
            value = track["disc_position"]
        else:
            value = None
        if value is None:
            return False
        if clause.lo is not None and value < clause.lo:
            return False
        if clause.hi is not None and value > clause.hi:
            return False
        return True

    if field == "artist":
        text = track["artist"]
    elif field == "album":
        text = track["album"]
    elif field == "title":
        text = track["title"]
    elif field == "year":
        text = track["year"]
    elif field == "track":
        position = track["track_position"]
        text = None if position is None else str(position)
    elif field == "disc":
        position = track["disc_position"]
        text = None if position is None else str(position)
    elif field == "path":
        text = track["path"]
    elif field == "release":
        text = track["release_mbid"]
    elif field == "recording":
        text = track["recording_id"]
    else:
        text = None
    if text is None:
        return False
    if field == "path":
        return _match_path(str(text), clause)
    return _match_text(str(text), clause)


def _query_error(term: str, reason: str) -> MimportError:
    return MimportError(
        f"invalid library query term {json.dumps(term, ensure_ascii=False)}: {reason}"
    )


def _field_from_prefix(prefix: str) -> str | None:
    key = _ascii_lower(prefix)
    return key if key in _FIELD_COLUMNS else None


def parse_query(terms: Sequence[str]) -> list[Clause]:
    return [_parse_term(term) for term in terms]


def _parse_term(raw: str) -> Clause:
    value = raw
    negate = False
    if value.startswith("\\-") or value.startswith("\\^"):
        value = value[1:]
    elif value.startswith("-") or value.startswith("^"):
        negate = True
        value = value[1:]
    if not value:
        raise _query_error(raw, "empty term")

    field: str | None = None
    prefix, separator, rest = value.partition(":")
    if separator and rest:
        field = _field_from_prefix(prefix)
        if field is None:
            raise _query_error(raw, "unknown field prefix")
        value = rest

    if value.startswith("~"):
        return Clause(field, negate, "fuzzy", text=value[1:])
    if value.startswith("="):
        return Clause(field, negate, "exact", text=value[1:])
    if field in _NUMERIC_FIELDS and ".." in value:
        lo_text, hi_text = value.split("..", 1)
        lo = None if lo_text == "" else _try_i64(lo_text)
        hi = None if hi_text == "" else _try_i64(hi_text)
        if lo_text != "" and lo is None:
            raise _query_error(raw, "range bound is not an integer")
        if hi_text != "" and hi is None:
            raise _query_error(raw, "range bound is not an integer")
        if lo is None and hi is None:
            raise _query_error(raw, "empty range")
        if lo is not None and hi is not None and lo > hi:
            raise _query_error(raw, "range start exceeds end")
        return Clause(field, negate, "range", lo=lo, hi=hi)
    return Clause(field, negate, "substring", text=value)


def _coerce_clauses(query: Sequence[str] | Sequence[Clause] | None) -> list[Clause]:
    if query is None:
        return []
    clauses = list(query)
    if all(isinstance(clause, Clause) for clause in clauses):
        return clauses
    if all(isinstance(clause, str) for clause in clauses):
        return parse_query(clauses)
    raise MimportError("library query must contain only strings or parsed clauses")


def list_tracks(
    conn: sqlite3.Connection,
    query: Sequence[str] | Sequence[Clause] | None,
) -> list[dict[str, Any]]:
    clauses = _coerce_clauses(query)
    where_parts: list[str] = []
    params: list[Any] = []
    fuzzy: list[Clause] = []
    for clause in clauses:
        sql = clause.to_sql()
        if sql is None:
            fuzzy.append(clause)
        else:
            fragment, values = sql
            where_parts.append(fragment)
            params.extend(values)
    sql = f"SELECT {', '.join(TRACK_COLUMNS)} FROM library_tracks"
    if where_parts:
        sql += " WHERE " + " AND ".join(where_parts)
    sql += " ORDER BY lower(artist), lower(album), disc_position, track_position"
    tracks = [_row_to_track(row) for row in conn.execute(sql, params).fetchall()]
    if fuzzy:
        tracks = [
            track
            for track in tracks
            if all(clause.matches(track) for clause in fuzzy)
        ]
    total = len(tracks)
    tracks = [track for track in tracks if Path(track["path"]).exists()]
    if len(tracks) < total:
        logging.warning(
            "%d index row(s) reference missing files and were hidden; run `mimport library prune` to drop them",
            total - len(tracks),
        )
    return tracks


def missing_tracks(conn: sqlite3.Connection) -> list[dict[str, Any]]:
    rows = conn.execute(
        f"SELECT {', '.join(TRACK_COLUMNS)} FROM library_tracks ORDER BY lower(path)"
    ).fetchall()
    tracks = [_row_to_track(row) for row in rows]
    return [track for track in tracks if not Path(track["path"]).exists()]


def prune_missing(
    conn: sqlite3.Connection,
    dry_run: bool = False,
) -> list[dict[str, Any]]:
    missing = missing_tracks(conn)
    if dry_run:
        return missing
    _remove_ids(conn, [track["id"] for track in missing])
    return missing


def sanitize(value: str) -> str:
    cleaned = "".join(
        "_" if char in '/\\:*?"<>|' else char
        for char in unicodedata.normalize("NFC", value)
    )
    return cleaned.strip().lstrip(".").rstrip(".")


def prune_empty_dirs(start: Path | str, stop: Path | str | None) -> None:
    if stop is None:
        return
    root = Path(stop).resolve()
    current = Path(start)
    while True:
        resolved = current.resolve()
        if resolved == root or not resolved.is_relative_to(root):
            return
        try:
            current.rmdir()
        except OSError:
            return
        parent = current.parent
        if parent == current:
            return
        current = parent


def _delete_tracks(
    tracks: Sequence[dict[str, Any]],
    delete_files: bool,
    library_root: Path | str | None,
) -> list[str]:
    if not delete_files:
        return []
    deleted: list[str] = []
    root = None if library_root is None else Path(library_root)
    for track in tracks:
        path = Path(track["path"])
        try:
            path.unlink()
        except FileNotFoundError:
            continue
        except OSError as exc:
            logging.warning("failed to delete %s: %s", track["path"], exc)
            continue
        deleted.append(track["path"])
        prune_empty_dirs(path.parent, root)
    return deleted


def remove(
    conn: sqlite3.Connection,
    tracks: Sequence[dict[str, Any]],
    delete_files: bool,
    library_root: Path | str | None,
) -> list[str]:
    _remove_ids(conn, [track["id"] for track in tracks])
    return _delete_tracks(tracks, delete_files, library_root)


def remove_tracks(
    conn: sqlite3.Connection,
    query: Sequence[str] | Sequence[Clause] | None,
    delete_files: bool = False,
    library_root: Path | str | None = None,
) -> tuple[list[dict[str, Any]], list[str]]:
    tracks = list_tracks(conn, query)
    deleted = remove(conn, tracks, delete_files, library_root)
    return tracks, deleted


def _edits_error(term: str, reason: str) -> MimportError:
    return MimportError(
        f"invalid library query term {json.dumps(term, ensure_ascii=False)}: {reason}"
    )


def parse_edits(fields: Sequence[str]) -> dict[str, Any]:
    edits: dict[str, Any] = {
        "title": None,
        "artist": None,
        "album_artist": None,
        "album": None,
        "year": None,
        "track": None,
        "disc": None,
        "compilation": None,
        "genre": None,
    }
    for pair in fields:
        field, separator, value = pair.partition("=")
        if not separator:
            raise _edits_error(pair, "expected field=value")
        if value == "":
            raise _edits_error(pair, "empty value")
        key = _ascii_lower(field)
        if key == "title":
            edits["title"] = value
        elif key == "artist":
            edits["artist"] = value
        elif key == "album_artist":
            edits["album_artist"] = value
        elif key == "album":
            edits["album"] = value
        elif key == "year":
            edits["year"] = value
        elif key == "track":
            edits["track"] = _parse_edit_number(pair, value)
        elif key == "disc":
            edits["disc"] = _parse_edit_number(pair, value)
        elif key == "compilation":
            edits["compilation"] = _parse_edit_bool(pair, value)
        elif key == "genre":
            if edits["genre"] is None:
                edits["genre"] = []
            edits["genre"].append(value)
        else:
            raise _edits_error(
                pair,
                "unknown field (title/artist/album_artist/album/year/track/disc/compilation/genre)",
            )
    return edits


def _parse_edit_number(term: str, value: str) -> int:
    number = _try_i64(value)
    if number is None:
        raise _edits_error(term, "not a number")
    return number


def _parse_edit_bool(term: str, value: str) -> bool:
    lowered = _ascii_lower(value)
    if lowered in ("1", "true", "yes"):
        return True
    if lowered in ("0", "false", "no"):
        return False
    raise _edits_error(term, "expected 1/0/true/false")


def edits_empty(edits: dict[str, Any]) -> bool:
    return all(value is None for value in edits.values())


def edit_track(
    conn: sqlite3.Connection,
    id: int,
    edits: dict[str, Any],
    new_path: str | Path | None = None,
) -> dict[str, Any]:
    sets: list[str] = []
    values: list[Any] = []
    if edits.get("title") is not None:
        sets.append("title = ?")
        values.append(edits["title"])
    if edits.get("artist") is not None:
        sets.append("artist = ?")
        values.append(edits["artist"])
    if edits.get("album") is not None:
        sets.append("album = ?")
        values.append(edits["album"])
    if edits.get("year") is not None:
        sets.append("year = ?")
        values.append(edits["year"])
    if edits.get("track") is not None:
        sets.append("track_position = ?")
        values.append(edits["track"])
    if edits.get("disc") is not None:
        sets.append("disc_position = ?")
        values.append(edits["disc"])
    if new_path is not None:
        sets.append("path = ?")
        values.append(str(new_path))
    if sets:
        values.append(id)
        conn.execute(
            f"UPDATE library_tracks SET {', '.join(sets)} WHERE id = ?",
            tuple(values),
        )
    return get_track(conn, id)


def rewrite_tags(path: str | Path, edits: dict[str, Any]) -> None:
    if edits_empty(edits):
        return
    from . import metadata

    metadata.rewrite_tags(Path(path), edits)


def renamed_path(
    track: dict[str, Any],
    library_root: str | Path,
    edits: dict[str, Any] | None = None,
) -> Path:
    changes = edits or {}
    artist = changes.get("artist")
    if artist is None:
        artist = track["artist"]
    album = changes.get("album")
    if album is None:
        album = track["album"]
    title = changes.get("title")
    if title is None:
        title = track["title"]
    year = changes.get("year")
    if year is None:
        year = track["year"]
    track_number = changes.get("track")
    if track_number is None:
        track_number = track["track_position"]
    disc = changes.get("disc")
    if disc is None:
        disc = track["disc_position"]
    if disc is None:
        disc = 1
    disc = max(1, min(99, int(disc)))

    artist_dir = sanitize(str(artist))
    album_dir = sanitize(str(album)) if not year else sanitize(f"{album} ({year})")

    path = Path(track["path"])
    file_name = path.name
    if "." not in file_name:
        return path
    stem, extension = file_name.rsplit(".", 1)
    head = stem.split(" - ", 1)[0]
    parts = head.split("-")
    current_is_multi = len(parts) == 2 and all(
        len(part) == 2 and part.isascii() and part.isdigit() for part in parts
    )
    if track_number is not None and int(track_number) >= 1:
        if current_is_multi:
            new_head = f"{disc:02d}-{int(track_number):02d}"
        else:
            new_head = f"{int(track_number):02d}"
    else:
        new_head = head
    new_file_name = f"{new_head} - {sanitize(str(title))}.{extension}"
    return Path(library_root) / artist_dir / album_dir / new_file_name
