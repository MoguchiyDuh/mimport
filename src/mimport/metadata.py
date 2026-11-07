from __future__ import annotations

import base64
from io import BytesIO
from pathlib import Path
from typing import Any

from mutagen.apev2 import APEValue, BINARY
from mutagen.flac import FLAC, Picture
from mutagen.id3 import (
    APIC,
    COMM,
    TALB,
    TCMP,
    TCON,
    TDRC,
    TIT2,
    TOPE,
    TOAL,
    TPE1,
    TPE2,
    TPOS,
    TPUB,
    TRCK,
    TXXX,
)
from mutagen.monkeysaudio import MonkeysAudio
from mutagen.mp3 import MP3
from mutagen.mp4 import MP4, MP4Cover, MP4FreeForm
from mutagen.oggopus import OggOpus
from mutagen.oggvorbis import OggVorbis
from mutagen.wave import WAVE
from mutagen.wavpack import WavPack

from .coverart import CoverArt
from .errors import MimportError

_VORBIS_EXTENSIONS = ("flac", "opus", "ogg")
_ID3_EXTENSIONS = ("mp3", "wav")
_MP4_EXTENSIONS = ("m4a", "mp4")
_APE_EXTENSIONS = ("ape", "wv")


def _ext(path: Path) -> str:
    return path.suffix[1:].lower() if path.suffix else ""


def _norm_key(value: str) -> str:
    return "".join(ch for ch in value if ch not in " _").upper()


def _looks_like_mbid(value: str) -> bool:
    return len(value) == 36 and value.count("-") == 4


def _int_or_none(value: Any) -> int | None:
    if value is None:
        return None
    if isinstance(value, bool):
        return None
    try:
        return int(str(value).split("/", 1)[0])
    except (TypeError, ValueError):
        return None


def _first(values: Any) -> str | None:
    if values is None:
        return None
    if isinstance(values, (list, tuple)):
        if not values:
            return None
        return str(values[0])
    return str(values)


def _text_list(values: Any) -> list[str]:
    if values is None:
        return []
    if isinstance(values, (list, tuple)):
        return [str(value) for value in values]
    return [str(values)]


def _load(path: Path) -> Any:
    ext = _ext(path)
    if ext == "flac":
        return FLAC(path)
    if ext == "opus":
        return OggOpus(path)
    if ext == "ogg":
        import mutagen

        audio = mutagen.File(path)
        if audio is None:
            raise MimportError(f"probe/tag error at {path}: unsupported audio format")
        return audio
    if ext == "mp3":
        return MP3(path)
    if ext in _MP4_EXTENSIONS:
        return MP4(path)
    if ext == "wav":
        return WAVE(path)
    if ext == "ape":
        return MonkeysAudio(path)
    if ext == "wv":
        return WavPack(path)
    import mutagen

    audio = mutagen.File(path)
    if audio is None:
        raise MimportError(f"probe/tag error at {path}: unsupported audio format")
    return audio


def _ensure_tags(obj: Any) -> Any:
    if getattr(obj, "tags", None) is None:
        obj.add_tags()
    return obj.tags


def _clear_vorbis(obj: Any) -> Any:
    tags = _ensure_tags(obj)
    preserved: dict[str, list[str]] = {}
    if isinstance(obj, FLAC):
        for key in ("COMMENT", "LYRICS"):
            values = _vorbis_values(tags, key)
            if values:
                preserved[key] = values
    for key in list(tags.keys()):
        del tags[key]
    if isinstance(obj, FLAC):
        obj.clear_pictures()
        for key, values in preserved.items():
            tags[key] = values
    return tags


def _clear_id3(obj: Any) -> Any:
    tags = _ensure_tags(obj)
    for key in list(tags.keys()):
        del tags[key]
    return tags


def _clear_mp4(obj: Any) -> Any:
    tags = _ensure_tags(obj)
    for key in list(tags.keys()):
        del tags[key]
    return tags


def _clear_ape(obj: Any) -> Any:
    tags = _ensure_tags(obj)
    for key in list(tags.keys()):
        del tags[key]
    return tags


def _vorbis_values(tags: Any, key: str) -> list[str]:
    if tags is None:
        return []
    target = _norm_key(key)
    for existing, values in tags.items():
        if _norm_key(str(existing)) == target:
            return _text_list(values)
    return []


def _vorbis_first(tags: Any, key: str) -> str | None:
    return _first(_vorbis_values(tags, key))


def _vorbis_set(tags: Any, key: str, value: str | None) -> None:
    if value is not None:
        tags[key] = [value]


def _vorbis_set_list(tags: Any, key: str, values: list[str]) -> None:
    if values:
        tags[key] = list(values)


def _id3_text(tags: Any, frame_id: str) -> str | None:
    if tags is None:
        return None
    frame = tags.get(frame_id)
    if frame is None:
        return None
    return _first(getattr(frame, "text", None))


def _id3_txxx(tags: Any, desc: str) -> str | None:
    if tags is None:
        return None
    target = _norm_key(desc)
    for frame in tags.getall("TXXX"):
        if _norm_key(str(getattr(frame, "desc", ""))) == target:
            return _first(getattr(frame, "text", None))
    return None


