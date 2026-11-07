import argparse
import logging
from typing import Any

from mimport import lidarr, mb, release, scorer
from mimport.config import Config
from mimport.errors import MimportError


def run(cfg: Config, args: argparse.Namespace) -> Any:
    if args.command == "lidarr":
        with lidarr.LidarrClient(cfg.lidarr) as client:
            if args.action == "artist":
                if args.query.startswith("mbid:"):
                    return lidarr.lookup_artist(client, args.query[5:])
                return lidarr.search_artists(client, args.query)
            album = lidarr.lookup_album(
                client, args.release_group_mbid if args.action == "tracks" else args.mbid
            )
            if args.action == "album":
                return scorer.rank_releases(
                    [release.from_lidarr(item) for item in album.get("Releases", [])],
                    release.group_from_lidarr(album),
                    cfg.scoring,
                )
            selected = next(
                (item for item in album.get("Releases", []) if item["Id"] == args.release_mbid),
                None,
            )
            if selected is None:
                raise MimportError(f"lidarr proxy has no release with mbid {args.release_mbid}")
            return release.from_lidarr(selected)
    logging.warning(
        "mb is rate-limited (~1 req/s, multi-round-trip); prefer the lidarr commands unless you specifically need direct MusicBrainz data"
    )
    with mb.MbClient(cfg.musicbrainz) as client:
        if args.action == "artist":
            if args.query.startswith("mbid:"):
                return mb.lookup_artist(client, args.query[5:])
            return mb.search_artists(client, args.query)
        if args.action == "track":
            return mb.search_recordings(client, args.text)
        if args.action == "tracks":
            return release.from_mb(mb.release_with_tracks(client, args.release_mbid))
        editions = mb.release_group_releases(client, args.mbid)
        group = mb.lookup_release_group(client, args.mbid)
        return scorer.rank_releases(
            [release.from_mb(item) for item in editions],
            release.group_from_mb(group),
            cfg.scoring,
        )
