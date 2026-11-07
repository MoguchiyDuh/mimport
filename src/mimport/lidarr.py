from __future__ import annotations

from typing import Any

from .errors import MimportError
from .http import LidarrClient

__all__ = [
    "LidarrClient",
    "search_artists",
    "lookup_artist",
    "lookup_album",
    "search_all",
]


def _obj(value: Any, what: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise MimportError(f"json error: expected object for {what}")
    return value


def _req_str(data: dict[str, Any], key: str) -> str:
    value = data.get(key)
    if not isinstance(value, str):
        raise MimportError(f"json error: missing field `{key}`")
    return value


def _opt_str(data: dict[str, Any], key: str) -> str | None:
    value = data.get(key)
    if value is None:
        return None
    if not isinstance(value, str):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return value


def _opt_int(data: dict[str, Any], key: str) -> int | None:
    value = data.get(key)
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, int):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return int(value)


def _req_float(data: dict[str, Any], key: str) -> float:
    value = data.get(key)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise MimportError(f"json error: missing field `{key}`")
    return float(value)


def _def_int(data: dict[str, Any], key: str) -> int:
    value = data.get(key)
    if value is None:
        return 0
    if isinstance(value, bool) or not isinstance(value, int):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return int(value)


def _def_str_list(data: dict[str, Any], key: str) -> list[str]:
    value = data.get(key)
    if value is None:
        return []
    if not isinstance(value, list):
        raise MimportError(f"json error: invalid type for field `{key}`")
    result: list[str] = []
    for item in value:
        if not isinstance(item, str):
            raise MimportError(f"json error: invalid type for field `{key}`")
        result.append(item)
    return result


def _def_list(data: dict[str, Any], key: str) -> list[Any]:
    value = data.get(key)
    if value is None:
        return []
    if not isinstance(value, list):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return value


def _album_summary(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "album summary")
    return {
        "Id": _req_str(data, "Id"),
        "Title": _req_str(data, "Title"),
        "Type": _opt_str(data, "Type"),
        "SecondaryTypes": _def_str_list(data, "SecondaryTypes"),
        "ReleaseStatuses": _def_str_list(data, "ReleaseStatuses"),
        "ReleaseDate": _opt_str(data, "ReleaseDate"),
    }


def _artist_resource(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "artist")
    return {
        "id": _req_str(data, "id"),
        "artistname": _req_str(data, "artistname"),
        "sortname": _opt_str(data, "sortname"),
        "disambiguation": _opt_str(data, "disambiguation"),
        "type": _opt_str(data, "type"),
        "status": _opt_str(data, "status"),
        "genres": _def_str_list(data, "genres"),
        "Albums": [_album_summary(item) for item in _def_list(data, "Albums")],
    }


def _media_resource(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "media")
    return {"Position": _opt_int(data, "Position"), "Format": _opt_str(data, "Format")}


def _track_resource(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "track")
    return {
        "Id": _req_str(data, "Id"),
        "RecordingId": _opt_str(data, "RecordingId"),
        "OldRecordingIds": _def_str_list(data, "OldRecordingIds"),
        "TrackName": _req_str(data, "TrackName"),
        "TrackNumber": _opt_str(data, "TrackNumber"),
        "DurationMs": _opt_int(data, "DurationMs"),
        "MediumNumber": _opt_int(data, "MediumNumber"),
    }


def _release_resource(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "release")
    return {
        "Id": _req_str(data, "Id"),
        "Title": _req_str(data, "Title"),
        "Status": _opt_str(data, "Status"),
        "Disambiguation": _opt_str(data, "Disambiguation"),
        "Country": _def_str_list(data, "Country"),
        "Label": _def_str_list(data, "Label"),
        "Media": [_media_resource(item) for item in _def_list(data, "Media")],
        "ReleaseDate": _opt_str(data, "ReleaseDate"),
        "TrackCount": _def_int(data, "TrackCount"),
        "Tracks": [_track_resource(item) for item in _def_list(data, "Tracks")],
    }


def _album_resource(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "album")
    return {
        "id": _req_str(data, "id"),
        "title": _req_str(data, "title"),
        "disambiguation": _opt_str(data, "disambiguation"),
        "releasedate": _opt_str(data, "releasedate"),
        "type": _opt_str(data, "type"),
        "secondarytypes": _def_str_list(data, "secondarytypes"),
        "Releases": [
            _release_resource(item) for item in _def_list(data, "Releases")
        ],
    }


def _search_result_item(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "search result")
    artist = data.get("artist")
    album = data.get("album")
    return {
        "score": _req_float(data, "score"),
        "artist": None if artist is None else _artist_resource(artist),
        "album": None if album is None else _album_resource(album),
    }


def search_artists(client: LidarrClient, query: str) -> list[dict[str, Any]]:
    response = client.get("/search", [("type", "artist"), ("query", query)])
    if not isinstance(response, list):
        raise MimportError("json error: expected array for artist search response")
    return [_artist_resource(item) for item in response]


def lookup_artist(client: LidarrClient, mbid: str) -> dict[str, Any]:
    return _artist_resource(client.get(f"/artist/{mbid}", []))


def lookup_album(client: LidarrClient, mbid: str) -> dict[str, Any]:
    return _album_resource(client.get(f"/album/{mbid}", []))


def search_all(client: LidarrClient, query: str) -> list[dict[str, Any]]:
    response = client.get("/search", [("type", "all"), ("query", query)])
    if not isinstance(response, list):
        raise MimportError("json error: expected array for search response")
    return [_search_result_item(item) for item in response]