def _id3_comment(tags: Any) -> str | None:
    if tags is None:
        return None
    for frame in tags.getall("COMM"):
        value = _first(getattr(frame, "text", None))
        if value is not None:
            return value
    return None


def _id3_ufid(tags: Any) -> str | None:
    if tags is None:
        return None
    for frame in tags.getall("UFID"):
        if str(getattr(frame, "owner", "")) == "http://musicbrainz.org":
            data = getattr(frame, "data", None)
            if data is not None:
                try:
                    return bytes(data).decode("ascii")
                except (UnicodeDecodeError, TypeError, ValueError):
                    return None
    return None


def _id3_track_pair(tags: Any) -> tuple[int | None, int | None]:
    value = _id3_text(tags, "TRCK")
    if value is None:
        return (None, None)
    parts = value.split("/", 1)
    number = _int_or_none(parts[0])
    total = _int_or_none(parts[1]) if len(parts) > 1 else None
    return (number, total)


def _id3_disc_pair(tags: Any) -> tuple[int | None, int | None]:
    value = _id3_text(tags, "TPOS")
    if value is None:
        return (None, None)
    parts = value.split("/", 1)
    number = _int_or_none(parts[0])
    total = _int_or_none(parts[1]) if len(parts) > 1 else None
    return (number, total)


def _read_id3(tags: Any) -> dict[str, Any]:
    if tags is None:
        return {}
    track_number, track_total = _id3_track_pair(tags)
    disc_number, _ = _id3_disc_pair(tags)
    compilation_value = _id3_text(tags, "TCMP")
    original_title = _id3_txxx(tags, "ORIGINALTITLE")
    if original_title is None:
        comment = _id3_comment(tags)
        if comment is not None and comment.startswith("Original title: "):
            original_title = comment[len("Original title: ") :]
    return {
        "title": _id3_text(tags, "TIT2"),
        "artist": _id3_text(tags, "TPE1"),
        "album": _id3_text(tags, "TALB"),
        "album_artist": _id3_text(tags, "TPE2"),
        "compilation": compilation_value is not None
        and compilation_value.lower() in ("1", "true"),
        "track_number": track_number,
        "track_total": track_total,
        "disc_number": disc_number,
        "date": _id3_text(tags, "TDRC"),
        "label": _id3_text(tags, "TPUB"),
        "genres": _text_list(getattr(tags.get("TCON"), "text", None)),
        "musicbrainz_trackid": _id3_txxx(tags, "MUSICBRAINZ_TRACKID")
        or _id3_ufid(tags),
        "musicbrainz_albumid": _id3_txxx(tags, "MUSICBRAINZ_ALBUMID"),
        "original_artist": _id3_text(tags, "TOPE"),
        "original_album": _id3_text(tags, "TOAL"),
        "original_title": original_title,
        "source_url": _id3_txxx(tags, "SOURCEURL"),
        "comment": _id3_comment(tags),
    }


def _mp4_text(tags: Any, key: str) -> str | None:
    if tags is None:
        return None
    return _first(tags.get(key))


def _mp4_text_list(tags: Any, key: str) -> list[str]:
    if tags is None:
        return []
    return _text_list(tags.get(key))


def _mp4_freeform(tags: Any, name: str) -> str | None:
    if tags is None:
        return None
    target = _norm_key(name)
    for key, values in tags.items():
        if not isinstance(key, str) or not key.startswith("----:"):
            continue
        parts = key.split(":", 2)
        if len(parts) != 3 or _norm_key(parts[2]) != target:
            continue
        if isinstance(values, (list, tuple)):
            value = values[0] if values else None
        else:
            value = values
        if value is None:
            continue
        if isinstance(value, bytes):
            try:
                return value.decode("utf-8")
            except UnicodeDecodeError:
                return None
        return str(value)
    return None


def _read_mp4(tags: Any) -> dict[str, Any]:
    if tags is None:
        return {}
    track_number = None
    track_total = None
    track_values = tags.get("trkn")
    if track_values:
        pair = track_values[0]
        if isinstance(pair, tuple) and len(pair) >= 2:
            track_number = int(pair[0]) or None
            track_total = int(pair[1]) or None
    disc_number = None
    disc_values = tags.get("disk")
    if disc_values:
        pair = disc_values[0]
        if isinstance(pair, tuple) and len(pair) >= 1:
            disc_number = int(pair[0])
    comment = _mp4_text(tags, "\xa9cmt")
    original_title = _mp4_freeform(tags, "ORIGINAL TITLE")
    if original_title is None and comment is not None and comment.startswith("Original title: "):
        original_title = comment[len("Original title: ") :]
    return {
        "title": _mp4_text(tags, "\xa9nam"),
        "artist": _mp4_text(tags, "\xa9ART"),
        "album": _mp4_text(tags, "\xa9alb"),
        "album_artist": _mp4_text(tags, "aART"),
        "compilation": bool(tags.get("cpil")),
        "track_number": track_number,
        "track_total": track_total,
        "disc_number": disc_number,
        "date": _mp4_text(tags, "\xa9day"),
        "label": _mp4_freeform(tags, "LABEL"),
        "genres": _mp4_text_list(tags, "\xa9gen"),
        "musicbrainz_trackid": _mp4_freeform(tags, "MusicBrainz Track Id"),
        "musicbrainz_albumid": _mp4_freeform(tags, "MusicBrainz Album Id"),
        "original_artist": _mp4_freeform(tags, "ORIGINAL ARTIST"),
        "original_album": _mp4_freeform(tags, "ORIGINAL ALBUM"),
        "original_title": original_title,
        "source_url": _mp4_freeform(tags, "SOURCEURL"),
        "comment": comment,
    }


