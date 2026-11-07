import argparse
import json
import logging
import re
import sqlite3
import sys
from pathlib import Path
from typing import Any

from mimport import __version__


def unsigned(value: str) -> int:
    if re.fullmatch(r"\+?[0-9]+", value) is None:
        raise argparse.ArgumentTypeError("expected a non-negative integer")
    try:
        result = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("expected a non-negative integer") from exc
    if result < 0 or result > 18_446_744_073_709_551_615:
        raise argparse.ArgumentTypeError("expected a non-negative 64-bit integer")
    return result


def position(value: str) -> int:
    result = unsigned(value)
    if result > 4_294_967_295:
        raise argparse.ArgumentTypeError("expected a non-negative 32-bit integer")
    return result


def integer(value: str) -> int:
    if re.fullmatch(r"[+-]?[0-9]+", value) is None:
        raise argparse.ArgumentTypeError("expected an integer")
    try:
        result = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("expected an integer") from exc
    if not -9_223_372_036_854_775_808 <= result <= 9_223_372_036_854_775_807:
        raise argparse.ArgumentTypeError("expected a 64-bit integer")
    return result


def _globals(parser: argparse.ArgumentParser, root: bool = False) -> None:
    parser.add_argument(
        "--config", type=Path, default=Path("config.toml") if root else argparse.SUPPRESS
    )
    parser.add_argument(
        "--json", action="store_true", default=False if root else argparse.SUPPRESS
    )
    return


def _command(
    subparsers: Any, name: str, help_text: str | None = None
) -> argparse.ArgumentParser:
    parser = subparsers.add_parser(name, help=help_text, allow_abbrev=False)
    _globals(parser)
    return parser


def _tag_options(parser: argparse.ArgumentParser, importing: bool) -> None:
    parser.add_argument("--tags", type=Path)
    for name in ("artist", "album", "album-artist"):
        parser.add_argument(f"--{name}")
    parser.add_argument("--compilation", action="store_true")
    parser.add_argument("--genre", action="append", default=[])
    parser.add_argument("--allow-native", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    if importing:
        parser.add_argument("--date")
        parser.add_argument("--label")
        parser.add_argument("--track-title", action="append", default=[])
    return


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="mimport", description="Personal music library pipeline", allow_abbrev=False
    )
    parser.add_argument("-V", "--version", action="version", version=f"mimport {__version__}")
    _globals(parser, root=True)
    commands = parser.add_subparsers(dest="command", required=True)
    for backend in ("lidarr", "mb"):
        group = _command(commands, backend)
        actions = group.add_subparsers(dest="action", required=True)
        artist = _command(actions, "artist")
        artist.add_argument("query")
        album = _command(actions, "album")
        album.add_argument("mbid")
        tracks = _command(actions, "tracks")
        if backend == "lidarr":
            tracks.add_argument("release_group_mbid")
        tracks.add_argument("release_mbid")
        if backend == "mb":
            track = _command(actions, "track")
            track.add_argument("text")

    group = _command(commands, "slskd")
    actions = group.add_subparsers(dest="action", required=True)
    search = _command(actions, "search")
    search.add_argument("query")
    search.add_argument("--fresh", action="store_true")
    search.add_argument("--all", action="store_true")
    _command(actions, "searches")
    status = _command(actions, "search-status")
    status.add_argument("id")
    status.add_argument("--all", action="store_true")
    remove = _command(actions, "search-remove")
    remove.add_argument("id")
    fetch = _command(actions, "fetch")
    fetch.add_argument("search_id")
    fetch.add_argument("username")
    fetch.add_argument("directory")
    fetch.add_argument("filenames", nargs="*")
    fetch.add_argument("--title")
    fetch.add_argument("--wait-secs", type=unsigned)
    for name in ("status", "cancel", "retry"):
        action = _command(actions, name)
        action.add_argument("target")
        if name == "cancel":
            action.add_argument("--remove", action="store_true")
        elif name == "retry":
            action.add_argument("--wait-secs", type=unsigned)
    _command(actions, "downloads")
    remove = _command(actions, "remove")
    remove.add_argument("username")
    remove.add_argument("transfer_id")
    _command(actions, "clear-completed")
    browse = _command(actions, "browse")
    browse.add_argument("username")
    browse.add_argument("directory", nargs="?", default="")

    importing = _command(commands, "import")
    importing.add_argument("target")
    importing.add_argument("--release", required=True)
    importing.add_argument("--force", type=Path)
    _tag_options(importing, importing=True)
    cover = importing.add_mutually_exclusive_group()
    cover.add_argument("--cover", type=Path)
    cover.add_argument("--cover-art", action="store_true")
    importing.add_argument("--move", dest="move_files", action="store_true")
    importing.add_argument("--allow-partial", action="store_true")
    importing.add_argument("--cleanup", action="store_true")

    group = _command(commands, "library")
    actions = group.add_subparsers(dest="action", required=True)
    listing = _command(actions, "list")
    listing.add_argument("query", nargs=argparse.REMAINDER)
    show = _command(actions, "show")
    show.add_argument("id", type=integer)
    edit = _command(actions, "edit")
    edit.add_argument("id", type=integer)
    edit.add_argument("--rename", action="store_true")
    edit.add_argument("--dry-run", action="store_true")
    edit.add_argument("fields", nargs=argparse.REMAINDER)
    remove = _command(actions, "remove")
    remove.add_argument("--files", action="store_true")
    remove.add_argument("query", nargs=argparse.REMAINDER)
    prune = _command(actions, "prune")
    prune.add_argument("--dry-run", action="store_true")

    cover = _command(commands, "cover")
    cover.add_argument("--fetch", action="store_true")
    cover.add_argument("query", nargs=argparse.REMAINDER)

    group = _command(commands, "yt")
    actions = group.add_subparsers(dest="action", required=True)
    fetch = _command(actions, "fetch")
    fetch.add_argument("url")
    _tag_options(fetch, importing=False)
    fetch.add_argument("--title")
    fetch.add_argument("--track", type=position)
    fetch.add_argument("--disc", type=position)
    fetch.add_argument("--year")
    fetch.add_argument("--release")
    fetch.add_argument("--playlist", action="store_true")
    fetch.add_argument("--cookies", type=Path)
    fetch.add_argument("--cookies-from-browser")
    completion = _command(commands, "completion")
    completion.add_argument("shell", choices=["zsh"])
    return parser


