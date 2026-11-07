import argparse
import logging
from dataclasses import replace
from pathlib import Path
from typing import Any

from mimport import mb, release, tags, yt
from mimport.config import Config
from mimport.coverart import CoverArt, from_local_file
from mimport.errors import MimportError
from mimport.importer import publish_import, validate_native


def _genres(overrides: dict[str, Any]) -> list[str]:
    candidates = ([overrides["genre"]] if overrides.get("genre") is not None else []) + overrides.get("genres", [])
    return list(dict.fromkeys(value.strip() for value in candidates if value.strip()))


def _synthetic(overrides: dict[str, Any]) -> dict[str, Any]:
    return {
        "id": "",
        "title": overrides.get("album") or "Unknown Album",
        "status": None,
        "country": None,
        "disambiguation": None,
        "label": overrides.get("label"),
        "formats": ["Opus"],
        "track_count": 0,
        "date": overrides.get("date"),
        "artist_credit": overrides.get("artist"),
        "artist_credit_parts": [],
        "release_group_id": None,
        "title_native": None,
        "artist_credit_native": None,
        "album_artist": overrides.get("album_artist"),
        "compilation": overrides.get("compilation") or False,
        "genres": _genres(overrides),
        "tracks": [],
    }


def _matched(fetched: dict[str, Any], track: dict[str, Any]) -> dict[str, Any]:
    return {
        "file": str(fetched["path"]),
        "position": track.get("position"),
        "medium_position": track.get("medium_position"),
        "title": track["title"],
        "title_native": track.get("title_native"),
        "source_url": fetched["source_url"],
        "recording_id": track.get("recording_id"),
        "raw_position": track.get("raw_position"),
        "distance": 0.0,
        "reasons": {},
    }


def _thumbnail(fetched: list[dict[str, Any]], dry_run: bool) -> CoverArt | None:
    if dry_run:
        return None
    thumbnail = next((item["thumbnail"] for item in fetched if item.get("thumbnail") is not None), None)
    return from_local_file(Path(thumbnail)) if thumbnail is not None else None


def _playlist(cfg: Config, args: argparse.Namespace, yt_cfg: Any) -> dict[str, Any]:
    if args.release is None:
        raise MimportError("`yt fetch --playlist` requires `--release <mbid>` to map playlist entries to tracks")
    overrides = tags.apply_flag_overrides(
        tags.load(args.tags),
        album_artist=args.album_artist,
        compilation=True if args.compilation else None,
        genres=args.genre,
    )
    with mb.MbClient(cfg.musicbrainz) as client:
        normalized = release.from_mb(mb.release_with_tracks(client, args.release))
        validate_native(client, normalized, overrides, args.allow_native)
    fetched = yt.fetch(yt_cfg, args.url, cfg.paths.staging, playlist=True, dry_run=args.dry_run)
    if not fetched:
        raise MimportError("`yt fetch` fetched no tracks")
    cover = _thumbnail(fetched, args.dry_run)
    multi_disc = len({track["medium_position"] if track.get("medium_position") is not None else 1 for track in normalized["tracks"]}) > 1
    ordered = sorted(
        normalized["tracks"],
        key=lambda item: (
            item["medium_position"] if item.get("medium_position") is not None else 1,
            item["position"] if item.get("position") is not None else 4_294_967_295,
        ),
    )
    claimed: set[int] = set()
    skipped: list[int] = []
    matched: list[dict[str, Any]] = []
    for index, item in enumerate(fetched, 1):
        position = item.get("playlist_index")
        if position is None:
            logging.warning("playlist entry %s has no playlist_index; assuming position %s", item["video_id"], index)
            position = index
        if position in claimed:
            logging.warning("playlist entry %s (%s) duplicates an earlier entry; skipping", position, item["video_id"])
            skipped.append(position)
            continue
        claimed.add(position)
        if multi_disc:
            track = ordered[position - 1] if 0 < position <= len(ordered) else None
        else:
            track = next((track for track in normalized["tracks"] if track.get("position") == position), None)
        if track is None:
            logging.warning("playlist entry %s (%s) has no matching track in release %s; skipping", position, item["video_id"], args.release)
            skipped.append(position)
            continue
        matched.append(_matched(item, track))
    imported, _ = publish_import(cfg, matched, normalized, cover, dry_run=args.dry_run)
    return {
        "source_url": args.url,
        "release": normalized["id"],
        "cover_art": cover is not None,
        "matched": matched,
        "skipped_positions": skipped,
        "imported": imported,
    }


def run(cfg: Config, args: argparse.Namespace) -> dict[str, Any]:
    yt_cfg = cfg.yt
    if args.cookies is not None or args.cookies_from_browser is not None:
        yt_cfg = replace(yt_cfg, cookies=args.cookies, cookies_from_browser=args.cookies_from_browser)
    if args.playlist:
        return _playlist(cfg, args, yt_cfg)
    overrides = tags.apply_flag_overrides(
        tags.load(args.tags),
        artist=args.artist,
        album=args.album,
        date=args.year,
        album_artist=args.album_artist,
        compilation=True if args.compilation else None,
        genres=args.genre,
    )
    backfill: dict[str, Any] | None = None
    if args.release is not None:
        if args.track is None:
            raise MimportError("`yt fetch --release` requires `--track` to pick which release track to backfill from")
        with mb.MbClient(cfg.musicbrainz) as client:
            normalized = release.from_mb(mb.release_with_tracks(client, args.release))
            validate_native(client, normalized, overrides, args.allow_native)
        candidates = [
            track for track in normalized["tracks"]
            if track.get("position") == args.track and (
                args.disc is None
                or (track["medium_position"] if track.get("medium_position") is not None else 1) == args.disc
            )
        ]
        if len(candidates) != 1:
            raise MimportError(f"release {args.release} has no track at position {args.track}")
        backfill = candidates[0]
    else:
        normalized = _synthetic(overrides)
    final_title = args.title if args.title is not None else backfill["title"] if backfill is not None else None
    if final_title is None:
        raise MimportError("`yt fetch` needs a title: pass --title, or --release/--track to backfill one")
    final_track = {
        "title": final_title,
        "position": args.track if args.track is not None else backfill.get("position") if backfill is not None else None,
        "medium_position": args.disc if args.disc is not None else backfill.get("medium_position") if backfill is not None else None,
        "recording_id": backfill.get("recording_id") if backfill is not None else None,
        "raw_position": backfill.get("raw_position") if backfill is not None else None,
        "title_native": backfill.get("title_native") if backfill is not None and args.title is None else None,
    }
    fetched = yt.fetch(yt_cfg, args.url, cfg.paths.staging, playlist=False, dry_run=args.dry_run)
    if not fetched:
        raise MimportError("`yt fetch` fetched no tracks")
    item = fetched[0]
    cover = _thumbnail([item], args.dry_run)
    matched = [_matched(item, final_track)]
    imported, _ = publish_import(cfg, matched, normalized, cover, dry_run=args.dry_run)
    return {
        "source_url": item["source_url"],
        "video_id": item["video_id"],
        "yt_title": item.get("title"),
        "yt_uploader": item.get("uploader"),
        "release": normalized["id"] or None,
        "cover_art": cover is not None,
        "imported": imported,
    }