def _ape_value(tags: Any, key: str) -> Any:
    if tags is None:
        return None
    target = _norm_key(key)
    for existing, value in tags.items():
        if _norm_key(str(existing)) == target:
            return value
    return None


def _ape_text(tags: Any, key: str) -> str | None:
    value = _ape_value(tags, key)
    if value is None:
        return None
    if getattr(value, "kind", None) == BINARY:
        return None
    return str(value).split("\0", 1)[0]


def _ape_text_list(tags: Any, key: str) -> list[str]:
    value = _ape_value(tags, key)
    if value is None or getattr(value, "kind", None) == BINARY:
        return []
    return str(value).split("\0")


def _read_ape(tags: Any) -> dict[str, Any]:
    if tags is None:
        return {}
    comment = _ape_text(tags, "COMMENT")
    original_title = _ape_text(tags, "ORIGINALTITLE")
    if original_title is None and comment is not None and comment.startswith("Original title: "):
        original_title = comment[len("Original title: ") :]
    return {
        "title": _ape_text(tags, "TITLE"),
        "artist": _ape_text(tags, "ARTIST"),
        "album": _ape_text(tags, "ALBUM"),
        "album_artist": _ape_text(tags, "ALBUMARTIST"),
        "compilation": (_ape_text(tags, "COMPILATION") or "").lower()
        in ("1", "true"),
        "track_number": _int_or_none(_ape_text(tags, "TRACKNUMBER")),
        "track_total": _int_or_none(_ape_text(tags, "TRACKTOTAL")),
        "disc_number": _int_or_none(_ape_text(tags, "DISCNUMBER")),
        "date": _ape_text(tags, "DATE"),
        "label": _ape_text(tags, "LABEL"),
        "genres": _ape_text_list(tags, "GENRE"),
        "musicbrainz_trackid": _ape_text(tags, "MUSICBRAINZ_TRACKID"),
        "musicbrainz_albumid": _ape_text(tags, "MUSICBRAINZ_ALBUMID"),
        "original_artist": _ape_text(tags, "ORIGINALARTIST"),
        "original_album": _ape_text(tags, "ORIGINALALBUMTITLE"),
        "original_title": original_title,
        "source_url": _ape_text(tags, "SOURCEURL"),
        "comment": comment,
    }


def _read_vorbis(obj: Any) -> dict[str, Any]:
    tags = getattr(obj, "tags", None)
    return {
        "title": _vorbis_first(tags, "TITLE"),
        "artist": _vorbis_first(tags, "ARTIST"),
        "album": _vorbis_first(tags, "ALBUM"),
        "album_artist": _vorbis_first(tags, "ALBUMARTIST")
        or _vorbis_first(tags, "ALBUM ARTIST"),
        "compilation": (_vorbis_first(tags, "COMPILATION") or "").lower()
        in ("1", "true"),
        "track_number": _int_or_none(_vorbis_first(tags, "TRACKNUMBER")),
        "track_total": _int_or_none(_vorbis_first(tags, "TRACKTOTAL")),
        "disc_number": _int_or_none(_vorbis_first(tags, "DISCNUMBER")),
        "date": _vorbis_first(tags, "DATE"),
        "label": _vorbis_first(tags, "LABEL"),
        "genres": _vorbis_values(tags, "GENRE"),
        "musicbrainz_trackid": _vorbis_first(tags, "MUSICBRAINZ_TRACKID"),
        "musicbrainz_albumid": _vorbis_first(tags, "MUSICBRAINZ_ALBUMID"),
        "original_artist": _vorbis_first(tags, "ORIGINALARTIST"),
        "original_album": _vorbis_first(tags, "ORIGINALALBUMTITLE"),
        "original_title": _vorbis_first(tags, "ORIGINALTITLE"),
        "source_url": _vorbis_first(tags, "SOURCEURL"),
        "comment": _vorbis_first(tags, "COMMENT"),
    }


def _read_logical_with_info(path: Path) -> tuple[dict[str, Any], Any]:
    obj = _load(path)
    ext = _ext(path)
    if isinstance(obj, (FLAC, OggVorbis, OggOpus)) or ext in _VORBIS_EXTENSIONS:
        logical = _read_vorbis(obj)
    elif isinstance(obj, (MP3, WAVE)) or ext in _ID3_EXTENSIONS:
        logical = _read_id3(getattr(obj, "tags", None))
    elif isinstance(obj, MP4) or ext in _MP4_EXTENSIONS:
        logical = _read_mp4(getattr(obj, "tags", None))
    elif isinstance(obj, (MonkeysAudio, WavPack)) or ext in _APE_EXTENSIONS:
        logical = _read_ape(getattr(obj, "tags", None))
    else:
        logical = {}
    return logical, getattr(obj, "info", None)


