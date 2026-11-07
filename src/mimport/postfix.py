from __future__ import annotations

import os
import shutil
import warnings
from pathlib import Path
from typing import Any

from mutagen.flac import FLAC
from mutagen.oggopus import OggOpus

from .audio import (
    is_audio,
    is_lossy,
    is_temp_file,
    needs_downsample,
    probe,
    suspicion_reason,
    transcode_opus,
    downsample,
)
from .errors import MimportError, io_error

ALLOWLIST = (
    "TITLE",
    "ARTIST",
    "ALBUMARTIST",
    "ALBUM",
    "COMPILATION",
    "GENRE",
    "TRACKNUMBER",
    "TRACKTOTAL",
    "DISCNUMBER",
    "DATE",
    "LABEL",
    "MUSICBRAINZ_ALBUMID",
    "MUSICBRAINZ_TRACKID",
    "SOURCEURL",
    "COMMENT",
    "LYRICS",
)


def _normalize_key(key: str) -> str:
    return "".join(ch for ch in key if ch not in " _").upper()


_NORMALIZED_ALLOWLIST = frozenset(
    _normalize_key(key) for key in ALLOWLIST
)


def is_allowed(key: str) -> bool:
    return _normalize_key(key) in _NORMALIZED_ALLOWLIST


def _ext(path: Path) -> str:
    return path.suffix[1:].lower() if path.suffix else ""


def _quality_value(quality: Any, name: str, default: Any) -> Any:
    if isinstance(quality, dict):
        return quality.get(name, default)
    return getattr(quality, name, default)


def _collect_audio(root: Path) -> list[Path]:
    if root.is_file():
        return [root] if is_audio(root) else []
    out: list[Path] = []

    def onerror(exc: OSError) -> None:
        raise io_error(Path(exc.filename or str(root)), exc)

    for dirpath, _dirnames, filenames in os.walk(root, onerror=onerror):
        for name in filenames:
            path = Path(dirpath) / name
            if is_audio(path) and not is_temp_file(path):
                out.append(path)
    out.sort(key=lambda path: str(path))
    return out


def _strip_id3v2_prefix(path: Path, dry_run: bool) -> bool | None:
    try:
        with path.open("rb") as source:
            head = source.read(3)
            if head != b"ID3":
                return None
            source.seek(0)
            buffer = source.read(2 * 1024 * 1024)
    except OSError as exc:
        raise io_error(path, exc)
    position = buffer.find(b"fLaC")
    if position < 0:
        return False
    if position == 0 or dry_run:
        return True
    tmp = path.with_suffix(".mimport-id3strip.tmp")
    try:
        with path.open("rb") as source, tmp.open("wb") as target:
            source.seek(position)
            shutil.copyfileobj(source, target)
        os.replace(tmp, path)
    except OSError as exc:
        try:
            tmp.unlink()
        except OSError:
            pass
        raise io_error(path, exc)
    return True


def _purge_tags(path: Path, dry_run: bool) -> list[str]:
    ext = _ext(path)
    if ext not in ("flac", "opus"):
        return []
    try:
        obj = FLAC(path) if ext == "flac" else OggOpus(path)
    except Exception as exc:
        raise MimportError(f"probe/tag error at {path}: {exc}")
    tags = getattr(obj, "tags", None)
    if tags is None:
        return []
    junk = sorted(
        {str(key) for key in tags.keys() if not is_allowed(str(key))}
    )
    if not junk or dry_run:
        return junk
    for key in junk:
        try:
            del tags[key]
        except KeyError:
            pass
    try:
        obj.save()
    except Exception as exc:
        raise MimportError(f"probe/tag error at {path}: {exc}")
    return junk


def _purge_and_report(
    path: Path, dry_run: bool, report: dict[str, Any]
) -> None:
    try:
        purged = _purge_tags(path, dry_run)
    except MimportError as exc:
        warnings.warn(
            f"skipping tag purge for {path}: {exc}", RuntimeWarning
        )
        return
    if purged:
        report["tags_purged"].append(
            {"path": str(path), "keys": purged}
        )


def run(root: Path, quality: Any, dry_run: bool = False) -> dict[str, Any]:
    root = Path(root)
    if not root.exists():
        raise io_error(root, FileNotFoundError("no such path"))
    files = _collect_audio(root)
    report: dict[str, Any] = {
        "files_scanned": len(files),
        "downsampled": [],
        "transcoded": [],
        "skipped_lossy": [],
        "tags_purged": [],
        "suspicious": [],
        "stripped_id3v2": [],
    }

    codec = _quality_value(quality, "codec", "flac")
    if hasattr(codec, "value"):
        codec = codec.value
    codec = str(codec).lower()
    target_rate = int(_quality_value(quality, "target_samplerate", 44100))
    target_depth = int(_quality_value(quality, "target_bitdepth", 16))
    opus_bitrate = int(_quality_value(quality, "opus_bitrate_kbps", 160))

    for path in files:
        lossy = is_lossy(path)

        if _ext(path) == "flac":
            try:
                stripped = _strip_id3v2_prefix(path, dry_run)
            except MimportError as exc:
                warnings.warn(
                    f"ID3v2 strip failed for {path}: {exc}",
                    RuntimeWarning,
                )
                stripped = None
            if stripped is False:
                report["suspicious"].append(
                    {
                        "path": str(path),
                        "reason": "ID3v2 chunk before fLaC magic; no fLaC found to strip to",
                    }
                )
                continue
            if stripped is True:
                report["stripped_id3v2"].append(str(path))

        try:
            props = probe(path)
        except MimportError as exc:
            warnings.warn(
                f"skipping unreadable file {path}: {exc}", RuntimeWarning
            )
            continue

        if lossy:
            report["skipped_lossy"].append(str(path))
        else:
            reason = suspicion_reason(props)
            if reason is not None:
                report["suspicious"].append(
                    {"path": str(path), "reason": reason}
                )

            if codec == "opus":
                report["transcoded"].append(str(path.with_suffix(".opus")))
                if not dry_run:
                    dst = transcode_opus(path, opus_bitrate)
                    _purge_and_report(dst, dry_run, report)
                    continue
            elif codec == "flac":
                if needs_downsample(props, target_rate, target_depth):
                    report["downsampled"].append(str(path))
                    if not dry_run:
                        path = downsample(path, target_rate, target_depth)

        _purge_and_report(path, dry_run, report)

    return report


def quality_ok(
    props: dict[str, Any], target_rate: int, target_depth: int
) -> bool:
    return not needs_downsample(props, target_rate, target_depth)
