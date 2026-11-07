from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

from .audio import is_audio, is_temp_file
from .errors import MimportError, io_error
from .metadata import read_track
from .scorer import text_similarity

REJECT_THRESHOLD = 0.40

W_RECORDING_ID = 10.0
W_TITLE = 3.0
W_LENGTH = 2.0
W_ARTIST = 2.0
W_INDEX = 1.0
TOTAL_WEIGHT = W_RECORDING_ID + W_TITLE + W_LENGTH + W_ARTIST + W_INDEX

SCALE = 1_000_000
DUMMY_COST_SCALED = SCALE
U32_MAX = 4294967295

_REASON_ORDER = ("recording_id", "track_artist", "track_index", "track_length", "track_title")


def _parse_u32(value: str) -> int | None:
    if value == "":
        return None
    text = value[1:] if value.startswith("+") else value
    if text == "" or not text.isascii() or not text.isdigit():
        return None
    number = int(text)
    if number > U32_MAX:
        return None
    return number


def _components(path: str | Path) -> tuple[tuple[int, str], ...]:
    text = os.fspath(path)
    if isinstance(text, bytes):
        text = os.fsdecode(text)
    if text == "":
        return ()
    rooted = text.startswith("/")
    parts = text.split("/")
    components: list[tuple[int, str]] = []
    if rooted:
        components.append((0, "/"))
    started = rooted
    for index, part in enumerate(parts):
        if part == "":
            continue
        if part == ".":
            if not rooted and not started and index == 0:
                components.append((1, "."))
                started = True
            continue
        if part == "..":
            components.append((2, ".."))
        else:
            components.append((3, part))
        started = True
    return tuple(components)


def path_sort_key(path: str | Path) -> tuple[tuple[int, str], ...]:
    return _components(path)


_path_key = path_sort_key


def _is_rust_whitespace(char: str) -> bool:
    code = ord(char)
    return (
        0x0009 <= code <= 0x000D
        or code in (0x0020, 0x0085, 0x00A0, 0x1680, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000)
        or 0x2000 <= code <= 0x200A
    )


def _rust_trim(text: str) -> str:
    start = 0
    end = len(text)
    while start < end and _is_rust_whitespace(text[start]):
        start += 1
    while end > start and _is_rust_whitespace(text[end - 1]):
        end -= 1
    return text[start:end]


def _filename_hint(path: Path) -> tuple[str | None, int | None]:
    stem = path.stem
    if stem is None:
        stem = ""
    if stem == "":
        return (None, None)
    dash = stem.find("-")
    if dash >= 0:
        number = _parse_u32(_rust_trim(stem[:dash]))
        if number is not None:
            title = _rust_trim(stem[dash + 1 :])
            return (title if title != "" else None, number)
    return (stem if stem != "" else None, None)


def _read_local_track(path: Path) -> dict[str, Any]:
    info = read_track(path)
    filename_title, filename_track = _filename_hint(path)
    title = info.get("title")
    track_number = info.get("track_number")
    if title is None:
        title = filename_title
    if track_number is None:
        track_number = filename_track
    return {
        "path": str(path),
        "title": title if isinstance(title, str) else None,
        "artist": info.get("artist") if isinstance(info.get("artist"), str) else None,
        "track_number": track_number if isinstance(track_number, int) and not isinstance(track_number, bool) else None,
        "duration_secs": info.get("duration_secs"),
        "musicbrainz_recording_id": info.get("musicbrainz_recording_id")
        if isinstance(info.get("musicbrainz_recording_id"), str)
        else None,
    }


