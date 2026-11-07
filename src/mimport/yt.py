from __future__ import annotations

import json
import subprocess
import warnings
from pathlib import Path
from typing import Any

from .errors import MimportError, io_error


def _status_text(returncode: int) -> str:
    return f"exit status: {returncode}"


def _tool_failed(status: str, stderr: str) -> MimportError:
    return MimportError(f"yt-dlp failed ({status})\n{stderr[:600]}")


def _run(args: list[str], staging_dir: Path) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            args,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except FileNotFoundError:
        raise MimportError("required external tool not found on PATH: yt-dlp")
    except OSError as exc:
        raise io_error(staging_dir, exc)


def _na(value: Any) -> Any:
    if value is None or value == "NA":
        return None
    return value


def _duration(value: Any) -> float | None:
    if value is None or value == "NA":
        return None
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


_VIDEO_ID_CHARS = frozenset(
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-"
)
_MAX_VIDEO_ID_LENGTH = 128


def _playlist_index(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        number = value
    elif isinstance(value, str):
        text = value[1:] if value.startswith("+") else value
        if not text or not text.isascii() or not text.isdigit():
            return None
        number = int(text)
    else:
        return None
    if 0 <= number <= 0xFFFFFFFF:
        return number
    return None


def _video_id(value: Any) -> str | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        value = str(value)
    if not isinstance(value, str):
        return None
    if not value or len(value) > _MAX_VIDEO_ID_LENGTH:
        return None
    if value in (".", ".."):
        return None
    if any(ch not in _VIDEO_ID_CHARS for ch in value):
        return None
    return value


def _inside(root: Path, path: Path) -> bool:
    try:
        base = root.resolve()
        candidate = path.resolve()
    except (OSError, RuntimeError, ValueError):
        return False
    return candidate != base and candidate.is_relative_to(base)


def _metadata_args(cfg: Any, playlist: bool) -> list[str]:
    args = [
        str(cfg.yt_dlp_path),
        "--ignore-config",
        "--no-cache-dir",
        "--simulate",
        "--dump-single-json",
    ]
    if playlist:
        args.append("--flat-playlist")
    else:
        args.append("--no-playlist")
    return args


def _download_args(cfg: Any, staging_dir: Path, playlist: bool) -> list[str]:
    out_template = staging_dir / "%(id)s.%(ext)s"
    print_template = "after_move:%()j"
    args = [
        str(cfg.yt_dlp_path),
        "-x",
        "--audio-format",
        "opus",
        "--write-thumbnail",
        "--convert-thumbnails",
        "jpg",
        "-o",
        str(out_template),
        "--print",
        print_template,
    ]
    if playlist:
        args.append("--ignore-errors")
    else:
        args.append("--no-playlist")
    cookies = getattr(cfg, "cookies", None)
    if cookies is not None:
        args.extend(["--cookies", str(cookies)])
    browser = getattr(cfg, "cookies_from_browser", None)
    if browser is not None:
        args.extend(["--cookies-from-browser", str(browser)])
    return args


def _dry_run(
    cfg: Any, url: str, staging_dir: Path, playlist: bool
) -> list[dict[str, Any]]:
    args = _metadata_args(cfg, playlist)
    args.append(url)
    result = _run(args, staging_dir)
    status = _status_text(result.returncode)
    if result.returncode != 0 and not playlist:
        raise _tool_failed(status, result.stderr)
    stdout = result.stdout.strip()
    if not stdout:
        raise _tool_failed(status, "no --print output captured on stdout")
    try:
        payload = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise _tool_failed(
            status, f"unexpected --print output shape: {stdout!r}: {exc}"
        )
    if not isinstance(payload, dict):
        raise _tool_failed(
            status, f"unexpected --print output shape: {stdout!r}"
        )
    entries = payload.get("entries")
    if not isinstance(entries, list):
        entries = [payload]
    tracks: list[dict[str, Any]] = []
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise _tool_failed(
                status, f"unexpected --print output shape: {entry!r}"
            )
        video_id = _video_id(entry.get("id"))
        if video_id is None:
            raise _tool_failed(
                status, "metadata entry has no filename-safe id"
            )
        raw_index = entry.get("playlist_index")
        playlist_index = _playlist_index(raw_index)
        if playlist_index is None and raw_index not in (None, "NA"):
            raise _tool_failed(
                status, "metadata entry has a non-integer playlist_index"
            )
        if playlist and playlist_index is None:
            playlist_index = index + 1
        uploader = entry.get("uploader")
        if uploader is None:
            uploader = entry.get("channel")
        tracks.append(
            {
                "path": staging_dir / f"{video_id}.opus",
                "video_id": video_id,
                "title": _na(entry.get("title")),
                "uploader": _na(uploader),
                "duration_secs": _duration(entry.get("duration")),
                "source_url": url,
                "playlist_index": playlist_index,
                "thumbnail": None,
            }
        )
    if not tracks:
        raise _tool_failed(status, "no --print output captured on stdout")
    if result.returncode != 0:
        warnings.warn(
            f"yt-dlp reported failures on some playlist entries ({status}); importing the ones that succeeded",
            RuntimeWarning,
        )
    return tracks


def _parse_download_line(
    line: str, url: str, staging_dir: Path, status: str
) -> dict[str, Any]:
    try:
        info = json.loads(line)
    except json.JSONDecodeError:
        raise _tool_failed(
            status, f"unexpected --print output shape: {line!r}"
        )
    if not isinstance(info, dict):
        raise _tool_failed(
            status, f"unexpected --print output shape: {line!r}"
        )
    video_id = _video_id(info.get("id"))
    if video_id is None:
        raise _tool_failed(
            status, "download output has no filename-safe id"
        )
    raw_index = info.get("playlist_index")
    playlist_index = _playlist_index(raw_index)
    if playlist_index is None and raw_index not in (None, "NA"):
        raise _tool_failed(
            status, "download output has a non-integer playlist_index"
        )
    filepath = info.get("filepath")
    if isinstance(filepath, str) and filepath:
        path = Path(filepath)
        if not path.is_absolute():
            path = staging_dir / path
        if not _inside(staging_dir, path):
            raise _tool_failed(
                status, "download output path escapes the staging directory"
            )
    else:
        path = staging_dir / f"{video_id}.opus"
    thumbnail = staging_dir / f"{video_id}.jpg"
    return {
        "path": path,
        "video_id": video_id,
        "title": _na(info.get("title")),
        "uploader": _na(info.get("uploader")),
        "duration_secs": _duration(info.get("duration")),
        "source_url": url,
        "playlist_index": playlist_index,
        "thumbnail": thumbnail if thumbnail.exists() else None,
    }


def fetch(
    cfg: Any,
    url: str,
    staging_dir: Path,
    playlist: bool = False,
    dry_run: bool = False,
) -> list[dict[str, Any]]:
    staging_dir = Path(staging_dir)
    if dry_run:
        return _dry_run(cfg, url, staging_dir, playlist)

    try:
        staging_dir.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        raise io_error(staging_dir, exc)

    args = _download_args(cfg, staging_dir, playlist)
    args.append(url)
    result = _run(args, staging_dir)
    status = _status_text(result.returncode)
    if result.returncode != 0 and not playlist:
        raise _tool_failed(status, result.stderr)
    if result.returncode != 0:
        warnings.warn(
            f"yt-dlp reported failures on some playlist entries ({status}); importing the ones that succeeded",
            RuntimeWarning,
        )
    tracks: list[dict[str, Any]] = []
    for line in result.stdout.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        tracks.append(
            _parse_download_line(stripped, url, staging_dir, status)
        )
    if not tracks:
        raise _tool_failed(status, "no --print output captured on stdout")
    return tracks
