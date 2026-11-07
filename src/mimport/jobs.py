from __future__ import annotations

import re
import sqlite3
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator
from urllib.parse import quote

from . import library
from .errors import JobNotFound, MimportError, io_error

STATUS_INCOMPLETE = "incomplete"
STATUS_FETCHED = "fetched"
STATUS_PARTIAL = "partial"
STATUS_FAILED = "failed"
STATUS_CANCELLED = "cancelled"
STATUS_IMPORTED = "imported"

_JOB_COLUMNS = (
    "id",
    "title",
    "status",
    "search_id",
    "username",
    "directory",
    "local_dir",
    "created_at",
    "updated_at",
)

_JOB_FILE_COLUMNS = (
    "id",
    "job_id",
    "remote_path",
    "transfer_id",
    "size",
    "state",
    "local_path",
)

_I64_RE = re.compile(r"^[+-]?[0-9]+$")
_I64_MIN = -(2**63)
_I64_MAX = 2**63 - 1


def open(path: str | Path) -> sqlite3.Connection:
    db_path = Path(path)
    parent = db_path.parent
    try:
        parent.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        raise io_error(parent, exc) from exc
    conn = sqlite3.connect(str(db_path), isolation_level=None, timeout=0)
    try:
        _ensure_schema(conn)
        library.ensure_schema(conn)
    except BaseException:
        conn.close()
        raise
    return conn


def open_readonly(path: str | Path) -> sqlite3.Connection:
    db_path = Path(path).resolve()
    uri = f"file:{quote(str(db_path), safe='/')}?mode=ro"
    return sqlite3.connect(uri, uri=True, isolation_level=None, timeout=0)


@contextmanager
def transaction(conn: sqlite3.Connection) -> Iterator[sqlite3.Connection]:
    if conn.in_transaction:
        name = f"mimport_{id(conn):x}"
        conn.execute(f"SAVEPOINT {name}")
        try:
            yield conn
        except BaseException:
            try:
                conn.execute(f"ROLLBACK TO {name}")
                conn.execute(f"RELEASE {name}")
            except sqlite3.Error:
                pass
            raise
        try:
            conn.execute(f"RELEASE {name}")
        except BaseException:
            try:
                conn.execute(f"ROLLBACK TO {name}")
                conn.execute(f"RELEASE {name}")
            except sqlite3.Error:
                pass
            raise
        return
    conn.execute("BEGIN IMMEDIATE")
    try:
        yield conn
    except BaseException:
        try:
            conn.execute("ROLLBACK")
        except sqlite3.Error:
            pass
        raise
    try:
        conn.execute("COMMIT")
    except BaseException:
        if conn.in_transaction:
            try:
                conn.execute("ROLLBACK")
            except sqlite3.Error:
                pass
        raise


def _ensure_schema(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS jobs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            status TEXT NOT NULL,
            search_id TEXT NOT NULL,
            username TEXT NOT NULL,
            directory TEXT NOT NULL,
            local_dir TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )
        """
    )
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS job_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id INTEGER NOT NULL REFERENCES jobs(id),
            remote_path TEXT NOT NULL,
            transfer_id TEXT NOT NULL,
            size INTEGER NOT NULL,
            state TEXT NOT NULL,
            local_path TEXT NOT NULL,
            UNIQUE(job_id, transfer_id)
        )
        """
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS job_files_job_id ON job_files(job_id)"
    )


def _remote_basename(remote: str) -> str:
    return remote.rsplit("\\", 1)[-1]


def _sanitize_dir_segment(value: str) -> str:
    return "".join("_" if ch in '/\\:*?"<>|' else ch for ch in value).strip()


def _contained_join(root: Path, parts: list[str], what: str) -> Path:
    candidate = root.joinpath(*parts)
    root_resolved = root.resolve()
    candidate_resolved = candidate.resolve()
    if not candidate_resolved.is_relative_to(root_resolved):
        raise MimportError(f"unsafe {what} path component escapes {root}: {candidate}")
    return candidate


def local_dir_for(downloads_root: Path, username: str, remote_directory: str) -> Path:
    basename = _remote_basename(remote_directory)
    if basename in (".", "..") or "/" in basename or "\x00" in basename:
        raise MimportError(f"unsafe remote directory basename: {basename!r}")
    return _contained_join(
        Path(downloads_root),
        [_sanitize_dir_segment(username), basename],
        "download",
    )


def default_title(remote_directory: str) -> str:
    return _remote_basename(remote_directory)


def create_job(
    conn: sqlite3.Connection,
    title: str,
    search_id: str,
    username: str,
    directory: str,
    local_dir: Path,
) -> int:
    cursor = conn.execute(
        """
        INSERT INTO jobs (title, status, search_id, username, directory, local_dir)
        VALUES (?, ?, ?, ?, ?, ?)
        """,
        (title, STATUS_INCOMPLETE, search_id, username, directory, str(local_dir)),
    )
    return int(cursor.lastrowid)