def _walk(directory: Path, out: list[Path]) -> None:
    try:
        entries = os.scandir(directory)
    except OSError as exc:
        raise io_error(directory, exc) from None
    with entries:
        while True:
            try:
                entry = next(entries)
            except StopIteration:
                break
            except OSError as exc:
                raise io_error(directory, exc) from None
            path = Path(entry.path)
            try:
                if entry.is_symlink():
                    if is_audio(path):
                        raise MimportError(f"refusing to scan symlinked audio file: {path}")
                    continue
                is_directory = entry.is_dir(follow_symlinks=False)
                if is_directory:
                    _walk(path, out)
                    continue
                is_file = entry.is_file(follow_symlinks=False)
            except OSError as exc:
                raise io_error(directory, exc) from None
            if is_file and is_audio(path) and not is_temp_file(path):
                out.append(path)


def scan_local_tracks(root: Path) -> list[dict[str, Any]]:
    root_path = Path(root)
    if root_path.is_symlink():
        raise MimportError(f"refusing to scan symlinked source path: {root_path}")
    paths: list[Path] = []
    if root_path.is_file():
        if is_audio(root_path):
            paths.append(root_path)
    else:
        _walk(root_path, paths)
    paths.sort(key=path_sort_key)
    return [_read_local_track(path) for path in paths]


def _track_distance(
    local: dict[str, Any],
    track: dict[str, Any],
    release_artist: str | None,
) -> tuple[float, dict[str, float]]:
    reasons: dict[str, float] = {}
    present_weight = 0.0

    local_id = local.get("musicbrainz_recording_id")
    track_id = track.get("recording_id")
    if isinstance(local_id, str) and isinstance(track_id, str) and _rust_trim(local_id) != "":
        present_weight += W_RECORDING_ID
        recording_id_cost = 0.0 if local_id == track_id else 1.0
    else:
        recording_id_cost = 0.0
    reasons["recording_id"] = recording_id_cost * W_RECORDING_ID

    local_title = local.get("title")
    if isinstance(local_title, str):
        present_weight += W_TITLE
        track_title = track.get("title") if isinstance(track.get("title"), str) else ""
        title_cost = 1.0 - text_similarity(local_title, track_title)
    else:
        title_cost = 0.0
    reasons["track_title"] = title_cost * W_TITLE

    local_duration = local.get("duration_secs")
    track_length = track.get("length_ms")
    if local_duration is not None and track_length is not None:
        present_weight += W_LENGTH
        diff = abs(float(local_duration) - float(track_length) / 1000.0)
        length_cost = min(max(diff - 10.0, 0.0) / 30.0, 1.0)
    else:
        length_cost = 0.0
    reasons["track_length"] = length_cost * W_LENGTH

    local_artist = local.get("artist")
    if isinstance(local_artist, str) and release_artist is not None:
        present_weight += W_ARTIST
        artist_cost = 1.0 - text_similarity(local_artist, release_artist)
    else:
        artist_cost = 0.0
    reasons["track_artist"] = artist_cost * W_ARTIST

    local_index = local.get("track_number")
    track_index = track.get("position")
    if local_index is not None and track_index is not None:
        present_weight += W_INDEX
        index_cost = 0.0 if local_index == track_index else 1.0
    else:
        index_cost = 0.0
    reasons["track_index"] = index_cost * W_INDEX

    denominator = present_weight if present_weight > 0.0 else TOTAL_WEIGHT
    total = sum(reasons[key] for key in _REASON_ORDER) / denominator
    return (total, reasons)


