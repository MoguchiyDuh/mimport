from __future__ import annotations

from typing import Any

JUNK_ALIAS_TYPES = ("Search hint",)

_NON_LATIN_RANGES = (
    (0x3040, 0x309F),
    (0x30A0, 0x30FF),
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xF900, 0xFAFF),
    (0x3000, 0x303F),
    (0xFF00, 0xFFEF),
    (0xAC00, 0xD7A3),
    (0x1100, 0x11FF),
    (0x3130, 0x318F),
)


def _is_non_latin_char(char: str) -> bool:
    code = ord(char)
    return any(start <= code <= end for start, end in _NON_LATIN_RANGES)


def is_non_latin(text: str) -> bool:
    return any(_is_non_latin_char(char) for char in text)


def _alias_type(alias: dict[str, Any]) -> str | None:
    value = alias.get("alias_type")
    if value is None:
        value = alias.get("type")
    return value if isinstance(value, str) else None


def pick_alias(aliases: list[dict[str, Any]] | None) -> str | None:
    candidates: list[dict[str, Any]] = []
    for alias in aliases or []:
        if not isinstance(alias, dict):
            continue
        name = alias.get("name")
        if not isinstance(name, str) or is_non_latin(name):
            continue
        alias_type = _alias_type(alias)
        if alias_type is not None and alias_type in JUNK_ALIAS_TYPES:
            continue
        locale = alias.get("locale")
        if isinstance(locale, str) and locale.startswith("en"):
            continue
        candidates.append(alias)

    def key(alias: dict[str, Any]) -> tuple[int, int]:
        locale = alias.get("locale")
        romaji_rank = 0 if isinstance(locale, str) and locale.endswith("Latn") else 1
        primary_rank = 0 if alias.get("primary") is True else 1
        return (romaji_rank, primary_rank)

    candidates.sort(key=key)
    if not candidates:
        return None
    name = candidates[0].get("name")
    return name if isinstance(name, str) else None