def _trailing_args(argv: list[str]) -> list[str]:
    command_words: list[tuple[int, str]] = []
    index = 0
    while index < len(argv) and len(command_words) < 2:
        token = argv[index]
        if token == "--config":
            index += 2
            continue
        if token.startswith("--config=") or token == "--json":
            index += 1
            continue
        if token.startswith("-"):
            return argv
        command_words.append((index, token))
        if token == "cover" and len(command_words) == 1:
            break
        index += 1
    if not command_words:
        return argv
    command = command_words[0][1]
    if command == "slskd" and len(command_words) == 2 and command_words[1][1] == "fetch":
        start = command_words[1][0] + 1
        options: list[str] = []
        positional: list[str] = []
        arities = {"--config": 1, "--title": 1, "--wait-secs": 1, "--json": 0, "--help": 0, "-h": 0}
        index = start
        while index < len(argv):
            token = argv[index]
            if token == "--":
                positional.extend(argv[index:])
                break
            if token in arities:
                width = arities[token] + 1
                options.extend(argv[index:index + width])
                index += width
            elif token.partition("=")[0] in arities and "=" in token:
                options.append(token)
                index += 1
            else:
                positional.append(token)
                index += 1
        return argv[:start] + options + positional
    if command == "cover":
        start = command_words[0][0] + 1
        allowed = {"--fetch", "--json", "--config", "-h", "--help"}
    elif command == "library" and len(command_words) == 2:
        action = command_words[1][1]
        if action not in {"list", "remove", "edit"}:
            return argv
        start = command_words[1][0] + 1
        if action == "edit":
            id_index: int | None = None
            while start < len(argv):
                token = argv[start]
                if token == "--config":
                    start += 2
                elif token in {"--json", "--rename", "--dry-run"} or token.startswith("--config="):
                    start += 1
                else:
                    id_index = start
                    start += 1
                    break
            if id_index is not None:
                options: list[str] = []
                end = start
                while end < len(argv):
                    token = argv[end]
                    if token == "--config" and end + 1 < len(argv):
                        options.extend(argv[end:end + 2])
                        end += 2
                    elif token in {"--json", "--rename", "--dry-run", "-h", "--help"} or token.startswith("--config="):
                        options.append(token)
                        end += 1
                    else:
                        break
                if options:
                    argv = argv[:id_index] + options + [argv[id_index]] + argv[end:]
                    start = id_index + len(options) + 1
        allowed = {"--json", "--config", "-h", "--help"}
        if action == "remove":
            allowed.add("--files")
        if action == "edit":
            allowed.update({"--rename", "--dry-run"})
    else:
        return argv
    index = start
    while index < len(argv):
        token = argv[index]
        if token == "--":
            return argv
        if token == "--config":
            index += 2
            continue
        if token in allowed or token.startswith("--config="):
            index += 1
            continue
        if token.startswith("-"):
            return argv[:index] + ["--"] + argv[index:]
        break
    return argv


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = build_parser()
    args = parser.parse_args(_trailing_args(list(sys.argv[1:] if argv is None else argv)))
    for field in ("query", "fields"):
        value = getattr(args, field, None)
        if isinstance(value, list) and value[:1] == ["--"]:
            setattr(args, field, value[1:])
    if args.command == "library" and args.action == "remove" and "--files" in args.query:
        args.query.remove("--files")
        args.files = True
    if args.command == "cover" and "--fetch" in args.query:
        args.query.remove("--fetch")
        args.fetch = True
    if args.command == "yt" and args.playlist:
        for field in ("title", "artist", "album", "track", "disc", "year"):
            if getattr(args, field) is not None:
                parser.error(f"--playlist cannot be used with --{field.replace('_', '-')}")
    return args


