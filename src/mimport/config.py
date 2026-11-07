from __future__ import annotations

import json
import math
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from .errors import MimportError, io_error

__all__ = (
    "Config",
    "PathsConfig",
    "MbConfig",
    "LidarrConfig",
    "SlskdConfig",
    "QualityConfig",
    "CoverArtConfig",
    "YtConfig",
    "load",
)

_U64_MAX = (1 << 64) - 1
_U32_MAX = (1 << 32) - 1
_U16_MAX = (1 << 16) - 1
_MISSING = object()
_UNSET_SENTINEL = "CHANGE_ME"


@dataclass
class PathsConfig:
    library: Path
    downloads: Path
    staging: Path
    database: Path


@dataclass
class MbConfig:
    base_url: str
    user_agent: str
    rate_limit_per_sec: float
    cache_dir: Path
    cache_ttl_secs: int
    max_retries: int


@dataclass
class LidarrConfig:
    base_url: str
    cache_dir: Path
    cache_ttl_secs: int
    max_retries: int


@dataclass
class SlskdConfig:
    base_url: str
    username: str
    password: str
    fetch_timeout_base_secs: int
    fetch_timeout_per_mb_secs: float
    fetch_timeout_per_file_secs: int
    request_timeout_secs: int


@dataclass
class QualityConfig:
    codec: str
    target_samplerate: int
    target_bitdepth: int
    opus_bitrate_kbps: int


@dataclass
class CoverArtConfig:
    base_url: str
    cache_dir: Path | None
    cache_ttl_secs: int
    negative_ttl_secs: int
    itunes_fallback: bool
    itunes_country: str


@dataclass
class YtConfig:
    yt_dlp_path: str
    cookies: Path | None
    cookies_from_browser: str | None


@dataclass
class Config:
    paths: PathsConfig
    musicbrainz: MbConfig
    lidarr: LidarrConfig
    slskd: SlskdConfig
    quality: QualityConfig
    scoring: dict[str, Any]
    cover_art: CoverArtConfig
    yt: YtConfig

    @classmethod
    def load(cls, path: str | Path) -> Config:
        return _load(path)


def _config_error(detail: str) -> MimportError:
    return MimportError(f"config error: {detail}")


def _missing(path: str) -> MimportError:
    return _config_error(f"missing field `{path}`")


def _wrong_type(path: str, expected: str) -> MimportError:
    return _config_error(f"invalid type for `{path}`: expected {expected}")


def _invalid_range(path: str, minimum: int, maximum: int) -> MimportError:
    return _config_error(f"invalid value for `{path}`: expected integer in {minimum}..={maximum}")


def _invalid_choice(path: str, choices: tuple[str, ...]) -> MimportError:
    rendered = ", ".join(json.dumps(choice) for choice in choices)
    return _config_error(f"invalid value for `{path}`: expected one of {rendered}")


def _positive_float(path: str, value: float) -> float:
    if value <= 0.0:
        raise _config_error(f"invalid value for `{path}`: expected value greater than 0")
    return value


def _nonnegative_float(path: str, value: float) -> float:
    if value < 0.0:
        raise _config_error(
            f"invalid value for `{path}`: expected value greater than or equal to 0"
        )
    return value


def _validate_base_url(path: str, value: str) -> None:
    if any(char.isspace() or ord(char) < 0x20 or ord(char) == 0x7F for char in value):
        raise _config_error(f"invalid value for `{path}`: expected an absolute http(s) URL")
    try:
        parsed = urlsplit(value)
        hostname = parsed.hostname
        _ = parsed.port
    except ValueError:
        raise _config_error(
            f"invalid value for `{path}`: expected an absolute http(s) URL"
        ) from None
    if (
        parsed.scheme.lower() not in ("http", "https")
        or not parsed.netloc
        or not hostname
        or parsed.netloc.endswith(":")
        or "\\" in value
    ):
        raise _config_error(f"invalid value for `{path}`: expected an absolute http(s) URL")


def _section(root: dict[str, Any], name: str, *, required: bool) -> dict[str, Any]:
    value = root.get(name, _MISSING)
    if value is _MISSING:
        if required:
            raise _missing(name)
        return {}
    if not isinstance(value, dict):
        raise _wrong_type(name, "table")
    return value


def _subtable(raw: dict[str, Any], key: str, path: str) -> dict[str, Any] | None:
    value = raw.get(key, _MISSING)
    if value is _MISSING:
        return None
    if not isinstance(value, dict):
        raise _wrong_type(path, "table")
    return value


def _field(raw: dict[str, Any], key: str, path: str, default: Any = _MISSING) -> Any:
    if key in raw:
        return raw[key]
    if default is _MISSING:
        raise _missing(path)
    return default


