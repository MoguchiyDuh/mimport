from __future__ import annotations

from typing import Any

from .errors import MimportError
from .http import MbClient

__all__ = [
    "MbClient",
    "escape_lucene",
    "search_artists",
    "lookup_artist",
    "release_group_releases",
    "lookup_release_group",
    "release_with_tracks",
    "search_recordings",
    "artist_aliases",
    "release_group_aliases",
    "recording_aliases",
    "join_artist_credit",
]

RELEASES_PAGE = 100
RELEASES_MAX = 1000


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


def _opt_bool(data: dict[str, Any], key: str) -> bool | None:
    value = data.get(key)
    if value is None:
        return None
    if not isinstance(value, bool):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return value


def _def_int(data: dict[str, Any], key: str) -> int:
    value = data.get(key)
    if value is None:
        return 0
    if isinstance(value, bool) or not isinstance(value, int):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return int(value)


def _def_str(data: dict[str, Any], key: str) -> str:
    value = data.get(key)
    if value is None:
        return ""
    if not isinstance(value, str):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return value


def _def_list(data: dict[str, Any], key: str) -> list[Any]:
    value = data.get(key)
    if value is None:
        return []
    if not isinstance(value, list):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return value


def _req_list(data: dict[str, Any], key: str) -> list[Any]:
    value = data.get(key)
    if not isinstance(value, list):
        raise MimportError(f"json error: missing field `{key}`")
    return value


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


def _artist_credit_artist(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "artist-credit.artist")
    return {"id": _req_str(data, "id")}


def _artist_credit_item(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "artist-credit")
    return {
        "name": _req_str(data, "name"),
        "joinphrase": _def_str(data, "joinphrase"),
        "artist": _artist_credit_artist(data.get("artist")),
    }


def _genre(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "genre")
    return {"name": _req_str(data, "name"), "count": _def_int(data, "count")}


def _label(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "label")
    return {"name": _req_str(data, "name")}


def _label_info(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "label-info")
    label = data.get("label")
    return {"label": None if label is None else _label(label)}


def _recording(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "recording")
    return {
        "id": _req_str(data, "id"),
        "title": _req_str(data, "title"),
        "length": _opt_int(data, "length"),
    }


def _track(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "track")
    return {
        "id": _req_str(data, "id"),
        "number": _req_str(data, "number"),
        "title": _req_str(data, "title"),
        "length": _opt_int(data, "length"),
        "recording": _recording(data.get("recording")),
    }


def _media(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "media")
    return {
        "format": _opt_str(data, "format"),
        "position": _opt_int(data, "position"),
        "track-count": _def_int(data, "track-count"),
        "tracks": [_track(t) for t in _def_list(data, "tracks")],
    }


def _release_group_ref(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "release-group")
    return {
        "id": _req_str(data, "id"),
        "genres": [_genre(g) for g in _def_list(data, "genres")],
    }


def _release(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "release")
    release_group = data.get("release-group")
    return {
        "id": _req_str(data, "id"),
        "title": _req_str(data, "title"),
        "status": _opt_str(data, "status"),
        "date": _opt_str(data, "date"),
        "country": _opt_str(data, "country"),
        "disambiguation": _opt_str(data, "disambiguation"),
        "media": [_media(m) for m in _def_list(data, "media")],
        "label-info": [_label_info(li) for li in _def_list(data, "label-info")],
        "artist-credit": [
            _artist_credit_item(c) for c in _def_list(data, "artist-credit")
        ],
        "release-group": (
            None if release_group is None else _release_group_ref(release_group)
        ),
        "genres": [_genre(g) for g in _def_list(data, "genres")],
    }


def _artist_search_result(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "artist")
    return {
        "id": _req_str(data, "id"),
        "name": _req_str(data, "name"),
        "sort-name": _opt_str(data, "sort-name"),
        "disambiguation": _opt_str(data, "disambiguation"),
        "country": _opt_str(data, "country"),
        "type": _opt_str(data, "type"),
        "score": _opt_int(data, "score"),
    }


def _artist(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "artist")
    return {
        "id": _req_str(data, "id"),
        "name": _req_str(data, "name"),
        "sort-name": _opt_str(data, "sort-name"),
        "disambiguation": _opt_str(data, "disambiguation"),
        "country": _opt_str(data, "country"),
        "type": _opt_str(data, "type"),
    }