def completion_script() -> str:
    parser = build_parser()
    choices: dict[str, list[str]] = {}

    def collect(current: argparse.ArgumentParser, path: tuple[str, ...]) -> None:
        options: list[str] = []
        for action in current._actions:
            options.extend(action.option_strings)
            if isinstance(action, argparse._SubParsersAction):
                options.extend(action.choices)
                for name, child in action.choices.items():
                    collect(child, (*path, name))
        choices[" ".join(path)] = sorted(set(options))
        return

    collect(parser, ())
    cases = []
    for path, options in sorted(choices.items(), key=lambda item: (-len(item[0]), item[0])):
        escaped = path.replace("'", "'\\''")
        candidates = " ".join(options)
        cases.append(f"    '{escaped}') candidates=({candidates}) ;;")
    return "\n".join(
        [
            "_mimport() {",
            "  local context='' word",
            "  local -a candidates",
            "  for word in ${words[2,CURRENT-1]}; do",
            "    case $word in",
            "      lidarr|mb|slskd|import|library|cover|yt|completion|artist|album|tracks|track|search|searches|search-status|search-remove|fetch|status|cancel|retry|downloads|remove|clear-completed|browse|list|show|edit|prune)",
            "        context=\"${context:+$context }$word\" ;;",
            "    esac",
            "  done",
            '  case "$context" in',
            *cases,
            "    *) candidates=(--config --json --help) ;;",
            "  esac",
            "  compadd -- $candidates",
            "  _files",
            "}",
            "compdef _mimport mimport",
            "",
        ]
    )


def run(args: argparse.Namespace) -> Any:
    from mimport.config import load

    cfg = load(args.config)
    if args.command in {"mb", "lidarr"}:
        from mimport.commands_metadata import run as metadata_run

        return metadata_run(cfg, args)
    if args.command == "slskd":
        from mimport.slskd import run as slskd_run

        return slskd_run(cfg, args)
    if args.command == "library":
        from mimport.commands_library import run as library_run

        return library_run(cfg, args)
    if args.command == "import":
        from mimport.importer import run as import_run

        return import_run(cfg, args)
    if args.command == "yt":
        from mimport.commands_yt import run as yt_run

        return yt_run(cfg, args)
    if args.command == "cover":
        from mimport.commands_cover import run as cover_run

        return cover_run(cfg, args)
    raise ValueError(f"unknown command: {args.command}")


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.command == "completion":
        sys.stdout.write(completion_script())
        return 0
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    logging.getLogger("httpx").setLevel(logging.WARNING)
    logging.getLogger("httpcore").setLevel(logging.WARNING)
    from mimport.errors import MimportError

    try:
        result = run(args)
        options: dict[str, Any] = {"ensure_ascii": False, "allow_nan": False, "sort_keys": True}
        if args.json:
            options["separators"] = (",", ":")
        else:
            options["indent"] = 2
        sys.stdout.write(json.dumps(result, **options) + "\n")
    except BrokenPipeError:
        return 1
    except KeyboardInterrupt:
        sys.stderr.write("error: interrupted\n")
        return 130
    except (MimportError, OSError, sqlite3.Error, ValueError) as exc:
        prefix = "db error: " if isinstance(exc, sqlite3.Error) else ""
        sys.stderr.write(f"error: {prefix}{exc}\n")
        return 1
    return 0