def set_job_status(conn: sqlite3.Connection, job_id: int, status: str) -> None:
    conn.execute(
        """
        UPDATE jobs
        SET status = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
        WHERE id = ?
        """,
        (status, job_id),
    )


def upsert_job_file(
    conn: sqlite3.Connection,
    job_id: int,
    local_dir: Path,
    transfer: dict[str, Any],
) -> None:
    basename = _remote_basename(str(transfer["filename"]))
    if basename in ("", ".", "..") or "/" in basename or "\x00" in basename:
        raise MimportError(f"unsafe remote filename basename: {basename!r}")
    local_path = _contained_join(Path(local_dir), [basename], "job file")
    conn.execute(
        """
        INSERT INTO job_files (job_id, remote_path, transfer_id, size, state, local_path)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(job_id, transfer_id) DO UPDATE SET
            state = excluded.state,
            size = excluded.size,
            remote_path = excluded.remote_path,
            local_path = excluded.local_path
        """,
        (
            job_id,
            transfer["filename"],
            transfer["id"],
            transfer["size"],
            transfer["state"],
            str(local_path),
        ),
    )
    conn.execute(
        """
        UPDATE jobs
        SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
        WHERE id = ?
        """,
        (job_id,),
    )


def _row_to_job(row: tuple[Any, ...]) -> dict[str, Any]:
    return dict(zip(_JOB_COLUMNS, row))


def _row_to_job_file(row: tuple[Any, ...]) -> dict[str, Any]:
    return dict(zip(_JOB_FILE_COLUMNS, row))


def _select_job(conn: sqlite3.Connection, job_id: int) -> dict[str, Any] | None:
    row = conn.execute(
        f"SELECT {', '.join(_JOB_COLUMNS)} FROM jobs WHERE id = ?",
        (job_id,),
    ).fetchone()
    if row is None:
        return None
    return _row_to_job(row)


def get_job(conn: sqlite3.Connection, job_id: int) -> dict[str, Any]:
    job = _select_job(conn, job_id)
    if job is None:
        raise JobNotFound(str(job_id))
    return job


def get_job_files(conn: sqlite3.Connection, job_id: int) -> list[dict[str, Any]]:
    rows = conn.execute(
        f"SELECT {', '.join(_JOB_FILE_COLUMNS)} FROM job_files WHERE job_id = ? ORDER BY id",
        (job_id,),
    ).fetchall()
    return [_row_to_job_file(row) for row in rows]


def delete_job_files(
    conn: sqlite3.Connection,
    job_id: int,
    transfer_ids: list[str],
) -> None:
    if not transfer_ids:
        return
    placeholders = ",".join("?" for _ in transfer_ids)
    conn.execute(
        f"DELETE FROM job_files WHERE job_id = ? AND transfer_id IN ({placeholders})",
        (job_id, *transfer_ids),
    )


def set_job_file_state(
    conn: sqlite3.Connection,
    job_id: int,
    transfer_id: str,
    state: str,
) -> None:
    conn.execute(
        "UPDATE job_files SET state = ? WHERE job_id = ? AND transfer_id = ?",
        (state, job_id, transfer_id),
    )


def resolve_target(conn: sqlite3.Connection, target: str) -> dict[str, Any]:
    if _I64_RE.fullmatch(target):
        job_id = int(target)
        if _I64_MIN <= job_id <= _I64_MAX:
            try:
                return get_job(conn, job_id)
            except JobNotFound:
                pass
    row = conn.execute(
        f"SELECT {', '.join(_JOB_COLUMNS)} FROM jobs WHERE title = ? ORDER BY id DESC LIMIT 1",
        (target,),
    ).fetchone()
    if row is None:
        raise JobNotFound(target)
    return _row_to_job(row)


def _is_terminal_state(state: str) -> bool:
    return (
        state.startswith("Completed")
        or "Cancelled" in state
        or "Errored" in state
        or "Rejected" in state
    )


def _is_succeeded(state: str) -> bool:
    return state.startswith("Completed") and "Succeeded" in state


def derive_status(transfers: list[dict[str, Any]]) -> str:
    return derive_status_from_states([str(transfer["state"]) for transfer in transfers])


def derive_status_from_states(states: list[str]) -> str:
    if not states:
        return STATUS_INCOMPLETE
    if any(not _is_terminal_state(state) for state in states):
        return STATUS_INCOMPLETE
    succeeded = sum(1 for state in states if _is_succeeded(state))
    if succeeded == len(states):
        return STATUS_FETCHED
    if succeeded == 0:
        any_errored = any("Errored" in state or "Rejected" in state for state in states)
        return STATUS_FAILED if any_errored else STATUS_CANCELLED
    return STATUS_PARTIAL


def retryable_files(files: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [file for file in files if not _is_succeeded(str(file["state"]))]
