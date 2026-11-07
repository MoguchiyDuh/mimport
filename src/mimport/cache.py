from __future__ import annotations

import os
import time
from pathlib import Path

from .errors import io_error

__all__ = ("DiskCache", "cache_dir_default")


def _fnv1a(data: bytes) -> int:
    value = 0xCBF29CE484222325
    for byte in data:
        value ^= byte
        value = (value * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return value


class DiskCache:
    def __init__(self, directory: Path, ttl_secs: int) -> None:
        self.directory = Path(directory)
        self.ttl_secs = ttl_secs

    def _path_for(self, key: str) -> Path:
        hash_value = _fnv1a(key.encode("utf-8"))
        slug = "".join(char for char in key if char.isascii() and char.isalnum())[:40]
        return self.directory / f"{slug}-{hash_value:016x}.json"

    def get(self, key: str) -> str | None:
        path = self._path_for(key)
        try:
            stat = path.stat()
        except (OSError, ValueError):
            return None
        now_ns = time.time_ns()
        modified_ns = stat.st_mtime_ns
        if modified_ns > now_ns:
            return None
        if now_ns - modified_ns > self.ttl_secs * 1_000_000_000:
            return None
        try:
            return path.read_bytes().decode("utf-8")
        except (OSError, ValueError):
            return None

    def put(self, key: str, value: str) -> None:
        try:
            self.directory.mkdir(parents=True, exist_ok=True)
        except (OSError, ValueError) as exc:
            raise io_error(self.directory, exc) from exc
        path = self._path_for(key)
        file_name = path.name or "entry"
        tmp_path = self.directory / f".{file_name}.{os.getpid()}.tmp"
        data = value.encode("utf-8")
        try:
            tmp_path.write_bytes(data)
        except (OSError, ValueError) as exc:
            try:
                tmp_path.unlink()
            except (OSError, ValueError):
                pass
            raise io_error(tmp_path, exc) from exc
        try:
            os.replace(tmp_path, path)
        except (OSError, ValueError) as exc:
            try:
                tmp_path.unlink()
            except (OSError, ValueError):
                pass
            raise io_error(path, exc) from exc


def cache_dir_default(name: str) -> Path:
    if "XDG_CACHE_HOME" in os.environ:
        base = Path(os.environ["XDG_CACHE_HOME"])
    else:
        base = Path(os.environ.get("HOME", ".")) / ".cache"
    return base / "mimport" / name