def _read_logical(path: Path) -> dict[str, Any]:
    logical, _ = _read_logical_with_info(path)
    return logical


def _filename_hint(path: Path) -> tuple[str | None, int | None]:
    stem = path.stem
    index = stem.find("-")
    if index >= 0:
        head = stem[:index].strip()
        if head.isascii() and head.isdigit():
            number = int(head)
            if number <= 0xFFFFFFFF:
                title = stem[index + 1 :].strip()
                return (title or None, number)
    return (stem or None, None)


def read_track(path: Path) -> dict[str, Any]:
    try:
        logical, info = _read_logical_with_info(path)
    except MimportError:
        raise
    except Exception as exc:
        raise MimportError(f"probe/tag error at {path}: {exc}")
    duration_secs = None
    if info is not None and getattr(info, "length", None) is not None:
        try:
            duration_secs = float(info.length)
        except (TypeError, ValueError):
            duration_secs = None
    filename_title, filename_track = _filename_hint(path)
    title = logical.get("title")
    track_number = logical.get("track_number")
    return {
        "path": str(path),
        "title": title if title is not None else filename_title,
        "artist": logical.get("artist"),
        "track_number": track_number
        if track_number is not None
        else filename_track,
        "duration_secs": duration_secs,
        "musicbrainz_recording_id": logical.get("musicbrainz_trackid"),
    }


def _disc_number(logical: dict[str, Any]) -> int:
    value = logical.get("disc_number")
    return int(value) if value is not None else 1


def _disc_total(release: dict[str, Any], matched: dict[str, Any]) -> int:
    medium = matched.get("medium_position")
    tracks = release.get("tracks") or []
    if medium is not None:
        return sum(
            1 for track in tracks if track.get("medium_position") == medium
        )
    try:
        return int(release.get("track_count") or 0)
    except (TypeError, ValueError):
        return 0


def _logical_from_write(
    matched: dict[str, Any], release: dict[str, Any]
) -> dict[str, Any]:
    artist = release.get("artist_credit")
    album_artist = release.get("album_artist")
    if album_artist is None:
        album_artist = artist
    position = matched.get("position")
    raw_position = matched.get("raw_position")
    if position is not None:
        track_number: str | None = str(int(position))
    elif raw_position is not None:
        track_number = str(raw_position)
    else:
        track_number = None
    disc_total = _disc_total(release, matched)
    medium_position = matched.get("medium_position")
    disc_number = int(medium_position) if medium_position is not None else 1
    release_id = str(release.get("id") or "")
    logical = {
        "title": str(matched.get("title") or ""),
        "album": str(release.get("title") or ""),
        "artist": artist,
        "album_artist": album_artist,
        "compilation": bool(release.get("compilation")),
        "track_number": track_number,
        "track_total": disc_total if disc_total > 0 else None,
        "disc_number": int(disc_number),
        "date": release.get("date"),
        "label": release.get("label"),
        "genres": [str(genre) for genre in (release.get("genres") or [])],
        "musicbrainz_trackid": matched.get("recording_id"),
        "musicbrainz_albumid": release_id if _looks_like_mbid(release_id) else None,
        "original_artist": release.get("artist_credit_native"),
        "original_album": release.get("title_native"),
        "original_title": matched.get("title_native"),
        "source_url": matched.get("source_url"),
    }
    return logical


def _picture(cover: CoverArt) -> Picture:
    picture = Picture()
    picture.type = 3
    picture.mime = cover.mime
    picture.desc = ""
    picture.data = cover.bytes
    return picture


def _vorbis_picture_value(cover: CoverArt) -> str:
    return base64.b64encode(_picture(cover).write()).decode("ascii")


def _write_vorbis(obj: Any, logical: dict[str, Any], cover: CoverArt | None) -> None:
    tags = _clear_vorbis(obj)
    _vorbis_set(tags, "TITLE", logical.get("title"))
    _vorbis_set(tags, "ALBUM", logical.get("album"))
    _vorbis_set(tags, "ARTIST", logical.get("artist"))
    _vorbis_set(tags, "ALBUMARTIST", logical.get("album_artist"))
    if logical.get("compilation"):
        _vorbis_set(tags, "COMPILATION", "1")
    _vorbis_set(tags, "TRACKNUMBER", logical.get("track_number"))
    if logical.get("track_total") is not None:
        _vorbis_set(tags, "TRACKTOTAL", str(logical["track_total"]))
    _vorbis_set(tags, "DISCNUMBER", str(_disc_number(logical)))
    date = logical.get("date")
    if date is not None and date != "":
        _vorbis_set(tags, "DATE", str(date))
    if logical.get("label") is not None:
        _vorbis_set(tags, "LABEL", str(logical["label"]))
    _vorbis_set_list(tags, "GENRE", logical.get("genres") or [])
    _vorbis_set(tags, "MUSICBRAINZ_TRACKID", logical.get("musicbrainz_trackid"))
    _vorbis_set(tags, "MUSICBRAINZ_ALBUMID", logical.get("musicbrainz_albumid"))
    _vorbis_set(tags, "ORIGINALARTIST", logical.get("original_artist"))
    _vorbis_set(tags, "ORIGINALALBUMTITLE", logical.get("original_album"))
    _vorbis_set(tags, "ORIGINALTITLE", logical.get("original_title"))
    _vorbis_set(tags, "SOURCEURL", logical.get("source_url"))
    if cover is not None:
        if isinstance(obj, FLAC):
            obj.add_picture(_picture(cover))
        else:
            tags["METADATA_BLOCK_PICTURE"] = [_vorbis_picture_value(cover)]