def _str_field(raw: dict[str, Any], key: str, path: str, default: Any = _MISSING) -> str:
    value = _field(raw, key, path, default)
    if not isinstance(value, str):
        raise _wrong_type(path, "string")
    return value


def _opt_str_field(raw: dict[str, Any], key: str, path: str) -> str | None:
    value = _field(raw, key, path, None)
    if value is None:
        return None
    if not isinstance(value, str):
        raise _wrong_type(path, "string")
    return value


def _path_field(raw: dict[str, Any], key: str, path: str, default: Any = _MISSING) -> Path:
    return Path(_str_field(raw, key, path, default))


def _opt_path_field(raw: dict[str, Any], key: str, path: str) -> Path | None:
    value = _opt_str_field(raw, key, path)
    if value is None:
        return None
    return Path(value)


def _int_field(
    raw: dict[str, Any],
    key: str,
    path: str,
    default: Any = _MISSING,
    *,
    minimum: int = 0,
    maximum: int,
) -> int:
    value = _field(raw, key, path, default)
    if isinstance(value, bool) or not isinstance(value, int):
        raise _wrong_type(path, "integer")
    if value < minimum or value > maximum:
        raise _invalid_range(path, minimum, maximum)
    return value


def _float_field(raw: dict[str, Any], key: str, path: str, default: Any = _MISSING) -> float:
    value = _field(raw, key, path, default)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise _wrong_type(path, "float")
    number = float(value)
    if not math.isfinite(number):
        raise _config_error(f"invalid value for `{path}`: expected a finite number")
    return number


def _bool_field(raw: dict[str, Any], key: str, path: str, default: Any = _MISSING) -> bool:
    value = _field(raw, key, path, default)
    if not isinstance(value, bool):
        raise _wrong_type(path, "boolean")
    return value


def _str_list_field(
    raw: dict[str, Any], key: str, path: str, default: Any = _MISSING
) -> list[str]:
    value = _field(raw, key, path, default)
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        raise _wrong_type(path, "array of strings")
    return list(value)


def _paths(raw: dict[str, Any]) -> PathsConfig:
    return PathsConfig(
        library=_path_field(raw, "library", "paths.library"),
        downloads=_path_field(raw, "downloads", "paths.downloads"),
        staging=_path_field(raw, "staging", "paths.staging"),
        database=_path_field(raw, "database", "paths.database"),
    )


def _musicbrainz(raw: dict[str, Any]) -> MbConfig:
    return MbConfig(
        base_url=_str_field(
            raw, "base_url", "musicbrainz.base_url", "https://musicbrainz.org/ws/2"
        ),
        user_agent=_str_field(raw, "user_agent", "musicbrainz.user_agent"),
        rate_limit_per_sec=_positive_float(
            "musicbrainz.rate_limit_per_sec",
            _float_field(raw, "rate_limit_per_sec", "musicbrainz.rate_limit_per_sec", 1.0),
        ),
        cache_dir=_path_field(raw, "cache_dir", "musicbrainz.cache_dir"),
        cache_ttl_secs=_int_field(
            raw,
            "cache_ttl_secs",
            "musicbrainz.cache_ttl_secs",
            2_592_000,
            maximum=_U64_MAX,
        ),
        max_retries=_int_field(
            raw, "max_retries", "musicbrainz.max_retries", 5, maximum=_U32_MAX
        ),
    )


def _lidarr(raw: dict[str, Any]) -> LidarrConfig:
    return LidarrConfig(
        base_url=_str_field(
            raw, "base_url", "lidarr.base_url", "https://api.lidarr.audio/api/v0.4"
        ),
        cache_dir=_path_field(raw, "cache_dir", "lidarr.cache_dir"),
        cache_ttl_secs=_int_field(
            raw, "cache_ttl_secs", "lidarr.cache_ttl_secs", 2_592_000, maximum=_U64_MAX
        ),
        max_retries=_int_field(raw, "max_retries", "lidarr.max_retries", 5, maximum=_U32_MAX),
    )


def _slskd(raw: dict[str, Any]) -> SlskdConfig:
    return SlskdConfig(
        base_url=_str_field(raw, "base_url", "slskd.base_url", "http://127.0.0.1:8111"),
        username=_str_field(raw, "username", "slskd.username"),
        password=_str_field(raw, "password", "slskd.password"),
        fetch_timeout_base_secs=_int_field(
            raw,
            "fetch_timeout_base_secs",
            "slskd.fetch_timeout_base_secs",
            30,
            maximum=_U64_MAX,
        ),
        fetch_timeout_per_mb_secs=_nonnegative_float(
            "slskd.fetch_timeout_per_mb_secs",
            _float_field(
                raw, "fetch_timeout_per_mb_secs", "slskd.fetch_timeout_per_mb_secs", 1.0
            ),
        ),
        fetch_timeout_per_file_secs=_int_field(
            raw,
            "fetch_timeout_per_file_secs",
            "slskd.fetch_timeout_per_file_secs",
            30,
            maximum=_U64_MAX,
        ),
        request_timeout_secs=_int_field(
            raw,
            "request_timeout_secs",
            "slskd.request_timeout_secs",
            30,
            minimum=1,
            maximum=_U64_MAX,
        ),
    )