def _round_half_away(value: float) -> int:
    floor = int(value // 1)
    fraction = value - floor
    if fraction >= 0.5:
        return floor + 1
    return floor


def _kuhn_munkres_min(costs: list[list[int]]) -> list[int]:
    rows = len(costs)
    if rows == 0:
        return []
    columns = len(costs[0])
    weights = [[-value for value in row] for row in costs]
    xy: list[int | None] = [None] * rows
    yx: list[int | None] = [None] * columns
    lx = [max(row) for row in weights]
    ly = [0] * columns
    for root in range(rows):
        alternating: list[int | None] = [None] * columns
        s_list = [root]
        s_set = {root}
        slack = [lx[root] + ly[y] - weights[root][y] for y in range(columns)]
        slackx = [root] * columns
        while True:
            delta = None
            x = 0
            y = 0
            for yy in range(columns):
                if alternating[yy] is None and (delta is None or slack[yy] < delta):
                    delta = slack[yy]
                    x = slackx[yy]
                    y = yy
            if delta is None:
                raise MimportError("assignment matrix has no available column")
            if delta > 0:
                for xx in s_list:
                    lx[xx] -= delta
                for yy in range(columns):
                    if alternating[yy] is not None:
                        ly[yy] += delta
                    else:
                        slack[yy] -= delta
            alternating[y] = x
            if yx[y] is None:
                break
            previous_x = yx[y]
            if previous_x not in s_set:
                s_list.append(previous_x)
                s_set.add(previous_x)
            for yy in range(columns):
                if alternating[yy] is None:
                    alternate_slack = lx[previous_x] + ly[yy] - weights[previous_x][yy]
                    if slack[yy] > alternate_slack:
                        slack[yy] = alternate_slack
                        slackx[yy] = previous_x
        current: int | None = y
        while current is not None:
            x = alternating[current]
            if x is None:
                break
            predecessor = xy[x]
            yx[current] = x
            xy[x] = current
            current = predecessor
    return [value if value is not None else 0 for value in xy]


def _matched_track(
    file: str | Path,
    track: dict[str, Any],
    distance: float,
    reasons: dict[str, float],
) -> dict[str, Any]:
    return {
        "file": os.fspath(file),
        "position": track.get("position"),
        "medium_position": track.get("medium_position"),
        "title": track.get("title") if isinstance(track.get("title"), str) else "",
        "title_native": track.get("title_native") if isinstance(track.get("title_native"), str) else None,
        "source_url": None,
        "recording_id": track.get("recording_id") if isinstance(track.get("recording_id"), str) else None,
        "raw_position": track.get("raw_position") if isinstance(track.get("raw_position"), str) else None,
        "distance": float(distance),
        "reasons": {key: float(reasons[key]) for key in _REASON_ORDER} if reasons else {},
    }


def _missing_track(track: dict[str, Any]) -> dict[str, Any]:
    return {
        "position": track.get("position"),
        "medium_position": track.get("medium_position"),
        "title": track.get("title") if isinstance(track.get("title"), str) else "",
        "recording_id": track.get("recording_id") if isinstance(track.get("recording_id"), str) else None,
    }


def match_tracks(locals: list[dict[str, Any]], release: dict[str, Any]) -> dict[str, Any]:
    local_count = len(locals)
    tracks = release.get("tracks") if isinstance(release.get("tracks"), list) else []
    track_count = len(tracks)
    size = max(local_count, track_count)
    if size == 0:
        return {"matched": [], "unmatched_files": [], "missing_tracks": []}

    release_artist = release.get("artist_credit") if isinstance(release.get("artist_credit"), str) else None
    real_distances: list[list[tuple[float, dict[str, float]]]] = []
    for local in locals:
        row = [_track_distance(local, track, release_artist) for track in tracks]
        real_distances.append(row)

    cost_rows: list[list[int]] = []
    for i in range(size):
        row: list[int] = []
        for j in range(size):
            if i < local_count and j < track_count:
                total = real_distances[i][j][0]
                row.append(_round_half_away(total * SCALE))
            else:
                row.append(DUMMY_COST_SCALED)
        cost_rows.append(row)
    assignment = _kuhn_munkres_min(cost_rows)

    matched: list[dict[str, Any]] = []
    unmatched_files: list[str] = []
    matched_tracks: set[int] = set()
    for i in range(local_count):
        j = assignment[i]
        if j < track_count and real_distances[i][j][0] <= REJECT_THRESHOLD:
            total, reasons = real_distances[i][j]
            matched.append(_matched_track(locals[i].get("path", ""), tracks[j], total, reasons))
            matched_tracks.add(j)
        else:
            unmatched_files.append(os.fspath(locals[i].get("path", "")))

    missing_tracks = [
        _missing_track(track)
        for index, track in enumerate(tracks)
        if index not in matched_tracks
    ]
    return {
        "matched": matched,
        "unmatched_files": unmatched_files,
        "missing_tracks": missing_tracks,
    }


def _debug_str(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def _force_target(value: Any) -> tuple[int, int | None] | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        if 0 <= value <= U32_MAX:
            return (value, None)
        return None
    if isinstance(value, dict):
        position = value.get("position")
        if isinstance(position, bool) or not isinstance(position, int) or not 0 <= position <= U32_MAX:
            return None
        disc = value.get("disc")
        if disc is None:
            return (position, None)
        if isinstance(disc, bool) or not isinstance(disc, int) or not 0 <= disc <= U32_MAX:
            return None
        return (position, disc)
    return None


def _force_error(detail: str) -> MimportError:
    return MimportError(f"--force mapping invalid: {detail}")


def apply_force_mapping(
    locals: list[dict[str, Any]],
    release: dict[str, Any],
    path: Path,
) -> dict[str, Any]:
    mapping_path = Path(path)
    try:
        text = mapping_path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise io_error(mapping_path, exc) from None
    try:
        mapping = json.loads(text)
    except json.JSONDecodeError as exc:
        raise _force_error(f"{mapping_path}: {exc}") from None
    if not isinstance(mapping, dict):
        raise _force_error(f"{mapping_path}: invalid type: {_serde_type(mapping)}, expected a map")

    tracks = release.get("tracks") if isinstance(release.get("tracks"), list) else []
    local_by_path: dict[tuple[tuple[int, str], ...], dict[str, Any]] = {}
    for local in locals:
        local_by_path.setdefault(_path_key(local.get("path", "")), local)
    matched: list[dict[str, Any]] = []
    matched_tracks: set[int] = set()
    matched_files: set[tuple[tuple[int, str], ...]] = set()

    for file_str in sorted(mapping):
        local = local_by_path.get(_path_key(file_str))
        if local is None:
            raise _force_error(f"{_debug_str(file_str)} is not one of the scanned local files")
        target = _force_target(mapping[file_str])
        if target is None:
            raise _force_error(
                f"{mapping_path}: data did not match any variant of untagged enum ForceTarget"
            )
        position, disc = target
        candidates: list[tuple[int, dict[str, Any]]] = []
        for index, track in enumerate(tracks):
            if track.get("position") != position:
                continue
            if disc is not None and track.get("medium_position") != disc:
                continue
            candidates.append((index, track))
        if len(candidates) > 1:
            raise _force_error(f"position {position} is ambiguous across discs; specify a disc")
        if not candidates:
            raise _force_error(f"no track at position {position} in the release")
        track_index, track = candidates[0]
        if track_index in matched_tracks:
            disc_debug = "None" if disc is None else f"Some({disc})"
            raise _force_error(f"multiple files map to position {position} on disc {disc_debug}")
        matched_tracks.add(track_index)
        local_path = local.get("path", "")
        matched.append(_matched_track(local_path, track, 0.0, {}))
        matched_files.add(_path_key(local_path))

    unmatched_files = [
        os.fspath(local.get("path", ""))
        for local in locals
        if _path_key(local.get("path", "")) not in matched_files
    ]
    missing_tracks = [
        _missing_track(track)
        for index, track in enumerate(tracks)
        if index not in matched_tracks
    ]
    return {
        "matched": matched,
        "unmatched_files": unmatched_files,
        "missing_tracks": missing_tracks,
    }


def _serde_type(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return f"boolean `{str(value).lower()}`"
    if isinstance(value, int):
        return f"integer `{value}`"
    if isinstance(value, float):
        return f"floating point `{value}`"
    if isinstance(value, str):
        return f"string {_debug_str(value)}"
    if isinstance(value, list):
        return "sequence"
    if isinstance(value, dict):
        return "map"
    return "value"