def _id3_track_text(logical: dict[str, Any]) -> str | None:
    track = logical.get("track_number")
    total = logical.get("track_total")
    if total is not None:
        return f"{track if track is not None else 0}/{total}"
    if track is not None:
        return str(track)
    return None


def _write_id3(tags: Any, logical: dict[str, Any], cover: CoverArt | None) -> None:
    title = logical.get("title")
    if title is not None:
        tags.add(TIT2(encoding=3, text=[str(title)]))
    album = logical.get("album")
    if album is not None:
        tags.add(TALB(encoding=3, text=[str(album)]))
    artist = logical.get("artist")
    if artist is not None:
        tags.add(TPE1(encoding=3, text=[str(artist)]))
    album_artist = logical.get("album_artist")
    if album_artist is not None:
        tags.add(TPE2(encoding=3, text=[str(album_artist)]))
    if logical.get("compilation"):
        tags.add(TCMP(encoding=3, text=["1"]))
    track_text = _id3_track_text(logical)
    if track_text is not None:
        tags.add(TRCK(encoding=3, text=[track_text]))
    tags.add(TPOS(encoding=3, text=[str(_disc_number(logical))]))
    date = logical.get("date")
    if date is not None and date != "":
        tags.add(TDRC(encoding=3, text=[str(date)]))
    label = logical.get("label")
    if label is not None:
        tags.add(TPUB(encoding=3, text=[str(label)]))
    genres = logical.get("genres") or []
    if genres:
        tags.add(TCON(encoding=3, text=[str(genre) for genre in genres]))
    recording_id = logical.get("musicbrainz_trackid")
    if recording_id is not None:
        tags.add(
            TXXX(
                encoding=3,
                desc="MUSICBRAINZ_TRACKID",
                text=[str(recording_id)],
            )
        )
    album_id = logical.get("musicbrainz_albumid")
    if album_id is not None:
        tags.add(
            TXXX(
                encoding=3,
                desc="MUSICBRAINZ_ALBUMID",
                text=[str(album_id)],
            )
        )
    original_artist = logical.get("original_artist")
    if original_artist is not None:
        tags.add(TOPE(encoding=3, text=[str(original_artist)]))
    original_album = logical.get("original_album")
    if original_album is not None:
        tags.add(TOAL(encoding=3, text=[str(original_album)]))
    original_title = logical.get("original_title")
    if original_title is not None:
        tags.add(
            COMM(
                encoding=3,
                lang="eng",
                desc="",
                text=[f"Original title: {original_title}"],
            )
        )
    source_url = logical.get("source_url")
    if source_url is not None:
        tags.add(
            TXXX(encoding=3, desc="SOURCEURL", text=[str(source_url)])
        )
    if cover is not None:
        tags.add(
            APIC(
                encoding=3,
                mime=cover.mime,
                type=3,
                desc="",
                data=cover.bytes,
            )
        )


def _mp4_freeform_key(name: str) -> str:
    return f"----:com.apple.iTunes:{name}"


def _set_mp4_free(tags: Any, name: str, value: str | None) -> None:
    if value is not None:
        tags[_mp4_freeform_key(name)] = [
            MP4FreeForm(str(value).encode("utf-8"))
        ]


def _mp4_cover(cover: CoverArt) -> MP4Cover:
    if cover.mime == "image/png":
        return MP4Cover(cover.bytes, imageformat=MP4Cover.FORMAT_PNG)
    if cover.mime == "image/jpeg":
        return MP4Cover(cover.bytes, imageformat=MP4Cover.FORMAT_JPEG)
    from PIL import Image

    with Image.open(BytesIO(cover.bytes)) as image:
        converted = image.convert("RGB")
        output = BytesIO()
        converted.save(output, format="JPEG")
    return MP4Cover(output.getvalue(), imageformat=MP4Cover.FORMAT_JPEG)