def _quality(raw: dict[str, Any]) -> QualityConfig:
    codec = _str_field(raw, "codec", "quality.codec", "flac")
    if codec not in ("flac", "opus"):
        raise _invalid_choice("quality.codec", ("flac", "opus"))
    target_bitdepth = _int_field(
        raw, "target_bitdepth", "quality.target_bitdepth", 16, maximum=_U16_MAX
    )
    if target_bitdepth not in (16, 24):
        raise _config_error("invalid value for `quality.target_bitdepth`: expected 16 or 24")
    return QualityConfig(
        codec=codec,
        target_samplerate=_int_field(
            raw,
            "target_samplerate",
            "quality.target_samplerate",
            44_100,
            minimum=1,
            maximum=_U32_MAX,
        ),
        target_bitdepth=target_bitdepth,
        opus_bitrate_kbps=_int_field(
            raw,
            "opus_bitrate_kbps",
            "quality.opus_bitrate_kbps",
            160,
            minimum=1,
            maximum=_U32_MAX,
        ),
    )


def _cover_art(raw: dict[str, Any]) -> CoverArtConfig:
    return CoverArtConfig(
        base_url=_str_field(
            raw, "base_url", "cover_art.base_url", "https://coverartarchive.org"
        ),
        cache_dir=_opt_path_field(raw, "cache_dir", "cover_art.cache_dir"),
        cache_ttl_secs=_int_field(
            raw, "cache_ttl_secs", "cover_art.cache_ttl_secs", 2_592_000, maximum=_U64_MAX
        ),
        negative_ttl_secs=_int_field(
            raw, "negative_ttl_secs", "cover_art.negative_ttl_secs", 604_800, maximum=_U64_MAX
        ),
        itunes_fallback=_bool_field(raw, "itunes_fallback", "cover_art.itunes_fallback", True),
        itunes_country=_str_field(raw, "itunes_country", "cover_art.itunes_country", "JP"),
    )


def _yt(raw: dict[str, Any]) -> YtConfig:
    return YtConfig(
        yt_dlp_path=_str_field(raw, "yt_dlp_path", "yt.yt_dlp_path", "yt-dlp"),
        cookies=_opt_path_field(raw, "cookies", "yt.cookies"),
        cookies_from_browser=_opt_str_field(raw, "cookies_from_browser", "yt.cookies_from_browser"),
    )


def _default_penalised_secondary_types() -> list[str]:
    return ["compilation", "live", "remix", "demo", "interview", "spokenword"]


def _default_strong_terms() -> list[str]:
    return ["live", "remix", "karaoke", "commentary", "interview", "rehearsal"]


def _default_medium_terms() -> list[str]:
    return ["demo", "instrumental", "acoustic", "alternate take", "alt take", "session"]


def _default_weak_terms() -> list[str]:
    return ["radio edit", "edit", "version"]


def _status_weights(raw: dict[str, Any] | None) -> dict[str, Any]:
    if raw is None:
        return {"official": 100.0, "other": -120.0}
    return {
        "official": _float_field(raw, "official", "scoring.status.official"),
        "other": _float_field(raw, "other", "scoring.status.other"),
    }


def _media_weights(raw: dict[str, Any] | None) -> dict[str, Any]:
    if raw is None:
        return {
            "digital": 70.0,
            "cd": -20.0,
            "other_non_digital": -20.0,
            "vinyl": -100.0,
            "other_physical": -95.0,
        }
    return {
        "digital": _float_field(raw, "digital", "scoring.media.digital"),
        "cd": _float_field(raw, "cd", "scoring.media.cd"),
        "other_non_digital": _float_field(
            raw, "other_non_digital", "scoring.media.other_non_digital"
        ),
        "vinyl": _float_field(raw, "vinyl", "scoring.media.vinyl"),
        "other_physical": _float_field(raw, "other_physical", "scoring.media.other_physical"),
    }


def _release_group_weights(raw: dict[str, Any] | None) -> dict[str, Any]:
    if raw is None:
        return {
            "primary_album": 30.0,
            "primary_ep": 15.0,
            "primary_single": 0.0,
            "secondary_penalty": -60.0,
            "penalised_secondary_types": _default_penalised_secondary_types(),
        }
    return {
        "primary_album": _float_field(
            raw, "primary_album", "scoring.release_group.primary_album"
        ),
        "primary_ep": _float_field(raw, "primary_ep", "scoring.release_group.primary_ep"),
        "primary_single": _float_field(
            raw, "primary_single", "scoring.release_group.primary_single"
        ),
        "secondary_penalty": _float_field(
            raw, "secondary_penalty", "scoring.release_group.secondary_penalty"
        ),
        "penalised_secondary_types": _str_list_field(
            raw,
            "penalised_secondary_types",
            "scoring.release_group.penalised_secondary_types",
            _default_penalised_secondary_types(),
        ),
    }


