from __future__ import annotations

import json
from pathlib import Path

__all__ = (
    "MimportError",
    "JobNotFound",
    "TrackNotFound",
    "ServiceNotFound",
    "io_error",
)


class MimportError(Exception):
    pass


def _debug_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


class JobNotFound(MimportError):
    def __init__(self, target: str | int, message: str | None = None) -> None:
        self.target = str(target)
        if message is None:
            message = f"no job matches {_debug_string(self.target)} (not a known job id and no job.title matches it)"
        super().__init__(message)


class TrackNotFound(MimportError):
    def __init__(self, id: int, message: str | None = None) -> None:
        self.id = id
        if message is None:
            message = f"no library track with id {id}"
        super().__init__(message)


class ServiceNotFound(MimportError):
    def __init__(
        self,
        message: str | None = None,
        *,
        service: str | None = None,
        status: int | None = None,
        resource: str | None = None,
    ) -> None:
        self.service = service
        self.status = status
        self.resource = resource
        if message is None:
            if service is not None and resource is not None:
                message = f"{service} has no {resource}"
            elif resource is not None:
                message = f"resource not found: {resource}"
            elif service is not None:
                message = f"{service} resource not found"
            else:
                message = "service resource not found"
        super().__init__(message)


def _os_error_text(exc: BaseException) -> str:
    if isinstance(exc, UnicodeDecodeError):
        return "stream did not contain valid UTF-8"
    if isinstance(exc, OSError) and exc.errno is not None:
        return f"{exc.strerror or str(exc)} (os error {exc.errno})"
    return str(exc)


def io_error(path: str | Path, exc: BaseException) -> MimportError:
    error = MimportError(f"io error at {Path(path)}: {_os_error_text(exc)}")
    error.path = Path(path)
    error.source = exc
    return error