def _write_mp4(tags: Any, logical: dict[str, Any], cover: CoverArt | None) -> None:
    title = logical.get("title")
    if title is not None:
        tags["\xa9nam"] = [str(title)]
    album = logical.get("album")
    if album is not None:
        tags["\xa9alb"] = [str(album)]
    artist = logical.get("artist")
    if artist is not None:
        tags["\xa9ART"] = [str(artist)]
    album_artist = logical.get("album_artist")
    if album_artist is not None:
        tags["aART"] = [str(album_artist)]
    if logical.get("compilation"):
        tags["cpil"] = True
    track = _int_or_none(logical.get("track_number"))
    total = _int_or_none(logical.get("track_total"))
    if track is not None or total is not None:
        tags["trkn"] = [(track or 0, total or 0)]
    tags["disk"] = [(_disc_number(logical), 0)]
    date = logical.get("date")
    if date is not None and date != "":
        tags["\xa9day"] = [str(date)]
    genres = logical.get("genres") or []
    if genres:
        tags["\xa9gen"] = [str(genre) for genre in genres]
    _set_mp4_free(tags, "LABEL", logical.get("label"))
    _set_mp4_free(
        tags, "MusicBrainz Track Id", logical.get("musicbrainz_trackid")
    )
    _set_mp4_free(
        tags, "MusicBrainz Album Id", logical.get("musicbrainz_albumid")
    )
    _set_mp4_free(tags, "ORIGINAL ARTIST", logical.get("original_artist"))
    _set_mp4_free(tags, "ORIGINAL ALBUM", logical.get("original_album"))
    original_title = logical.get("original_title")
    if original_title is not None:
        tags["\xa9cmt"] = [f"Original title: {original_title}"]
    _set_mp4_free(tags, "SOURCEURL", logical.get("source_url"))
    if cover is not None:
        tags["covr"] = [_mp4_cover(cover)]


def _write_ape(tags: Any, logical: dict[str, Any], cover: CoverArt | None) -> None:
    def put(key: str, value: Any) -> None:
        if value is not None:
            tags[key] = value

    put("TITLE", logical.get("title"))
    put("ALBUM", logical.get("album"))
    put("ARTIST", logical.get("artist"))
    put("ALBUMARTIST", logical.get("album_artist"))
    if logical.get("compilation"):
        put("COMPILATION", "1")
    put("TRACKNUMBER", logical.get("track_number"))
    if logical.get("track_total") is not None:
        put("TRACKTOTAL", str(logical["track_total"]))
    put("DISCNUMBER", str(_disc_number(logical)))
    date = logical.get("date")
    if date is not None and date != "":
        put("DATE", str(date))
    put("LABEL", logical.get("label"))
    genres = logical.get("genres") or []
    if genres:
        put("GENRE", [str(genre) for genre in genres])
    put("MUSICBRAINZ_TRACKID", logical.get("musicbrainz_trackid"))
    put("MUSICBRAINZ_ALBUMID", logical.get("musicbrainz_albumid"))
    put("ORIGINALARTIST", logical.get("original_artist"))
    put("ORIGINALALBUMTITLE", logical.get("original_album"))
    original_title = logical.get("original_title")
    if original_title is not None:
        put("COMMENT", f"Original title: {original_title}")
    put("SOURCEURL", logical.get("source_url"))
    if cover is not None:
        tags["Cover Art (Front)"] = APEValue(
            b"cover.jpg\0" + cover.bytes, BINARY
        )


def write_tags(
    path: Path,
    matched: dict[str, Any],
    release: dict[str, Any],
    cover: CoverArt | None = None,
) -> None:
    logical = _logical_from_write(matched, release)
    ext = _ext(path)
    try:
        obj = _load(path)
        if isinstance(obj, (FLAC, OggVorbis, OggOpus)) or ext in _VORBIS_EXTENSIONS:
            _write_vorbis(obj, logical, cover)
        elif isinstance(obj, (MP3, WAVE)) or ext in _ID3_EXTENSIONS:
            tags = _clear_id3(obj)
            _write_id3(tags, logical, cover)
        elif isinstance(obj, MP4) or ext in _MP4_EXTENSIONS:
            tags = _clear_mp4(obj)
            _write_mp4(tags, logical, cover)
        elif isinstance(obj, (MonkeysAudio, WavPack)) or ext in _APE_EXTENSIONS:
            tags = _clear_ape(obj)
            _write_ape(tags, logical, cover)
        else:
            raise MimportError(
                f"probe/tag error at {path}: unsupported tag format"
            )
        obj.save()
    except MimportError:
        raise
    except Exception as exc:
        raise MimportError(f"probe/tag error at {path}: {exc}")


def read_source_url(path: Path) -> str | None:
    try:
        logical = _read_logical(path)
    except Exception:
        return None
    value = logical.get("source_url")
    return str(value) if value is not None else None


def _has_cover(obj: Any) -> bool:
    if isinstance(obj, FLAC):
        return bool(obj.pictures)
    if isinstance(obj, (OggVorbis, OggOpus)):
        tags = getattr(obj, "tags", None)
        return bool(_vorbis_values(tags, "METADATA_BLOCK_PICTURE")) or bool(
            _vorbis_values(tags, "COVERART")
        )
    if isinstance(obj, (MP3, WAVE)):
        tags = getattr(obj, "tags", None)
        return tags is not None and bool(tags.getall("APIC"))
    if isinstance(obj, MP4):
        tags = getattr(obj, "tags", None)
        return tags is not None and bool(tags.get("covr"))
    if isinstance(obj, (MonkeysAudio, WavPack)):
        tags = getattr(obj, "tags", None)
        if tags is None:
            return False
        return any(
            _norm_key(str(key)).startswith("COVERART") for key in tags.keys()
        )
    return False


def has_embedded_cover(path: Path) -> bool:
    try:
        return _has_cover(_load(path))
    except Exception:
        return False


