from __future__ import annotations

import time
import warnings
from dataclasses import dataclass
from io import BytesIO
from pathlib import Path
from typing import Any

import httpx
from PIL import Image

from .cache import cache_dir_default
from .errors import MimportError, io_error

MAX_COVER_EDGE = 500
ITUNES_BASE_URL = "https://itunes.apple.com"


@dataclass
class CoverArt:
    mime: str
    bytes: bytes


def _sniff_mime(data: bytes) -> str | None:
    if data.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    if data.startswith(b"\x89PNG"):
        return "image/png"
    if data.startswith(b"GIF8"):
        return "image/gif"
    return None


def _finish_cover(image: Image.Image) -> CoverArt:
    if max(image.size) > MAX_COVER_EDGE:
        image.thumbnail(
            (MAX_COVER_EDGE, MAX_COVER_EDGE),
            Image.Resampling.LANCZOS,
            reducing_gap=None,
        )
    if image.mode not in ("RGB", "L"):
        image = image.convert("RGB")
    output = BytesIO()
    image.save(output, format="JPEG", quality=75)
    return CoverArt("image/jpeg", output.getvalue())


def from_local_file(path: Path) -> CoverArt:
    try:
        data = path.read_bytes()
    except OSError as exc:
        raise io_error(path, exc)
    try:
        with Image.open(BytesIO(data)) as image:
            image.load()
            return _finish_cover(image)
    except Exception as exc:
        raise MimportError(
            f"failed to preprocess cover image at {path}: {exc}"
        )


def _norm_title(value: str) -> str:
    return "".join(ch.lower() for ch in value if ch.isalnum())


def _itunes_album_term(album: str) -> str:
    head = album.split(" / ", 1)[0].strip()
    if head.endswith(" - EP"):
        head = head[: -len(" - EP")]
    elif head.endswith(" - Single"):
        head = head[: -len(" - Single")]
    return head.strip()


def _itunes_artist_term(artist: str) -> str:
    return artist.replace(" x ", " & ").replace(" × ", " & ")