def _recording_search_result(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "recording")
    return {
        "id": _req_str(data, "id"),
        "title": _req_str(data, "title"),
        "length": _opt_int(data, "length"),
        "score": _opt_int(data, "score"),
        "artist-credit": [
            _artist_credit_item(c) for c in _def_list(data, "artist-credit")
        ],
    }


def _release_group(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "release-group")
    return {
        "id": _req_str(data, "id"),
        "title": _req_str(data, "title"),
        "primary-type": _opt_str(data, "primary-type"),
        "secondary-types": _def_str_list(data, "secondary-types"),
    }


def _alias(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "alias")
    return {
        "name": _req_str(data, "name"),
        "locale": _opt_str(data, "locale"),
        "type": _opt_str(data, "type"),
        "primary": _opt_bool(data, "primary"),
    }


def join_artist_credit(credits: list[dict[str, Any]]) -> str | None:
    if not credits:
        return None
    out: list[str] = []
    for credit in credits:
        out.append(_req_str(credit, "name"))
        out.append(_def_str(credit, "joinphrase"))
    return "".join(out)


def escape_lucene(text: str) -> str:
    special = '+-&|!(){}[]^"~*?:\\/'
    out: list[str] = []
    for char in text:
        if char in special:
            out.append("\\")
        out.append(char)
    return "".join(out)


def _lucene_phrase(text: str) -> str:
    out: list[str] = ['"']
    for char in text:
        if char in ('"', "\\"):
            out.append("\\")
        out.append(char)
    out.append('"')
    return "".join(out)


def search_artists(client: MbClient, query: str) -> list[dict[str, Any]]:
    if not query.strip():
        return []
    q = f"artist:{_lucene_phrase(query)}"
    response = client.get("/artist", [("query", q), ("limit", "10")])
    data = _obj(response, "artist search response")
    return [_artist_search_result(item) for item in _req_list(data, "artists")]


def lookup_artist(client: MbClient, mbid: str) -> dict[str, Any]:
    return _artist(client.get(f"/artist/{mbid}", []))


def release_group_releases(
    client: MbClient, release_group_mbid: str
) -> list[dict[str, Any]]:
    all_releases: list[dict[str, Any]] = []
    offset = 0
    while True:
        response = client.get(
            "/release",
            [
                ("release-group", release_group_mbid),
                ("inc", "media+labels+recordings+artist-credits"),
                ("limit", "100"),
                ("offset", str(offset)),
            ],
        )
        data = _obj(response, "release browse response")
        releases = _req_list(data, "releases")
        count = len(releases)
        all_releases.extend(_release(item) for item in releases)
        offset += count
        if count < RELEASES_PAGE or offset >= RELEASES_MAX:
            break
    return all_releases


def lookup_release_group(client: MbClient, release_group_mbid: str) -> dict[str, Any]:
    return _release_group(client.get(f"/release-group/{release_group_mbid}", []))


def release_with_tracks(client: MbClient, release_mbid: str) -> dict[str, Any]:
    return _release(
        client.get(
            f"/release/{release_mbid}",
            [
                (
                    "inc",
                    "media+labels+recordings+artist-credits+release-groups+genres",
                )
            ],
        )
    )


def search_recordings(client: MbClient, query: str) -> list[dict[str, Any]]:
    if not query.strip():
        return []
    q = f"recording:{_lucene_phrase(query)}"
    response = client.get("/recording", [("query", q), ("limit", "10")])
    data = _obj(response, "recording search response")
    return [_recording_search_result(item) for item in _req_list(data, "recordings")]


def _aliases(client: MbClient, path: str) -> list[dict[str, Any]]:
    response = client.get(path, [("inc", "aliases")])
    data = _obj(response, "alias response")
    return [_alias(item) for item in _def_list(data, "aliases")]


def artist_aliases(client: MbClient, artist_mbid: str) -> list[dict[str, Any]]:
    return _aliases(client, f"/artist/{artist_mbid}")


def release_group_aliases(
    client: MbClient, release_group_mbid: str
) -> list[dict[str, Any]]:
    return _aliases(client, f"/release-group/{release_group_mbid}")


def recording_aliases(client: MbClient, recording_mbid: str) -> list[dict[str, Any]]:
    return _aliases(client, f"/recording/{recording_mbid}")