def _remove_ape_cover(tags: Any) -> None:
    for key in list(tags.keys()):
        if _norm_key(str(key)).startswith("COVERART"):
            del tags[key]


def embed_cover(path: Path, cover: CoverArt) -> None:
    ext = _ext(path)
    try:
        obj = _load(path)
        if isinstance(obj, FLAC):
            obj.clear_pictures()
            obj.add_picture(_picture(cover))
        elif isinstance(obj, (OggVorbis, OggOpus)):
            tags = _ensure_tags(obj)
            for key in list(tags.keys()):
                if _norm_key(str(key)) in (
                    "METADATA_BLOCK_PICTURE",
                    "COVERART",
                ):
                    del tags[key]
            tags["METADATA_BLOCK_PICTURE"] = [_vorbis_picture_value(cover)]
        elif isinstance(obj, (MP3, WAVE)):
            tags = _ensure_tags(obj)
            tags.delall("APIC")
            tags.add(
                APIC(
                    encoding=3,
                    mime=cover.mime,
                    type=3,
                    desc="",
                    data=cover.bytes,
                )
            )
        elif isinstance(obj, MP4):
            tags = _ensure_tags(obj)
            tags["covr"] = [_mp4_cover(cover)]
        elif isinstance(obj, (MonkeysAudio, WavPack)):
            tags = _ensure_tags(obj)
            _remove_ape_cover(tags)
            tags["Cover Art (Front)"] = APEValue(
                b"cover.jpg\0" + cover.bytes, BINARY
            )
        else:
            raise MimportError(
                f"probe/tag error at {path}: cover embedding unsupported for format {ext or 'unknown'}"
            )
        obj.save()
    except MimportError:
        raise
    except Exception as exc:
        raise MimportError(f"probe/tag error at {path}: {exc}")


def _edits_empty(edits: dict[str, Any]) -> bool:
    return all(value is None for value in edits.values())


def _edits_genres(edits: dict[str, Any]) -> list[str] | None:
    if "genre" in edits:
        value = edits.get("genre")
    else:
        value = edits.get("genres")
    if value is None:
        return None
    return [str(genre) for genre in value]


def _vorbis_edit(tags: Any, edits: dict[str, Any]) -> None:
    title = edits.get("title")
    if title is not None:
        _vorbis_set(tags, "TITLE", str(title))
    artist = edits.get("artist")
    if artist is not None:
        _vorbis_set(tags, "ARTIST", str(artist))
        _vorbis_set(tags, "ALBUMARTIST", str(artist))
        _vorbis_set(tags, "ALBUM ARTIST", str(artist))
        _vorbis_set(tags, "ARTISTS", str(artist))
    album_artist = edits.get("album_artist")
    if album_artist is not None:
        _vorbis_set(tags, "ALBUMARTIST", str(album_artist))
        _vorbis_set(tags, "ALBUM ARTIST", str(album_artist))
    album = edits.get("album")
    if album is not None:
        _vorbis_set(tags, "ALBUM", str(album))
    year = edits.get("year")
    if year is not None:
        _vorbis_set(tags, "DATE", str(year))
    track = edits.get("track")
    if track is not None:
        _vorbis_set(tags, "TRACKNUMBER", str(max(0, int(track))))
    disc = edits.get("disc")
    if disc is not None:
        _vorbis_set(tags, "DISCNUMBER", str(max(0, int(disc))))
    compilation = edits.get("compilation")
    if compilation is True:
        _vorbis_set(tags, "COMPILATION", "1")
    elif compilation is False:
        for key in list(tags.keys()):
            if _norm_key(str(key)) == "COMPILATION":
                del tags[key]
    genres = _edits_genres(edits)
    if genres is not None:
        for key in list(tags.keys()):
            if _norm_key(str(key)) == "GENRE":
                del tags[key]
        _vorbis_set_list(tags, "GENRE", genres)


def _id3_put(tags: Any, frame_id: str, frame: Any) -> None:
    tags.delall(frame_id)
    tags.add(frame)


def _id3_put_txxx(tags: Any, desc: str, value: str) -> None:
    target = _norm_key(desc)
    for frame in list(tags.getall("TXXX")):
        if _norm_key(str(getattr(frame, "desc", ""))) == target:
            tags.delall(frame.HashKey)
    tags.add(TXXX(encoding=3, desc=desc, text=[value]))


