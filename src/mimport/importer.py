import argparse
import logging
import os
import re
import shutil
import stat
import tempfile
import unicodedata
from collections.abc import Iterator
from contextlib import closing, contextmanager
from pathlib import Path
from typing import Any

from mimport import audio, jobs, library, matching, mb, metadata, postfix, release, tags
from mimport.config import Config
from mimport.coverart import CoverArt, CoverArtClient, from_local_file
from mimport.errors import JobNotFound, MimportError, io_error
from mimport.files import FileBatch, contained, fingerprint, same_file


def sanitize(value: str) -> str:
    normalized = unicodedata.normalize("NFC", value)
    return "".join("_" if char in '/\\:*?"<>|' else char for char in normalized).strip().strip(".")


def resolve_job_or_path(
    cfg: Config, target: str, dry_run: bool = False
) -> tuple[dict[str, Any] | None, Path]:
    candidate = Path(target)
    try:
        numeric = re.fullmatch(r"[+-]?[0-9]+", target) is not None and -9_223_372_036_854_775_808 <= int(target) <= 9_223_372_036_854_775_807
    except ValueError:
        numeric = False
    if not numeric and candidate.exists():
        return None, candidate
    if dry_run and not cfg.paths.database.exists():
        if candidate.exists():
            return None, candidate
        raise JobNotFound(target)
    opener = jobs.open_readonly if dry_run else jobs.open
    with closing(opener(cfg.paths.database)) as connection:
        try:
            job = jobs.resolve_target(connection, target)
        except JobNotFound:
            if candidate.exists():
                return None, candidate
            raise
    return job, Path(job["local_dir"])


def validate_native(client: mb.MbClient, normalized: dict[str, Any], overrides: dict[str, Any], allow: bool) -> None:
    unresolved = tags.resolve(client, normalized, overrides)
    if unresolved and not allow:
        raise MimportError(
            "release has non-Latin fields with no MusicBrainz romanization alias; supply --tags/--track-title or pass --allow-native:\n"
            + tags.format_unresolved(unresolved)
        )
    return


def scan_sources(root: Path) -> list[Path]:
    if root.is_symlink():
        raise MimportError(f"refusing to scan symlinked source path: {root}")
    if root.is_file():
        candidates = [root]
    elif root.is_dir():
        candidates: list[Path] = []

        def fail(error: OSError) -> None:
            raise io_error(Path(error.filename) if error.filename else root, error)

        for directory, directories, filenames in os.walk(root, followlinks=False, onerror=fail):
            for name in [*directories, *filenames]:
                path = Path(directory) / name
                if path.is_symlink():
                    if audio.is_audio(path) and not audio.is_temp_file(path):
                        raise MimportError(f"refusing to scan symlinked audio file: {path}")
                    continue
                if path.is_file():
                    candidates.append(path)
    else:
        raise io_error(root, FileNotFoundError("source does not exist"))
    return [
        path for path in sorted(candidates, key=matching.path_sort_key)
        if audio.is_audio(path) and not audio.is_temp_file(path)
    ]


def _translated_report(report: dict[str, Any], temporary: Path, root: Path) -> dict[str, Any]:
    base = root.parent if root.is_file() else root

    def translate(value: Any) -> Any:
        if isinstance(value, dict):
            return {key: translate(item) for key, item in value.items()}
        if isinstance(value, list):
            return [translate(item) for item in value]
        if isinstance(value, (str, Path)):
            path = Path(value)
            if path.is_relative_to(temporary):
                return str(base / path.relative_to(temporary))
            return str(value)
        return value

    return translate(report)


def _predicted_sources(paths: list[Path], report: dict[str, Any]) -> dict[str, Path]:
    converted = {str(path) for path in report["transcoded"]}
    downsampled = {str(path) for path in report["downsampled"]}
    result: dict[str, Path] = {}
    for source in paths:
        if str(source.with_suffix(".opus")) in converted:
            result[str(source)] = source.with_suffix(".opus")
        elif str(source) in downsampled or str(source.with_suffix(".flac")) in downsampled:
            result[str(source)] = source.with_suffix(".flac")
        else:
            result[str(source)] = source
    return result


