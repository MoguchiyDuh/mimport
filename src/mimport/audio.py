from __future__ import annotations

import os
import subprocess
from pathlib import Path
from typing import Any

from mutagen.flac import FLAC
from mutagen.mp3 import MP3
from mutagen.mp4 import MP4
from mutagen.oggopus import OggOpus
from mutagen.wave import WAVE
from mutagen.wavpack import WavPack

from .errors import MimportError, io_error

AUDIO_EXTENSIONS = ("flac", "mp3", "m4a", "mp4", "ogg", "opus", "wav", "ape", "wv")
LOSSY_EXTENSIONS = ("mp3", "m4a", "mp4", "ogg", "opus", "aac", "wma")


def _ext(path: Path) -> str:
    return path.suffix[1:].lower() if path.suffix else ""


def is_audio(path: Path) -> bool:
    return _ext(path) in AUDIO_EXTENSIONS


def is_lossy(path: Path) -> bool:
    ext = _ext(path)
    if ext in ("m4a", "mp4"):
        try:
            info = MP4(path).info
            return getattr(info, "codec", "") != "alac"
        except Exception:
            return True
    return ext in LOSSY_EXTENSIONS


def _open(path: Path) -> Any:
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
    if ext in ("m4a", "mp4"):
        return MP4(path)
    if ext == "wav":
        return WAVE(path)
    if ext == "ape":
        from mutagen.monkeysaudio import MonkeysAudio

        return MonkeysAudio(path)
    if ext == "wv":
        return WavPack(path)
    import mutagen

    audio = mutagen.File(path)
    if audio is None:
        raise MimportError(f"probe/tag error at {path}: unsupported audio format")
    return audio


def _int_or_none(value: Any) -> int | None:
    if value is None:
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _float_or_none(value: Any) -> float | None:
    if value is None:
        return None
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def probe(path: Path) -> dict[str, Any]:
    try:
        file_size = path.stat().st_size
    except OSError as exc:
        raise io_error(path, exc)
    try:
        audio = _open(path)
        info = getattr(audio, "info", None)
        if info is None:
            raise MimportError(f"probe/tag error at {path}: no audio stream information")
        return {
            "duration_secs": _float_or_none(getattr(info, "length", None)),
            "sample_rate": _int_or_none(getattr(info, "sample_rate", None))
            or (48000 if isinstance(audio, OggOpus) else None),
            "bits_per_sample": _int_or_none(getattr(info, "bits_per_sample", None)),
            "channels": _int_or_none(getattr(info, "channels", None)),
            "file_size": file_size,
        }
    except MimportError:
        raise
    except Exception as exc:
        raise MimportError(f"probe/tag error at {path}: {exc}")


def bitrate_bps(info: dict[str, Any]) -> float | None:
    duration = info.get("duration_secs")
    file_size = info.get("file_size")
    if duration is None or file_size is None or duration <= 0:
        return None
    return (float(file_size) * 8.0) / float(duration)


def pcm_bps(info: dict[str, Any]) -> float | None:
    sample_rate = info.get("sample_rate")
    bits_per_sample = info.get("bits_per_sample")
    if sample_rate is None or bits_per_sample is None:
        return None
    channels = info.get("channels")
    if channels is None:
        channels = 2
    return float(sample_rate) * float(bits_per_sample) * float(channels)


def compression_ratio(info: dict[str, Any]) -> float | None:
    bitrate = bitrate_bps(info)
    pcm = pcm_bps(info)
    if bitrate is None or pcm is None or pcm <= 0:
        return None
    return bitrate / pcm


def needs_downsample(info: dict[str, Any], target_rate: int, target_depth: int) -> bool:
    sample_rate = info.get("sample_rate")
    if sample_rate is not None and int(sample_rate) > int(target_rate):
        return True
    bits_per_sample = info.get("bits_per_sample")
    if bits_per_sample is not None and int(bits_per_sample) > int(target_depth):
        return True
    return False


def _temp_sibling(path: Path, ext: str) -> Path:
    return path.with_name(f"{path.name}.mimport-tmp.{os.getpid()}.{ext}")


