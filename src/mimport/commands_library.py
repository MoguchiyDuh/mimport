from __future__ import annotations

import logging
from contextlib import closing
from pathlib import Path
from typing import Any

from . import jobs, library
from .errors import MimportError, io_error
from .files import FileBatch, fingerprint


def run(cfg: Any, args: Any) -> Any:
    action = args.action
    if action == "list":
        return _run_list(cfg, args)
    if action == "show":
        return _run_show(cfg, args)
    if action == "edit":
        return _run_edit(cfg, args)
    if action == "remove":
        return _run_remove(cfg, args)
    if action == "prune":
        return _run_prune(cfg, args)
    raise MimportError(f"unknown library action: {action}")


def _run_list(cfg: Any, args: Any) -> list[dict[str, Any]]:
    query = list(getattr(args, "query", []) or [])
    with closing(jobs.open(cfg.paths.database)) as db:
        return library.list_tracks(db, query)


def _run_show(cfg: Any, args: Any) -> dict[str, Any]:
    with closing(jobs.open(cfg.paths.database)) as db:
        return library.get_track(db, int(args.id))


def _preflight_path_unique(
    db: Any,
    path: Path,
    track_id: int,
) -> None:
    row = db.execute(
        "SELECT id FROM library_tracks WHERE path = ? AND id <> ?",
        (str(path), track_id),
    ).fetchone()
    if row is not None:
        raise MimportError(f"library path already indexed by track {row[0]}: {path}")


def _remove_renamed_source(
    original: Path,
    identity: tuple[int, int, int, int],
    library_root: Path,
) -> None:
    try:
        current = fingerprint(original)
    except FileNotFoundError:
        return
    except OSError as exc:
        logging.warning("failed to inspect renamed source %s: %s", original, exc)
        return
    if current != identity:
        logging.warning("renamed source changed; leaving %s in place", original)
        return
    try:
        original.unlink()
    except FileNotFoundError:
        return
    except OSError as exc:
        logging.warning("failed to remove renamed source %s: %s", original, exc)
        return
    library.prune_empty_dirs(original.parent, library_root)


def _apply_edit(
    db: Any,
    track: dict[str, Any],
    edits: dict[str, Any],
    new_path: Path | None,
    library_root: Path,
    database_path: Path,
) -> dict[str, Any]:
    original = Path(track["path"])
    if original.is_symlink():
        raise MimportError(f"refusing to edit a symbolic link: {original}")
    destination = original if new_path is None else Path(new_path)
    if destination.is_symlink():
        raise MimportError(f"refusing to replace a symbolic link: {destination}")
    try:
        identity = fingerprint(original)
    except OSError as exc:
        raise io_error(original, exc) from exc

    def writer(path: Path) -> None:
        library.rewrite_tags(path, edits)

    with FileBatch(library_root, database_path) as batch:
        batch.add(original, destination, original, writer)
        batch.publish()
        with jobs.transaction(db):
            if new_path is not None:
                _preflight_path_unique(db, destination, int(track["id"]))
            updated = library.edit_track(
                db,
                int(track["id"]),
                edits,
                None if new_path is None else str(destination),
            )
        batch.finish()

    if new_path is not None:
        _remove_renamed_source(original, identity, library_root)
    return updated


def _run_edit(cfg: Any, args: Any) -> dict[str, Any]:
    fields = list(getattr(args, "fields", []) or [])
    edits = library.parse_edits(fields)
    rename = bool(getattr(args, "rename", False))
    dry_run = bool(getattr(args, "dry_run", False))
    opener = jobs.open_readonly if dry_run else jobs.open
    with closing(opener(cfg.paths.database)) as db:
        track = library.get_track(db, int(args.id))
        original = Path(track["path"])
        if original.is_symlink():
            raise MimportError(f"refusing to edit a symbolic link: {original}")
        new_path = (
            library.renamed_path(track, cfg.paths.library, edits) if rename else None
        )
        if new_path is not None and str(new_path) == track["path"]:
            new_path = None
        if new_path is not None and new_path.is_symlink():
            raise MimportError(f"refusing to replace a symbolic link: {new_path}")
        if library.edits_empty(edits) and new_path is None:
            return {"track": track, "edits": edits, "renamed_to": None}
        if new_path is not None:
            if new_path.exists():
                raise io_error(
                    str(new_path),
                    FileExistsError("renamed destination already exists"),
                )
            _preflight_path_unique(db, new_path, int(track["id"]))
        if dry_run:
            return {
                "track": track,
                "edits": edits,
                "renamed_to": None if new_path is None else str(new_path),
                "dry_run": True,
            }
        updated = _apply_edit(
            db,
            track,
            edits,
            new_path,
            Path(cfg.paths.library),
            Path(cfg.paths.database),
        )
        return {
            "track": updated,
            "edits": edits,
            "renamed_to": None if new_path is None else str(new_path),
        }


def _run_remove(cfg: Any, args: Any) -> dict[str, Any]:
    query = list(getattr(args, "query", []) or [])
    files = bool(getattr(args, "files", False))
    if "--files" in query:
        query.remove("--files")
        files = True
    with closing(jobs.open(cfg.paths.database)) as db:
        removed, deleted_files = library.remove_tracks(
            db,
            query,
            files,
            cfg.paths.library,
        )
    return {"removed": removed, "deleted_files": deleted_files}


def _run_prune(cfg: Any, args: Any) -> dict[str, Any]:
    dry_run = bool(getattr(args, "dry_run", False))
    opener = jobs.open_readonly if dry_run else jobs.open
    with closing(opener(cfg.paths.database)) as db:
        removed = library.prune_missing(db, dry_run)
    return {"removed": removed, "count": len(removed), "dry_run": dry_run}