def _bonus_weights(raw: dict[str, Any] | None) -> dict[str, Any]:
    if raw is None:
        return {
            "canonical_track_count": 10.0,
            "label_present": 8.0,
            "edition_bonus": 15.0,
            "edition_filler_cap": 25.0,
        }
    return {
        "canonical_track_count": _float_field(
            raw, "canonical_track_count", "scoring.bonus.canonical_track_count"
        ),
        "label_present": _float_field(raw, "label_present", "scoring.bonus.label_present"),
        "edition_bonus": _float_field(
            raw, "edition_bonus", "scoring.bonus.edition_bonus", 15.0
        ),
        "edition_filler_cap": _float_field(
            raw, "edition_filler_cap", "scoring.bonus.edition_filler_cap", 25.0
        ),
    }


def _term_weights(raw: dict[str, Any] | None) -> dict[str, Any]:
    if raw is None:
        return {
            "strong_cost": 45.0,
            "medium_cost": 25.0,
            "weak_cost": 10.0,
            "cap": 180.0,
            "strong": _default_strong_terms(),
            "medium": _default_medium_terms(),
            "weak": _default_weak_terms(),
        }
    return {
        "strong_cost": _float_field(raw, "strong_cost", "scoring.terms.strong_cost"),
        "medium_cost": _float_field(raw, "medium_cost", "scoring.terms.medium_cost"),
        "weak_cost": _float_field(raw, "weak_cost", "scoring.terms.weak_cost"),
        "cap": _float_field(raw, "cap", "scoring.terms.cap"),
        "strong": _str_list_field(
            raw, "strong", "scoring.terms.strong", _default_strong_terms()
        ),
        "medium": _str_list_field(
            raw, "medium", "scoring.terms.medium", _default_medium_terms()
        ),
        "weak": _str_list_field(raw, "weak", "scoring.terms.weak", _default_weak_terms()),
    }


def _scoring(raw: dict[str, Any]) -> dict[str, Any]:
    return {
        "status": _status_weights(_subtable(raw, "status", "scoring.status")),
        "media": _media_weights(_subtable(raw, "media", "scoring.media")),
        "release_group": _release_group_weights(
            _subtable(raw, "release_group", "scoring.release_group")
        ),
        "bonus": _bonus_weights(_subtable(raw, "bonus", "scoring.bonus")),
        "terms": _term_weights(_subtable(raw, "terms", "scoring.terms")),
    }


def _is_unset(value: str) -> bool:
    trimmed = value.strip()
    return not trimmed or trimmed == _UNSET_SENTINEL


def _validate(cfg: Config) -> None:
    if _is_unset(cfg.musicbrainz.user_agent):
        value = json.dumps(cfg.musicbrainz.user_agent, ensure_ascii=False)
        raise MimportError(
            f"musicbrainz.user_agent is not set meaningfully: {value}\nset it in config.toml"
        )
    for field in ("username", "password"):
        if _is_unset(getattr(cfg.slskd, field)):
            raise MimportError(f"slskd.{field} is not set meaningfully\nset it in config.toml")
    _validate_base_url("musicbrainz.base_url", cfg.musicbrainz.base_url)
    _validate_base_url("lidarr.base_url", cfg.lidarr.base_url)
    _validate_base_url("slskd.base_url", cfg.slskd.base_url)
    _validate_base_url("cover_art.base_url", cfg.cover_art.base_url)


def _load(path: str | Path) -> Config:
    source = Path(path)
    try:
        with source.open("rb") as handle:
            root = tomllib.load(handle)
    except tomllib.TOMLDecodeError as exc:
        raise _config_error(str(exc)) from exc
    except UnicodeDecodeError as exc:
        raise io_error(source, exc) from exc
    except OSError as exc:
        raise io_error(source, exc) from exc

    cfg = Config(
        paths=_paths(_section(root, "paths", required=True)),
        musicbrainz=_musicbrainz(_section(root, "musicbrainz", required=True)),
        lidarr=_lidarr(_section(root, "lidarr", required=True)),
        slskd=_slskd(_section(root, "slskd", required=True)),
        quality=_quality(_section(root, "quality", required=False)),
        scoring=_scoring(_section(root, "scoring", required=False)),
        cover_art=_cover_art(_section(root, "cover_art", required=False)),
        yt=_yt(_section(root, "yt", required=False)),
    )
    _validate(cfg)
    return cfg


def load(path: str | Path) -> Config:
    return _load(path)
