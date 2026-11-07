from __future__ import annotations

import re
from typing import Any

GENRES_KEEP = 3
GENRES_MAX = 4

_U32_RE = re.compile(r"^\+?[0-9]+$")
_U32_MAX = 4294967295


def _parse_u32(value: str) -> int | None:
    if not _U32_RE.fullmatch(value):
        return None
    number = int(value)
    if number > _U32_MAX:
        return None
    return number


def _list(value: Any) -> list[Any]:
    return value if isinstance(value, list) else []


def _dict(value: Any) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}


def _opt_str(value: Any) -> str | None:
    return value if isinstance(value, str) else None


def _opt_int(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    return value if isinstance(value, int) else None


def _title_case(text: str) -> str:
    out: list[str] = []
    upper = True
    for char in text:
        out.append(char.upper() if upper else char)
        upper = char in (" ", "-", "/")
    return "".join(out)


def _top_genres(group_genres: list[Any], release_genres: list[Any]) -> list[str]:
    source = group_genres if group_genres else release_genres
    ranked = [genre for genre in source if isinstance(genre, dict) and (_opt_int(genre.get("count")) or 0) > 0]
    ranked.sort(key=lambda genre: -(_opt_int(genre.get("count")) or 0))
    keep = min(len(ranked), GENRES_KEEP)
    while keep < min(len(ranked), GENRES_MAX):
        current = _opt_int(ranked[keep].get("count")) or 0
        previous = _opt_int(ranked[keep - 1].get("count")) or 0
        if current != previous:
            break
        keep += 1
    return [_title_case(genre.get("name") if isinstance(genre.get("name"), str) else "") for genre in ranked[:keep]]


def _artist_credit(raw_credits: list[Any]) -> str | None:
    if not raw_credits:
        return None
    out: list[str] = []
    for credit in raw_credits:
        item = _dict(credit)
        out.append(item.get("name") if isinstance(item.get("name"), str) else "")
        out.append(item.get("joinphrase") if isinstance(item.get("joinphrase"), str) else "")
    return "".join(out)


def _artist_credit_parts(raw_credits: list[Any]) -> list[dict[str, Any]]:
    parts: list[dict[str, Any]] = []
    for credit in raw_credits:
        item = _dict(credit)
        artist = _dict(item.get("artist"))
        parts.append(
            {
                "id": artist.get("id") if isinstance(artist.get("id"), str) else "",
                "name": item.get("name") if isinstance(item.get("name"), str) else "",
                "join_phrase": item.get("joinphrase") if isinstance(item.get("joinphrase"), str) else "",
            }
        )
    return parts


def from_mb(raw: dict[str, Any]) -> dict[str, Any]:
    media = _list(raw.get("media"))
    listed = 0
    present = 0
    formats: list[str] = []
    for medium in media:
        item = _dict(medium)
        count = _opt_int(item.get("track-count")) or 0
        listed += count
        tracks = _list(item.get("tracks"))
        present += len(tracks)
        fmt = _opt_str(item.get("format"))
        if fmt is not None:
            formats.append(fmt)
    track_count = max(listed, present)

    label: str | None = None
    label_info = _list(raw.get("label-info"))
    if label_info:
        first = _dict(label_info[0])
        label_obj = first.get("label")
        if isinstance(label_obj, dict):
            label = _opt_str(label_obj.get("name"))

    raw_credits = _list(raw.get("artist-credit"))
    artist_credit = _artist_credit(raw_credits)
    artist_credit_parts = _artist_credit_parts(raw_credits)

    raw_group = raw.get("release-group")
    group = _dict(raw_group) if isinstance(raw_group, dict) else None
    release_group_id = _opt_str(group.get("id")) if group is not None else None
    group_genres = _list(group.get("genres")) if group is not None else []
    genres = _top_genres(group_genres, _list(raw.get("genres")))

    tracks: list[dict[str, Any]] = []
    for medium in media:
        item = _dict(medium)
        medium_format = _opt_str(item.get("format"))
        medium_position = _opt_int(item.get("position"))
        for index, track in enumerate(_list(item.get("tracks"))):
            entry = _dict(track)
            number = entry.get("number") if isinstance(entry.get("number"), str) else ""
            position = _parse_u32(number)
            if position is None:
                position = index + 1
            recording = _dict(entry.get("recording"))
            length = _opt_int(entry.get("length"))
            if length is None:
                length = _opt_int(recording.get("length"))
            recording_id = recording.get("id") if isinstance(recording.get("id"), str) else None
            tracks.append(
                {
                    "position": position,
                    "title": entry.get("title") if isinstance(entry.get("title"), str) else "",
                    "length_ms": length,
                    "recording_id": recording_id,
                    "medium_format": medium_format,
                    "medium_position": medium_position,
                    "raw_position": number,
                    "title_native": None,
                }
            )

    return {
        "id": raw.get("id") if isinstance(raw.get("id"), str) else "",
        "title": raw.get("title") if isinstance(raw.get("title"), str) else "",
        "status": _opt_str(raw.get("status")),
        "country": _opt_str(raw.get("country")),
        "disambiguation": _opt_str(raw.get("disambiguation")),
        "label": label,
        "formats": formats,
        "track_count": track_count,
        "date": _opt_str(raw.get("date")),
        "artist_credit": artist_credit,
        "artist_credit_parts": artist_credit_parts,
        "release_group_id": release_group_id,
        "title_native": None,
        "artist_credit_native": None,
        "album_artist": None,
        "compilation": False,
        "genres": genres,
        "tracks": tracks,
    }


def from_lidarr(raw: dict[str, Any]) -> dict[str, Any]:
    media = _list(raw.get("Media"))
    formats: list[str] = []
    for medium in media:
        fmt = _opt_str(_dict(medium).get("Format"))
        if fmt is not None:
            formats.append(fmt)

    country_list = _list(raw.get("Country"))
    country = country_list[0] if country_list and isinstance(country_list[0], str) else None
    label_list = _list(raw.get("Label"))
    label = label_list[0] if label_list and isinstance(label_list[0], str) else None

    ordinals: dict[int, int] = {}
    tracks: list[dict[str, Any]] = []
    for track in _list(raw.get("Tracks")):
        entry = _dict(track)
        medium_number = _opt_int(entry.get("MediumNumber"))
        if medium_number is None:
            medium_number = 1
        ordinal = ordinals.get(medium_number, 0) + 1
        ordinals[medium_number] = ordinal

        medium_format: str | None = None
        for medium in media:
            item = _dict(medium)
            if _opt_int(item.get("Position")) == medium_number:
                medium_format = _opt_str(item.get("Format"))
                break

        raw_number = entry.get("TrackNumber")
        position = _parse_u32(raw_number) if isinstance(raw_number, str) else None
        if position is None:
            position = ordinal

        tracks.append(
            {
                "position": position,
                "title": entry.get("TrackName") if isinstance(entry.get("TrackName"), str) else "",
                "length_ms": _opt_int(entry.get("DurationMs")),
                "recording_id": _opt_str(entry.get("RecordingId")),
                "medium_format": medium_format,
                "medium_position": _opt_int(entry.get("MediumNumber")),
                "raw_position": None,
                "title_native": None,
            }
        )

    return {
        "id": raw.get("Id") if isinstance(raw.get("Id"), str) else "",
        "title": raw.get("Title") if isinstance(raw.get("Title"), str) else "",
        "status": _opt_str(raw.get("Status")),
        "country": country,
        "disambiguation": _opt_str(raw.get("Disambiguation")),
        "label": label,
        "formats": formats,
        "track_count": _opt_int(raw.get("TrackCount")) or 0,
        "date": _opt_str(raw.get("ReleaseDate")),
        "artist_credit": None,
        "artist_credit_parts": [],
        "release_group_id": None,
        "title_native": None,
        "artist_credit_native": None,
        "album_artist": None,
        "compilation": False,
        "genres": [],
        "tracks": tracks,
    }


def group_from_mb(raw: dict[str, Any]) -> dict[str, Any]:
    return {
        "primary_type": _opt_str(raw.get("primary-type")),
        "secondary_types": [item for item in _list(raw.get("secondary-types")) if isinstance(item, str)],
    }


def group_from_lidarr(raw: dict[str, Any]) -> dict[str, Any]:
    return {
        "primary_type": _opt_str(raw.get("type")),
        "secondary_types": [item for item in _list(raw.get("secondarytypes")) if isinstance(item, str)],
    }


def year(release: dict[str, Any]) -> str | None:
    date = release.get("date")
    if not isinstance(date, str):
        return None
    return date.split("-", 1)[0]


def empty_release() -> dict[str, Any]:
    return {
        "id": "",
        "title": "",
        "status": None,
        "country": None,
        "disambiguation": None,
        "label": None,
        "formats": [],
        "track_count": 0,
        "date": None,
        "artist_credit": None,
        "artist_credit_parts": [],
        "release_group_id": None,
        "title_native": None,
        "artist_credit_native": None,
        "album_artist": None,
        "compilation": False,
        "genres": [],
        "tracks": [],
    }
