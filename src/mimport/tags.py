from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any

from .errors import MimportError, io_error
from .mb import artist_aliases, recording_aliases, release_group_aliases
from .romanize import is_non_latin, pick_alias

U32_MAX = 4294967295
LOGGER = logging.getLogger(__name__)

_STRING_FIELDS = ("artist", "album", "date", "label", "album_artist", "genre")


def _default_overrides() -> dict[str, Any]:
    return {
        "artist": None,
        "album": None,
        "date": None,
        "label": None,
        "album_artist": None,
        "compilation": None,
        "genre": None,
        "genres": [],
        "cover": None,
        "tracks": {},
    }


def _debug_str(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


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


def _invalid(path: Path, detail: str) -> MimportError:
    return MimportError(f"--tags file invalid: {path}: {detail}")


def load(path: Path | None) -> dict[str, Any]:
    if path is None:
        return _default_overrides()
    source = Path(path)
    try:
        text = source.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise io_error(source, exc) from None
    try:
        raw = json.loads(text)
    except json.JSONDecodeError as exc:
        raise _invalid(source, str(exc)) from None
    if not isinstance(raw, dict):
        raise _invalid(source, f"invalid type: {_serde_type(raw)}, expected struct TagOverrides")

    overrides = _default_overrides()
    for field in _STRING_FIELDS:
        value = raw.get(field)
        if value is None:
            continue
        if not isinstance(value, str):
            raise _invalid(source, f"invalid type: {_serde_type(value)}, expected a string")
        overrides[field] = value

    compilation = raw.get("compilation")
    if compilation is not None:
        if not isinstance(compilation, bool):
            raise _invalid(source, f"invalid type: {_serde_type(compilation)}, expected a boolean")
        overrides["compilation"] = compilation

    if "genres" in raw:
        genres = raw["genres"]
        if not isinstance(genres, list):
            raise _invalid(source, f"invalid type: {_serde_type(genres)}, expected a sequence")
        for item in genres:
            if not isinstance(item, str):
                raise _invalid(source, f"invalid type: {_serde_type(item)}, expected a string")
        overrides["genres"] = list(genres)

    if "cover" in raw:
        cover = raw["cover"]
        if cover is not None:
            if not isinstance(cover, str):
                raise _invalid(source, f"invalid type: {_serde_type(cover)}, expected a string")
            overrides["cover"] = cover

    if "tracks" in raw:
        tracks = raw["tracks"]
        if not isinstance(tracks, dict):
            raise _invalid(source, f"invalid type: {_serde_type(tracks)}, expected a map")
        for value in tracks.values():
            if not isinstance(value, str):
                raise _invalid(source, f"invalid type: {_serde_type(value)}, expected a string")
        overrides["tracks"] = dict(tracks)

    return overrides


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


def _normalize_track_key(key: str) -> str | None:
    text = _rust_trim(key)
    if ":" in text:
        disc_text, position_text = text.split(":", 1)
        disc = _parse_u32(_rust_trim(disc_text))
        position = _parse_u32(_rust_trim(position_text))
        if disc is None or position is None:
            return None
        return f"{disc}:{position}"
    position = _parse_u32(text)
    if position is None:
        return None
    return str(position)


def apply_flag_overrides(
    overrides: dict[str, Any],
    *,
    artist: str | None = None,
    album: str | None = None,
    date: str | None = None,
    label: str | None = None,
    album_artist: str | None = None,
    compilation: bool | None = None,
    genres: list[str] | None = None,
    cover: Path | str | None = None,
    track_titles: list[str] | None = None,
) -> dict[str, Any]:
    for key, value in _default_overrides().items():
        overrides.setdefault(key, value)

    if artist is not None:
        overrides["artist"] = artist
    if album is not None:
        overrides["album"] = album
    if date is not None:
        overrides["date"] = date
    if label is not None:
        overrides["label"] = label
    if album_artist is not None:
        overrides["album_artist"] = album_artist
    if compilation is not None:
        overrides["compilation"] = compilation
    if genres:
        overrides["genres"] = list(genres)
    if cover is not None:
        overrides["cover"] = cover

    tracks = overrides["tracks"]
    if not isinstance(tracks, dict):
        tracks = {}
        overrides["tracks"] = tracks
    for raw in track_titles or []:
        if "=" not in raw:
            raise MimportError(
                f"--tags file invalid: --track-title {_debug_str(raw)} must be \"[<disc>:]<position>=<title>\""
            )
        key, title = raw.split("=", 1)
        normalized = _normalize_track_key(key)
        if normalized is None:
            raise MimportError(
                f"--tags file invalid: --track-title {_debug_str(raw)} has a non-numeric [disc:]position"
            )
        tracks[normalized] = title

    return overrides


def merged_genres(overrides: dict[str, Any]) -> list[str]:
    out: list[str] = []
    single = overrides.get("genre")
    values: list[str] = []
    if isinstance(single, str):
        values.append(single)
    many = overrides.get("genres")
    if isinstance(many, list):
        values.extend(item for item in many if isinstance(item, str))
    for value in values:
        genre = _rust_trim(value)
        if genre != "" and genre not in out:
            out.append(genre)
    return out


def _alias_call(function: Any, client: Any, identifier: str) -> list[dict[str, Any]] | None:
    try:
        result = function(client, identifier)
    except Exception:
        return None
    if not isinstance(result, list):
        return None
    return [item for item in result if isinstance(item, dict)]


def _resolve_album(
    client: Any,
    release: dict[str, Any],
    overrides: dict[str, Any],
    unresolved: list[dict[str, Any]],
) -> None:
    title = release.get("title") if isinstance(release.get("title"), str) else ""
    album = overrides.get("album")
    if album is not None:
        if album != title:
            release["title_native"] = title
            release["title"] = album
        return
    if not is_non_latin(title):
        return
    native = title
    group_id = release.get("release_group_id")
    resolved: str | None = None
    if isinstance(group_id, str):
        aliases = _alias_call(release_group_aliases, client, group_id)
        resolved = pick_alias(aliases)
    if resolved is not None:
        release["title_native"] = native
        release["title"] = resolved
    else:
        unresolved.append({"field": "album", "native": native})


def _resolve_artist(
    client: Any,
    release: dict[str, Any],
    overrides: dict[str, Any],
    unresolved: list[dict[str, Any]],
) -> None:
    parts = release.get("artist_credit_parts")
    if not isinstance(parts, list):
        parts = []

    if not parts:
        artist = overrides.get("artist")
        if artist is not None:
            release["artist_credit"] = artist
        return

    native_parts: list[str] = []
    for part in parts:
        item = part if isinstance(part, dict) else {}
        name = item.get("name") if isinstance(item.get("name"), str) else ""
        join_phrase = item.get("join_phrase") if isinstance(item.get("join_phrase"), str) else ""
        native_parts.append(name + join_phrase)
    native_joined = "".join(native_parts)

    artist = overrides.get("artist")
    if artist is not None:
        if artist != native_joined:
            release["artist_credit_native"] = native_joined
            release["artist_credit"] = artist
        return

    if not is_non_latin(native_joined):
        return

    rejoined: list[str] = []
    all_resolved = True
    for part in parts:
        item = part if isinstance(part, dict) else {}
        name = item.get("name") if isinstance(item.get("name"), str) else ""
        join_phrase = item.get("join_phrase") if isinstance(item.get("join_phrase"), str) else ""
        if is_non_latin(name):
            artist_id = item.get("id")
            aliases = _alias_call(artist_aliases, client, artist_id) if isinstance(artist_id, str) else None
            resolved = pick_alias(aliases)
            if resolved is not None:
                rejoined.append(resolved)
            else:
                all_resolved = False
                rejoined.append(name)
        else:
            rejoined.append(name)
        rejoined.append(join_phrase)

    if all_resolved:
        release["artist_credit_native"] = native_joined
        release["artist_credit"] = "".join(rejoined)
    else:
        unresolved.append({"field": "artist", "native": native_joined})


def _resolve_tracks(
    client: Any,
    release: dict[str, Any],
    overrides: dict[str, Any],
    unresolved: list[dict[str, Any]],
) -> None:
    tracks = release.get("tracks")
    if not isinstance(tracks, list):
        return
    manual_titles = overrides.get("tracks")
    if not isinstance(manual_titles, dict):
        manual_titles = {}

    per_position: dict[int, int] = {}
    for track in tracks:
        position = track.get("position") if isinstance(track, dict) else None
        if isinstance(position, int) and not isinstance(position, bool):
            per_position[position] = per_position.get(position, 0) + 1

    for track in tracks:
        if not isinstance(track, dict):
            continue
        position = track.get("position") if isinstance(track.get("position"), int) else None
        if isinstance(position, bool):
            position = None
        title = track.get("title") if isinstance(track.get("title"), str) else ""

        manual: str | None = None
        if position is not None:
            medium = track.get("medium_position")
            disc = medium if isinstance(medium, int) and not isinstance(medium, bool) else 1
            disc_key = f"{disc}:{position}"
            if disc_key in manual_titles:
                manual = manual_titles[disc_key]
            elif per_position.get(position, 0) > 1:
                if str(position) in manual_titles:
                    LOGGER.warning(
                        "--track-title %s= is ambiguous across discs in release %s; use <disc>:%s=",
                        position,
                        release.get("id"),
                        position,
                    )
                manual = None
            else:
                manual = manual_titles.get(str(position))

        if not isinstance(manual, str):
            manual = None
        if manual is not None:
            if manual != title:
                track["title_native"] = title
                track["title"] = manual
            continue

        if not is_non_latin(title):
            continue
        native = title
        recording_id = track.get("recording_id")
        aliases = (
            _alias_call(recording_aliases, client, recording_id)
            if isinstance(recording_id, str)
            else None
        )
        resolved = pick_alias(aliases)
        if resolved is not None:
            track["title_native"] = native
            track["title"] = resolved
        else:
            field = f"track {position}" if position is not None else "track (no position)"
            unresolved.append({"field": field, "native": native})


def resolve(
    client: Any,
    release: dict[str, Any],
    overrides: dict[str, Any],
) -> list[dict[str, Any]]:
    unresolved: list[dict[str, Any]] = []
    _resolve_album(client, release, overrides, unresolved)
    _resolve_artist(client, release, overrides, unresolved)

    if overrides.get("date") is not None:
        release["date"] = overrides["date"]
    if overrides.get("label") is not None:
        release["label"] = overrides["label"]
    if overrides.get("album_artist") is not None:
        release["album_artist"] = overrides["album_artist"]
    if overrides.get("compilation") is not None:
        release["compilation"] = overrides["compilation"]
    genres = merged_genres(overrides)
    if genres:
        release["genres"] = genres

    _resolve_tracks(client, release, overrides, unresolved)
    return unresolved


def format_unresolved(unresolved: list[dict[str, Any]]) -> str:
    return "\n".join(f"  {item.get('field')}: {item.get('native')}" for item in unresolved)
