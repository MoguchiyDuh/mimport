import json
import logging
import os
import shutil
import stat
import tempfile
import uuid
from collections.abc import Callable
from pathlib import Path
from types import TracebackType
from typing import Any

from mimport.errors import MimportError, io_error


def same_file(first: Path, second: Path) -> bool:
    try:
        return first.samefile(second)
    except OSError:
        return first.absolute() == second.absolute()


def contained(root: Path, path: Path) -> bool:
    return path.resolve().is_relative_to(root.resolve())


def fingerprint(path: Path) -> tuple[int, int, int, int]:
    stat = path.stat()
    return stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns


class FileBatch:
    def __init__(self, root: Path, database: Path) -> None:
        self.root = root
        self.database = database
        self.path = root / f".mimport-operation-{uuid.uuid4().hex}.json"
        self.operations: list[dict[str, Any]] = []
        self.directories: list[Path] = []
        self.phase = "preparing"
        self.committed = False

    def __enter__(self) -> "FileBatch":
        return self

    def _mkdir(self, directory: Path) -> None:
        absent: list[Path] = []
        current = directory
        while not current.exists():
            absent.append(current)
            current = current.parent
        directory.mkdir(parents=True, exist_ok=True)
        self.directories.extend(reversed(absent))
        return

    def _save(self) -> None:
        self._mkdir(self.root)
        temporary = self.path.with_suffix(".tmp")
        payload = {
            "phase": self.phase,
            "database": str(self.database),
            "operations": self.operations,
        }
        with temporary.open("w", encoding="utf-8") as output:
            json.dump(payload, output, ensure_ascii=False, indent=2)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, self.path)
        return

    def add(
        self,
        source: Path,
        destination: Path,
        original: Path,
        writer: Callable[[Path], None],
    ) -> None:
        if not contained(self.root, destination) or destination.resolve() == self.root.resolve():
            raise MimportError(f"destination escapes library root: {destination}")
        replacing = destination.exists() and same_file(original, destination)
        if destination.exists() and not replacing:
            raise io_error(destination, FileExistsError("destination already exists"))
        if destination.is_symlink():
            raise MimportError(f"refusing to replace a symbolic link: {destination}")
        self._mkdir(destination.parent)
        descriptor, name = tempfile.mkstemp(
            prefix=f".{destination.stem}.mimport-tmp.",
            suffix=destination.suffix,
            dir=destination.parent,
        )
        os.close(descriptor)
        temporary = Path(name)
        operation = {
            "source": str(original),
            "temporary": str(temporary),
            "destination": str(destination),
            "backup": None,
            "replacing": replacing,
            "original_identity": list(fingerprint(destination)) if replacing else None,
            "published_identity": None,
            "published": False,
        }
        self.operations.append(operation)
        self._save()
        shutil.copy2(source, temporary)
        temporary.chmod(temporary.stat().st_mode | stat.S_IWUSR)
        writer(temporary)
        with temporary.open("rb") as prepared:
            os.fsync(prepared.fileno())
        return

    def publish(self) -> None:
        self.phase = "publishing"
        self._save()
        for operation in self.operations:
            destination = Path(operation["destination"])
            temporary = Path(operation["temporary"])
            if not contained(self.root, destination):
                raise MimportError(f"destination escapes library root: {destination}")
            if operation["replacing"]:
                if list(fingerprint(destination)) != operation["original_identity"]:
                    raise MimportError(f"destination changed during import: {destination}")
                backup = destination.with_name(f".{destination.name}.mimport-backup.{uuid.uuid4().hex}")
                operation["backup"] = str(backup)
                self._save()
                os.link(destination, backup)
                operation["published_identity"] = list(fingerprint(temporary))
                self._save()
                if list(fingerprint(destination)) != operation["original_identity"]:
                    raise MimportError(f"destination changed during import: {destination}")
                os.replace(temporary, destination)
            else:
                operation["published_identity"] = list(fingerprint(temporary))
                self._save()
                os.link(temporary, destination)
                temporary.unlink()
            operation["published"] = True
            self._save()
        self.phase = "published"
        self._save()
        return

    def finish(self) -> None:
        self.committed = True
        self.phase = "committed"
        try:
            self._save()
            for operation in self.operations:
                for key in ("temporary", "backup"):
                    if operation[key] is not None:
                        Path(operation[key]).unlink(missing_ok=True)
            self.path.unlink(missing_ok=True)
        except OSError as exc:
            logging.warning("import committed; retained recovery information at %s: %s", self.path, exc)
        return

    def rollback(self) -> None:
        failures: list[str] = []
        for operation in reversed(self.operations):
            destination = Path(operation["destination"])
            backup = Path(operation["backup"]) if operation["backup"] is not None else None
            try:
                published_identity = operation["published_identity"]
                owns_destination = (
                    destination.exists()
                    and published_identity is not None
                    and list(fingerprint(destination)) == published_identity
                )
                if operation["published"] or owns_destination:
                    if not owns_destination:
                        raise MimportError(f"destination changed; manual recovery required: {destination}")
                    if operation["replacing"] and (backup is None or not backup.exists()):
                        raise MimportError(f"original backup missing; retaining published file: {destination}")
                    if backup is not None and backup.exists():
                        os.replace(backup, destination)
                    else:
                        destination.unlink()
                if backup is not None:
                    backup.unlink(missing_ok=True)
                Path(operation["temporary"]).unlink(missing_ok=True)
            except (OSError, MimportError) as exc:
                failures.append(str(exc))
        if failures:
            raise MimportError(
                f"rollback incomplete; recovery information: {self.path}\n" + "\n".join(failures)
            )
        self.path.unlink(missing_ok=True)
        self.path.with_suffix(".tmp").unlink(missing_ok=True)
        for directory in reversed(self.directories):
            try:
                directory.rmdir()
            except OSError:
                pass
        return

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> bool:
        if not self.committed:
            try:
                self.rollback()
            except (MimportError, OSError) as rollback_error:
                raise MimportError(f"{exc or 'import not committed'}\n{rollback_error}") from exc
        return False