class CoverArtClient:
    def __init__(self, cfg: Any, user_agent: str) -> None:
        self._client = httpx.Client(timeout=30.0, follow_redirects=True)
        self.base_url = str(cfg.base_url).rstrip("/")
        self.user_agent = user_agent
        cache_dir = cfg.cache_dir
        self.cache_dir = (
            Path(cache_dir) if cache_dir is not None else cache_dir_default("coverart")
        )
        self.positive_ttl = int(cfg.cache_ttl_secs)
        self.negative_ttl = int(cfg.negative_ttl_secs)
        self.itunes_fallback = bool(cfg.itunes_fallback)
        self.itunes_country = str(cfg.itunes_country)

    def close(self) -> None:
        self._client.close()

    def __enter__(self) -> "CoverArtClient":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def _cache_path(self, key: str) -> Path:
        return self.cache_dir / f"{key}.front-500"

    def _read_cache(self, key: str) -> tuple[str, CoverArt | None]:
        path = self._cache_path(key)
        try:
            age = max(0.0, time.time() - path.stat().st_mtime)
        except OSError:
            return ("miss", None)
        try:
            data = path.read_bytes()
        except OSError:
            return ("miss", None)
        if not data:
            if age <= self.negative_ttl:
                return ("negative", None)
            return ("miss", None)
        if age > self.positive_ttl:
            return ("miss", None)
        mime = _sniff_mime(data)
        if mime is None:
            try:
                path.unlink()
            except OSError:
                pass
            return ("miss", None)
        return ("positive", CoverArt(mime, data))

    def _write_cache(self, key: str, data: bytes) -> None:
        try:
            self.cache_dir.mkdir(parents=True, exist_ok=True)
            self._cache_path(key).write_bytes(data)
        except OSError:
            return

    def _write_negative(self, key: str) -> None:
        self._write_cache(key, b"")

    def front_cover(self, release_mbid: str) -> CoverArt | None:
        cover = self._fetch_caa(f"release/{release_mbid}", release_mbid)
        if cover is None:
            self._write_negative(release_mbid)
        return cover

    def front_cover_with_fallback(
        self, release_mbid: str, artist: str, album: str
    ) -> CoverArt | None:
        if release_mbid:
            key = f"{release_mbid}.fb"
            state, cached = self._read_cache(key)
            if state == "positive":
                return cached
            if state == "negative":
                return None
            try:
                cover = self._fetch_caa(f"release/{release_mbid}", release_mbid)
            except MimportError as exc:
                warnings.warn(
                    f"CAA fetch failed for {release_mbid}: {exc}; trying iTunes",
                    RuntimeWarning,
                )
            else:
                if cover is not None:
                    return cover
        key = f"{release_mbid}.fb"
        cover = self.fetch_itunes(artist, album)
        if cover is not None:
            self._write_cache(key, cover.bytes)
            return cover
        self._write_negative(key)
        return None

    def itunes_fallback_enabled(self) -> bool:
        return self.itunes_fallback

    def fetch_itunes(self, artist: str, album: str) -> CoverArt | None:
        if not artist or not album:
            return None
        term = f"{_itunes_artist_term(artist)} {_itunes_album_term(album)}"
        search_url = f"{ITUNES_BASE_URL}/search"
        error_url = f"{ITUNES_BASE_URL}/search?term={artist}+{album}"
        try:
            response = self._client.get(
                search_url,
                headers={"User-Agent": self.user_agent},
                params={
                    "term": term,
                    "entity": "album",
                    "country": self.itunes_country,
                    "limit": "5",
                },
            )
        except httpx.HTTPError as exc:
            raise MimportError(f"cover art fetch failed for {error_url}: {exc}")
        try:
            search = response.json()
        except Exception as exc:
            raise MimportError(f"cover art fetch failed for {error_url}: {exc}")

        results = search.get("results") if isinstance(search, dict) else None
        if not isinstance(results, list):
            results = []
        album_norm = _norm_title(album)
        artist_norm = _norm_title(_itunes_artist_term(artist))
        art_url: str | None = None
        for result in results:
            if not isinstance(result, dict):
                continue
            name = result.get("collectionName")
            if not isinstance(name, str):
                name = ""
            normalized = _norm_title(name)
            if (
                album_norm
                and normalized
                and (album_norm in normalized or normalized in album_norm)
            ):
                value = result.get("artworkUrl100")
                if isinstance(value, str):
                    art_url = value
                break
        if art_url is None and results:
            result = results[0]
            if isinstance(result, dict):
                artist_name = result.get("artistName")
                if not isinstance(artist_name, str):
                    artist_name = ""
                normalized = _norm_title(artist_name)
                if (
                    not artist_norm
                    or not normalized
                    or artist_norm in normalized
                    or normalized in artist_norm
                ):
                    value = result.get("artworkUrl100")
                    if isinstance(value, str):
                        art_url = value
        if art_url is None:
            return None
        big = art_url.replace("100x100bb", "1000x1000bb")
        try:
            response = self._client.get(
                big, headers={"User-Agent": self.user_agent}
            )
        except httpx.HTTPError as exc:
            raise MimportError(f"cover art fetch failed for {big}: {exc}")
        if not 200 <= response.status_code < 300:
            warnings.warn(
                f"itunes artwork fetch failed ({big}): HTTP {response.status_code}",
                RuntimeWarning,
            )
            return None
        data = response.content
        try:
            image = Image.open(BytesIO(data))
            image.load()
        except Exception as exc:
            warnings.warn(
                f"itunes artwork not an image ({big}): {exc}", RuntimeWarning
            )
            return None
        try:
            return _finish_cover(image)
        except Exception as exc:
            raise MimportError(
                f"failed to preprocess cover image at <memory>: {exc}"
            )

    def front_cover_release_group(
        self, release_group_mbid: str
    ) -> CoverArt | None:
        key = f"rg-{release_group_mbid}"
        cover = self._fetch_caa(
            f"release-group/{release_group_mbid}", key
        )
        if cover is None:
            self._write_negative(key)
        return cover

    def _fetch_caa(self, path: str, cache_key: str) -> CoverArt | None:
        state, cached = self._read_cache(cache_key)
        if state == "positive":
            return cached
        if state == "negative":
            return None
        url = f"{self.base_url}/{path}/front-500"
        try:
            response = self._client.get(
                url, headers={"User-Agent": self.user_agent}
            )
        except httpx.HTTPError as exc:
            raise MimportError(f"cover art fetch failed for {url}: {exc}")
        if response.status_code == 404:
            return None
        if not 200 <= response.status_code < 300:
            status = f"{response.status_code} {response.reason_phrase}"
            raise MimportError(
                f"cover art fetch failed for {url}: HTTP {status} {response.text}"
            )
        data = response.content
        mime = _sniff_mime(data)
        if mime is None:
            warnings.warn(
                f"cover art fetch for {cache_key}: non-image body ({len(data)} bytes), not caching",
                RuntimeWarning,
            )
            return None
        self._write_cache(cache_key, data)
        return CoverArt(mime, data)


def has_embedded_cover(path: Path) -> bool:
    from .metadata import has_embedded_cover as _has_embedded_cover

    return _has_embedded_cover(path)


def embed_cover(path: Path, cover: CoverArt) -> None:
    from .metadata import embed_cover as _embed_cover

    _embed_cover(path, cover)