def _id3_edit(tags: Any, edits: dict[str, Any]) -> None:
    title = edits.get("title")
    if title is not None:
        _id3_put(tags, "TIT2", TIT2(encoding=3, text=[str(title)]))
    artist = edits.get("artist")
    if artist is not None:
        _id3_put(tags, "TPE1", TPE1(encoding=3, text=[str(artist)]))
        _id3_put(tags, "TPE2", TPE2(encoding=3, text=[str(artist)]))
        _id3_put_txxx(tags, "ARTISTS", str(artist))
    album_artist = edits.get("album_artist")
    if album_artist is not None:
        _id3_put(tags, "TPE2", TPE2(encoding=3, text=[str(album_artist)]))
    album = edits.get("album")
    if album is not None:
        _id3_put(tags, "TALB", TALB(encoding=3, text=[str(album)]))
    year = edits.get("year")
    if year is not None:
        _id3_put(tags, "TDRC", TDRC(encoding=3, text=[str(year)]))
    track = edits.get("track")
    if track is not None:
        _, total = _id3_track_pair(tags)
        value = str(max(0, int(track)))
        if total is not None:
            value = f"{value}/{total}"
        _id3_put(tags, "TRCK", TRCK(encoding=3, text=[value]))
    disc = edits.get("disc")
    if disc is not None:
        _, total = _id3_disc_pair(tags)
        value = str(max(0, int(disc)))
        if total is not None:
            value = f"{value}/{total}"
        _id3_put(tags, "TPOS", TPOS(encoding=3, text=[value]))
    compilation = edits.get("compilation")
    if compilation is True:
        _id3_put(tags, "TCMP", TCMP(encoding=3, text=["1"]))
    elif compilation is False:
        tags.delall("TCMP")
    genres = _edits_genres(edits)
    if genres is not None:
        tags.delall("TCON")
        if genres:
            tags.add(TCON(encoding=3, text=genres))


def _mp4_edit(tags: Any, edits: dict[str, Any]) -> None:
    title = edits.get("title")
    if title is not None:
        tags["\xa9nam"] = [str(title)]
    artist = edits.get("artist")
    if artist is not None:
        tags["\xa9ART"] = [str(artist)]
        tags["aART"] = [str(artist)]
        _set_mp4_free(tags, "ARTISTS", str(artist))
    album_artist = edits.get("album_artist")
    if album_artist is not None:
        tags["aART"] = [str(album_artist)]
    album = edits.get("album")
    if album is not None:
        tags["\xa9alb"] = [str(album)]
    year = edits.get("year")
    if year is not None:
        tags["\xa9day"] = [str(year)]
    track = edits.get("track")
    if track is not None:
        values = tags.get("trkn")
        total = 0
        if values and isinstance(values[0], tuple) and len(values[0]) >= 2:
            total = int(values[0][1])
        tags["trkn"] = [(max(0, int(track)), total)]
    disc = edits.get("disc")
    if disc is not None:
        values = tags.get("disk")
        total = 0
        if values and isinstance(values[0], tuple) and len(values[0]) >= 2:
            total = int(values[0][1])
        tags["disk"] = [(max(0, int(disc)), total)]
    compilation = edits.get("compilation")
    if compilation is True:
        tags["cpil"] = True
    elif compilation is False:
        if "cpil" in tags:
            del tags["cpil"]
    genres = _edits_genres(edits)
    if genres is not None:
        if genres:
            tags["\xa9gen"] = genres
        else:
            if "\xa9gen" in tags:
                del tags["\xa9gen"]


def _ape_edit(tags: Any, edits: dict[str, Any]) -> None:
    def put(key: str, value: Any) -> None:
        tags[key] = value

    title = edits.get("title")
    if title is not None:
        put("TITLE", str(title))
    artist = edits.get("artist")
    if artist is not None:
        put("ARTIST", str(artist))
        put("ALBUMARTIST", str(artist))
        put("ARTISTS", str(artist))
    album_artist = edits.get("album_artist")
    if album_artist is not None:
        put("ALBUMARTIST", str(album_artist))
    album = edits.get("album")
    if album is not None:
        put("ALBUM", str(album))
    year = edits.get("year")
    if year is not None:
        put("DATE", str(year))
    track = edits.get("track")
    if track is not None:
        put("TRACKNUMBER", str(max(0, int(track))))
    disc = edits.get("disc")
    if disc is not None:
        put("DISCNUMBER", str(max(0, int(disc))))
    compilation = edits.get("compilation")
    if compilation is True:
        put("COMPILATION", "1")
    elif compilation is False:
        for key in list(tags.keys()):
            if _norm_key(str(key)) == "COMPILATION":
                del tags[key]
    genres = _edits_genres(edits)
    if genres is not None:
        for key in list(tags.keys()):
            if _norm_key(str(key)) == "GENRE":
                del tags[key]
        if genres:
            put("GENRE", genres)


def rewrite_tags(path: Path, edits: dict[str, Any]) -> None:
    if not edits or _edits_empty(edits):
        return
    ext = _ext(path)
    try:
        obj = _load(path)
        if isinstance(obj, (FLAC, OggVorbis, OggOpus)) or ext in _VORBIS_EXTENSIONS:
            tags = _ensure_tags(obj)
            _vorbis_edit(tags, edits)
        elif isinstance(obj, (MP3, WAVE)) or ext in _ID3_EXTENSIONS:
            tags = _ensure_tags(obj)
            _id3_edit(tags, edits)
        elif isinstance(obj, MP4) or ext in _MP4_EXTENSIONS:
            tags = _ensure_tags(obj)
            _mp4_edit(tags, edits)
        elif isinstance(obj, (MonkeysAudio, WavPack)) or ext in _APE_EXTENSIONS:
            tags = _ensure_tags(obj)
            _ape_edit(tags, edits)
        else:
            raise MimportError(
                f"probe/tag error at {path}: unsupported tag format"
            )
        obj.save()
    except MimportError:
        raise
    except Exception as exc:
        raise MimportError(f"probe/tag error at {path}: {exc}")