def _run_ffmpeg(args: list[str], path: Path, tmp: Path) -> None:
    try:
        result = subprocess.run(
            args,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except FileNotFoundError:
        raise MimportError("required external tool not found on PATH: ffmpeg")
    except OSError as exc:
        raise io_error(path, exc)
    if result.returncode != 0:
        try:
            tmp.unlink()
        except OSError:
            pass
        status = f"exit status: {result.returncode}"
        raise MimportError(f"ffmpeg failed ({status})\n{result.stderr[:600]}")


def downsample(path: Path, target_rate: int, target_depth: int) -> Path:
    if is_lossy(path):
        raise MimportError(
            f"invalid config value: refusing to transcode lossy file {path}: downsampling applies to lossless only"
        )
    if int(target_depth) == 16:
        sample_fmt = "s16"
    elif int(target_depth) == 24:
        sample_fmt = "s32"
    else:
        raise MimportError(f"invalid config value: unsupported target bit depth {target_depth}")

    dst = path if _ext(path) == "flac" else path.with_suffix(".flac")
    if dst != path and dst.exists():
        raise MimportError(
            f"invalid config value: refusing to downsample {path}: {dst} already exists"
        )

    tmp = _temp_sibling(path, "flac")
    audio_filter = (
        f"aresample={int(target_rate)}:resampler=soxr:precision=28:"
        f"osf={sample_fmt}:dither_method=triangular_hp"
    )
    _run_ffmpeg(
        [
            "ffmpeg",
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            str(path),
            "-map",
            "0",
            "-c:v",
            "copy",
            "-af",
            audio_filter,
            "-sample_fmt",
            sample_fmt,
            "-map_metadata",
            "0",
            "-metadata",
            "encoder=",
            str(tmp),
        ],
        path,
        tmp,
    )
    try:
        os.replace(tmp, dst)
    except OSError as exc:
        try:
            tmp.unlink()
        except OSError:
            pass
        raise io_error(path, exc)
    if dst != path:
        try:
            path.unlink()
        except OSError as exc:
            raise io_error(path, exc)
    return dst


def transcode_opus(path: Path, bitrate_kbps: int) -> Path:
    if is_lossy(path):
        raise MimportError(
            f"invalid config value: refusing to transcode lossy file {path}: opus transcode applies to lossless only"
        )
    dst = path.with_suffix(".opus")
    if dst.exists():
        raise MimportError(
            f"invalid config value: refusing to transcode {path}: {dst} already exists"
        )

    tmp = _temp_sibling(path, "opus")
    _run_ffmpeg(
        [
            "ffmpeg",
            "-nostdin",
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            str(path),
            "-map",
            "0:a:0",
            "-map_metadata",
            "0",
            "-map_chapters",
            "-1",
            "-af",
            "aresample=48000:filter_size=64:phase_shift=10",
            "-c:a",
            "libopus",
            "-b:a",
            f"{int(bitrate_kbps)}k",
            "-vbr",
            "on",
            "-compression_level",
            "10",
            "-application",
            "audio",
            "-fflags",
            "+bitexact",
            "-f",
            "opus",
            str(tmp),
        ],
        path,
        tmp,
    )
    try:
        os.replace(tmp, dst)
    except OSError as exc:
        try:
            tmp.unlink()
        except OSError:
            pass
        raise io_error(dst, exc)
    try:
        path.unlink()
    except OSError as exc:
        raise io_error(path, exc)
    return dst


def is_temp_file(path: Path) -> bool:
    return ".mimport-tmp." in path.name


def suspicion_reason(info: dict[str, Any]) -> str | None:
    ratio = compression_ratio(info)
    if ratio is not None and ratio < 0.30:
        return "ImplausibleCompression"
    bitrate = bitrate_bps(info)
    sample_rate = info.get("sample_rate")
    if (
        bitrate is not None
        and bitrate < 400_000.0
        and sample_rate is not None
        and int(sample_rate) >= 44_100
    ):
        return "ImplausibleBitrate"
    return None


def is_suspected_lossy(info: dict[str, Any]) -> bool:
    return suspicion_reason(info) is not None