@contextmanager
def prepare_sources(
    cfg: Config, root: Path, paths: list[Path], dry_run: bool
) -> Iterator[tuple[list[dict[str, Any]], dict[str, Path], dict[str, Any]]]:
    if dry_run:
        report = postfix.run(root, cfg.quality, dry_run=True)
        yield matching.scan_local_tracks(root), _predicted_sources(paths, report), report
        return
    cfg.paths.staging.mkdir(parents=True, exist_ok=True)
    base = root.parent if root.is_file() else root
    with tempfile.TemporaryDirectory(prefix=".mimport-prepare-", dir=cfg.paths.staging) as name:
        temporary = Path(name)
        originals: dict[Path, Path] = {}
        for source in paths:
            prepared = temporary / source.relative_to(base)
            prepared.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, prepared)
            prepared.chmod(prepared.stat().st_mode | stat.S_IWUSR)
            originals[prepared] = source
        report = postfix.run(temporary, cfg.quality)
        prepared_sources: dict[str, Path] = {}
        by_prepared: dict[Path, Path] = {}
        for copied, source in originals.items():
            available = [copied] if copied.exists() else [
                candidate for candidate in (copied.with_suffix(".opus"), copied.with_suffix(".flac"))
                if candidate.exists()
            ]
            if len(available) != 1 or available[0] in by_prepared:
                raise MimportError(f"source preparation did not produce one unique audio file: {source}")
            by_prepared[available[0]] = source
            prepared_sources[str(source)] = available[0]
        locals_ = matching.scan_local_tracks(temporary)
        for local in locals_:
            prepared = Path(local["path"])
            if prepared not in by_prepared:
                raise MimportError(f"unexpected prepared audio file: {prepared}")
            local["path"] = str(by_prepared[prepared])
        locals_.sort(key=lambda item: matching.path_sort_key(item["path"]))
        if len(locals_) != len(paths):
            raise MimportError("source preparation lost audio files")
        yield locals_, prepared_sources, _translated_report(report, temporary, root)
    return


def destination_for(
    library_root: Path,
    normalized: dict[str, Any],
    matched: dict[str, Any],
    extension: str,
) -> Path:
    multi_disc = any((track.get("medium_position") or 1) > 1 for track in normalized["tracks"])
    artist = sanitize(normalized.get("artist_credit") or "Unknown Artist")
    year = release.year(normalized)
    album = sanitize(normalized["title"] + (f" ({year})" if year else ""))
    position = matched.get("position")
    prefix = f"{position:02}" if position is not None else sanitize(matched.get("raw_position") or "") or "00"
    if multi_disc:
        prefix = f"{matched.get('medium_position') or 1:02}-{prefix}"
    destination = library_root / artist / album / f"{prefix} - {sanitize(matched['title'])}{extension}"
    if not contained(library_root, destination):
        raise MimportError(f"destination escapes library root: {destination}")
    return destination


def plan_import(
    cfg: Config,
    matched: list[dict[str, Any]],
    normalized: dict[str, Any],
    prepared: dict[str, Path] | None = None,
) -> list[dict[str, str]]:
    result: list[dict[str, str]] = []
    seen: set[Path] = set()
    for track in matched:
        source = Path(track["file"])
        physical = prepared.get(str(source), source) if prepared is not None else source
        destination = destination_for(cfg.paths.library, normalized, track, physical.suffix)
        key = destination.resolve()
        if key in seen:
            raise io_error(destination, FileExistsError("two matched files resolve to the same destination"))
        seen.add(key)
        if destination.exists() and not same_file(source, destination):
            raise io_error(destination, FileExistsError("destination already exists"))
        if destination.is_symlink():
            raise MimportError(f"refusing to replace a symbolic link: {destination}")
        result.append({"source": str(source), "dest": str(destination)})
    return result


def publish_import(
    cfg: Config,
    matched: list[dict[str, Any]],
    normalized: dict[str, Any],
    cover: CoverArt | None = None,
    prepared: dict[str, Path] | None = None,
    job: dict[str, Any] | None = None,
    dry_run: bool = False,
) -> tuple[list[dict[str, str]], dict[str, Any] | None]:
    plan = plan_import(cfg, matched, normalized, prepared)
    if dry_run:
        return plan, job
    if not plan and job is None:
        return plan, job
    with FileBatch(cfg.paths.library, cfg.paths.database) as batch:
        for track, placement in zip(matched, plan, strict=True):
            original = Path(placement["source"])
            physical = prepared.get(str(original), original) if prepared is not None else original
            batch.add(
                physical,
                Path(placement["dest"]),
                original,
                lambda path, track=track: metadata.write_tags(path, track, normalized, cover),
            )
        with closing(jobs.open(cfg.paths.database)) as connection:
            with jobs.transaction(connection):
                batch.publish()
                for track, placement in zip(matched, plan, strict=True):
                    library.insert_track(connection, job["id"] if job is not None else None, normalized, track, placement)
                if job is not None:
                    jobs.set_job_status(connection, job["id"], jobs.STATUS_IMPORTED)
                    job = jobs.get_job(connection, job["id"])
            batch.finish()
    return plan, job


