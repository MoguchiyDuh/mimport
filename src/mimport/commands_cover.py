import argparse
import logging
from collections import defaultdict
from contextlib import closing
from pathlib import Path
from typing import Any

from mimport import jobs, library, mb, metadata
from mimport.config import Config
from mimport.coverart import CoverArt, CoverArtClient
from mimport.errors import MimportError
from mimport.files import FileBatch


def _embed(cfg: Config, tracks: list[dict[str, Any]], cover: CoverArt) -> tuple[int, list[dict[str, str]]]:
    embedded = 0
    failed: list[dict[str, str]] = []
    for track in tracks:
        path = Path(track["path"])
        try:
            with FileBatch(cfg.paths.library, cfg.paths.database) as batch:
                batch.add(path, path, path, lambda temporary: metadata.embed_cover(temporary, cover))
                batch.publish()
                batch.finish()
            embedded += 1
        except (MimportError, OSError) as exc:
            logging.warning("embed failed for %s: %s", path, exc)
            failed.append({"path": str(path), "error": str(exc)})
    return embedded, failed


def _without_mbid(
    cfg: Config, client: CoverArtClient, tracks: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    if not client.itunes_fallback_enabled():
        return [{"release": None, "tracks": len(tracks), "embedded": 0, "reason": "no release mbid"}]
    groups: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for track in tracks:
        groups[(track["artist"], track["album"])].append(track)
    results: list[dict[str, Any]] = []
    for (artist, album), group in sorted(groups.items()):
        result: dict[str, Any] = {"release": None, "album": album, "tracks": len(group), "embedded": 0}
        try:
            cover = client.fetch_itunes(artist, album)
        except MimportError as exc:
            result.update(reason="fetch error", failed=[{"error": str(exc)}])
        else:
            if cover is None:
                result["reason"] = "no cover in itunes"
            else:
                embedded, failed = _embed(cfg, group, cover)
                result.update(source="itunes", embedded=embedded, failed=failed)
        results.append(result)
    return results


def run(cfg: Config, args: argparse.Namespace) -> list[dict[str, Any]]:
    with closing(jobs.open(cfg.paths.database)) as connection:
        tracks = library.list_tracks(connection, args.query)
    groups: dict[str | None, list[dict[str, Any]]] = defaultdict(list)
    for track in tracks:
        groups[track.get("release_mbid")].append(track)
    results: list[dict[str, Any]] = []
    if not args.fetch:
        for mbid, group in sorted(groups.items(), key=lambda item: (item[0] is not None, item[0] or "")):
            covered = sum(metadata.has_embedded_cover(Path(track["path"])) for track in group)
            results.append({"release": mbid, "tracks": len(group), "covered": covered, "missing": len(group) - covered})
        return results
    with CoverArtClient(cfg.cover_art, cfg.musicbrainz.user_agent) as client, mb.MbClient(cfg.musicbrainz) as mb_client:
        for mbid, group in sorted(groups.items(), key=lambda item: (item[0] is not None, item[0] or "")):
            if mbid is None:
                results.extend(_without_mbid(cfg, client, group))
                continue
            fetch_error: str | None = None
            source = "caa"
            cover: CoverArt | None = None
            try:
                cover = client.front_cover(mbid)
            except MimportError as exc:
                fetch_error = str(exc)
            else:
                if cover is None:
                    try:
                        raw = mb.release_with_tracks(mb_client, mbid)
                    except MimportError:
                        raw = {}
                    release_group = raw.get("release-group")
                    if release_group is not None:
                        try:
                            cover = client.front_cover_release_group(release_group["id"])
                        except MimportError as exc:
                            fetch_error = str(exc)
            if cover is None and fetch_error is None and client.itunes_fallback_enabled():
                source = "itunes"
                try:
                    cover = client.fetch_itunes(group[0]["artist"], group[0]["album"])
                except MimportError as exc:
                    fetch_error = str(exc)
            result: dict[str, Any] = {"release": mbid, "tracks": len(group), "embedded": 0}
            if fetch_error is not None:
                result.update(reason="fetch error", failed=[{"error": fetch_error}])
            elif cover is None:
                result["reason"] = "no cover in archive or itunes"
            else:
                embedded, failed = _embed(cfg, group, cover)
                result.update(source=source, embedded=embedded, failed=failed)
            results.append(result)
    return results