def cleanup_sources(
    imported: list[dict[str, str]], root: Path, snapshots: dict[str, tuple[int, int, int, int]]
) -> dict[str, list[str]]:
    report: dict[str, list[str]] = {"deleted_files": [], "removed_dirs": []}
    boundary = root.parent if root.is_file() else root
    parents: set[Path] = set()
    for item in imported:
        source, destination = Path(item["source"]), Path(item["dest"])
        if same_file(source, destination) or not source.exists():
            continue
        if not contained(boundary, source):
            raise MimportError(f"refusing to clean a source outside its root: {source}")
        if fingerprint(source) != snapshots[str(source)]:
            raise MimportError(f"source changed during import; retained original: {source}")
        source.unlink()
        report["deleted_files"].append(str(source))
        parents.add(source.parent)
    for start in sorted(parents):
        current = start
        while contained(boundary, current) and current.resolve() != boundary.resolve():
            try:
                current.rmdir()
            except OSError:
                break
            report["removed_dirs"].append(str(current))
            current = current.parent
    return report


def run(cfg: Config, args: argparse.Namespace) -> dict[str, Any]:
    job, root = resolve_job_or_path(cfg, args.target, args.dry_run)
    overrides = tags.apply_flag_overrides(
        tags.load(args.tags),
        artist=args.artist,
        album=args.album,
        date=args.date,
        label=args.label,
        album_artist=args.album_artist,
        compilation=True if args.compilation else None,
        genres=args.genre,
        cover=args.cover,
        track_titles=args.track_title,
    )
    with mb.MbClient(cfg.musicbrainz) as client:
        normalized = release.from_mb(mb.release_with_tracks(client, args.release))
        validate_native(client, normalized, overrides, args.allow_native)
    paths = scan_sources(root)
    snapshots = {str(path): fingerprint(path) for path in paths}
    cover: CoverArt | None = None
    imported: list[dict[str, str]] = []
    cleanup: dict[str, list[str]] | None = None
    with prepare_sources(cfg, root, paths, args.dry_run) as (locals_, prepared, postfix_report):
        report = (
            matching.apply_force_mapping(locals_, normalized, args.force)
            if args.force is not None else matching.match_tracks(locals_, normalized)
        )
        blocked = False if args.force is not None else bool(
            report["unmatched_files"] or (not args.allow_partial and report["missing_tracks"])
        )
        if not blocked and not args.dry_run:
            if overrides.get("cover") is not None:
                cover = from_local_file(Path(overrides["cover"]))
            elif args.cover_art:
                try:
                    with CoverArtClient(cfg.cover_art, cfg.musicbrainz.user_agent) as client:
                        cover = (
                            client.front_cover_with_fallback(
                                normalized["id"], normalized.get("artist_credit") or "", normalized["title"]
                            )
                            if client.itunes_fallback_enabled() else client.front_cover(normalized["id"])
                        )
                except MimportError as exc:
                    logging.warning("cover art fetch failed for %s: %s; importing without art", normalized["id"], exc)
        if not blocked:
            imported, job = publish_import(
                cfg, report["matched"], normalized, cover, prepared, job, args.dry_run
            )
    if not args.dry_run and not blocked and (args.move_files or args.cleanup):
        cleanup_report = cleanup_sources(imported, root, snapshots)
        if args.cleanup:
            cleanup = cleanup_report
    return {
        "job": job,
        "release": normalized["id"],
        "blocked": blocked,
        "cover_art": cover is not None,
        "postfix": postfix_report,
        "matched": report["matched"],
        "unmatched_files": report["unmatched_files"],
        "missing_tracks": report["missing_tracks"],
        "imported": imported,
        "cleanup": cleanup,
    }
