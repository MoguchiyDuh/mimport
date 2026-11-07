from __future__ import annotations

import math
from decimal import Decimal
from functools import cmp_to_key
from typing import Any

_DEFAULTS: dict[str, dict[str, Any]] = {
    "status": {"official": 100.0, "other": -120.0},
    "media": {
        "digital": 70.0,
        "cd": -20.0,
        "other_non_digital": -20.0,
        "vinyl": -100.0,
        "other_physical": -95.0,
    },
    "release_group": {
        "primary_album": 30.0,
        "primary_ep": 15.0,
        "primary_single": 0.0,
        "secondary_penalty": -60.0,
        "penalised_secondary_types": [
            "compilation",
            "live",
            "remix",
            "demo",
            "interview",
            "spokenword",
        ],
    },
    "bonus": {
        "canonical_track_count": 10.0,
        "label_present": 8.0,
        "edition_bonus": 15.0,
        "edition_filler_cap": 25.0,
    },
    "terms": {
        "strong_cost": 45.0,
        "medium_cost": 25.0,
        "weak_cost": 10.0,
        "cap": 180.0,
        "strong": ["live", "remix", "karaoke", "commentary", "interview", "rehearsal"],
        "medium": ["demo", "instrumental", "acoustic", "alternate take", "alt take", "session"],
        "weak": ["radio edit", "edit", "version"],
    },
}


def _section(scoring: dict[str, Any], name: str) -> dict[str, Any]:
    value = scoring.get(name)
    return value if isinstance(value, dict) else {}


def _weight(scoring: dict[str, Any], section: str, key: str) -> float:
    return float(_section(scoring, section).get(key, _DEFAULTS[section][key]))


def _terms(scoring: dict[str, Any], key: str) -> list[str]:
    value = _section(scoring, "terms").get(key, _DEFAULTS["terms"][key])
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, str)]


def _fmt_float(value: float) -> str:
    if math.isnan(value):
        return "NaN"
    if math.isinf(value):
        return "inf" if value > 0 else "-inf"
    if value == 0.0:
        return "-0" if math.copysign(1.0, value) < 0 else "0"
    text = repr(float(value))
    if "e" in text or "E" in text:
        text = format(Decimal(text), "f")
    if text.endswith(".0"):
        text = text[:-2]
    return text


def _rust_lower(text: str) -> str:
    return "".join(char.lower() for char in text)


def _is_rust_whitespace(char: str) -> bool:
    code = ord(char)
    return (
        0x0009 <= code <= 0x000D
        or code in (0x0020, 0x0085, 0x00A0, 0x1680, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000)
        or 0x2000 <= code <= 0x200A
    )


def _rust_trim(text: str) -> str:
    start = 0
    end = len(text)
    while start < end and _is_rust_whitespace(text[start]):
        start += 1
    while end > start and _is_rust_whitespace(text[end - 1]):
        end -= 1
    return text[start:end]


def normalize_title(title: str) -> str:
    text = _rust_lower(_rust_trim(title))
    text = text.replace("\u2018", "'").replace("\u2019", "'").replace("\u2032", "'")
    text = text.replace("\u201c", '"').replace("\u201d", '"')
    return text.replace("\u2013", "-").replace("\u2014", "-")


def _levenshtein(a: str, b: str) -> int:
    n = len(a)
    m = len(b)
    if n == 0:
        return m
    if m == 0:
        return n
    prev = list(range(m + 1))
    curr = [0] * (m + 1)
    for i in range(1, n + 1):
        curr[0] = i
        for j in range(1, m + 1):
            cost = 0 if a[i - 1] == b[j - 1] else 1
            curr[j] = min(prev[j] + 1, curr[j - 1] + 1, prev[j - 1] + cost)
        prev, curr = curr, prev
    return prev[m]


def text_similarity(a: str, b: str) -> float:
    left = normalize_title(a)
    right = normalize_title(b)
    max_len = max(len(left), len(right))
    if max_len == 0:
        return 1.0
    distance = _levenshtein(left, right)
    return 1.0 - (distance / max_len)


def canonical_track_count(releases: list[dict[str, Any]]) -> int | None:
    counts = [int(release.get("track_count") or 0) for release in releases]
    counts = [count for count in counts if count > 0]
    if not counts:
        return None
    counts.sort()
    best_count = counts[0]
    best_freq = 0
    current_count = counts[0]
    current_freq = 0
    for count in counts:
        if count == current_count:
            current_freq += 1
        else:
            current_count = count
            current_freq = 1
        if current_freq > best_freq:
            best_count = current_count
            best_freq = current_freq
    return best_count


def _contains_term(hay: str, term: str) -> bool:
    if term == "":
        return hay == ""
    start = 0
    while True:
        pos = hay.find(term, start)
        if pos < 0:
            return False
        after = pos + len(term)
        before_ok = pos == 0 or not hay[pos - 1].isalnum()
        after_ok = after >= len(hay) or not hay[after].isalnum()
        if before_ok and after_ok:
            return True
        start = after


def _term_penalty(text: str, scoring: dict[str, Any]) -> float:
    hay = _rust_lower(text)
    total = 0.0
    strong_cost = _weight(scoring, "terms", "strong_cost")
    medium_cost = _weight(scoring, "terms", "medium_cost")
    weak_cost = _weight(scoring, "terms", "weak_cost")
    for term in _terms(scoring, "strong"):
        if _contains_term(hay, term):
            total += strong_cost
    for term in _terms(scoring, "medium"):
        if _contains_term(hay, term):
            total += medium_cost
    for term in _terms(scoring, "weak"):
        if _contains_term(hay, term):
            total += weak_cost
    return total


def _tracklist_penalty(release: dict[str, Any], scoring: dict[str, Any]) -> float:
    total = 0.0
    for track in release.get("tracks") or []:
        total += _term_penalty(track.get("title") if isinstance(track.get("title"), str) else "", scoring)
    return min(total, _weight(scoring, "terms", "cap"))


class _Breakdown:
    def __init__(self, release: dict[str, Any]) -> None:
        self.data: dict[str, Any] = {
            "release_id": release.get("id") if isinstance(release.get("id"), str) else "",
            "title": release.get("title") if isinstance(release.get("title"), str) else "",
            "disambiguation": release.get("disambiguation") if isinstance(release.get("disambiguation"), str) else None,
            "total": 0.0,
            "components": [],
            "status": release.get("status") if isinstance(release.get("status"), str) else None,
            "country": release.get("country") if isinstance(release.get("country"), str) else None,
            "label": release.get("label") if isinstance(release.get("label"), str) else None,
            "formats": list(release.get("formats") or []),
            "track_count": int(release.get("track_count") or 0),
            "date": release.get("date") if isinstance(release.get("date"), str) else None,
        }

    def push(self, label: str, points: float) -> None:
        if points != 0.0:
            self.data["total"] += points
            self.data["components"].append({"label": label, "points": float(points)})


def _score_status(release: dict[str, Any], scoring: dict[str, Any], breakdown: _Breakdown) -> None:
    status = release.get("status")
    if not isinstance(status, str):
        return
    lowered = _rust_lower(status)
    if lowered == "official":
        breakdown.push("official", _weight(scoring, "status", "official"))
    else:
        breakdown.push(f"status={lowered}", _weight(scoring, "status", "other"))


def _score_media(formats: list[str], scoring: dict[str, Any], breakdown: _Breakdown) -> None:
    if not formats:
        return
    lowered = [_rust_lower(fmt) for fmt in formats]
    worst_format: str | None = None
    worst_points = 0.0
    for fmt in lowered:
        if "digital" in fmt:
            points = _weight(scoring, "media", "digital")
        elif "vinyl" in fmt:
            points = _weight(scoring, "media", "vinyl")
        elif "cassette" in fmt or "shellac" in fmt or "8-track" in fmt or "cylinder" in fmt:
            points = _weight(scoring, "media", "other_physical")
        elif "cd" in fmt:
            points = _weight(scoring, "media", "cd")
        else:
            points = _weight(scoring, "media", "other_non_digital")
        if worst_format is None or points < worst_points:
            worst_format = fmt
            worst_points = points
    if worst_format is not None:
        breakdown.push(f"media={worst_format}", worst_points)


def _score_tracks(
    release: dict[str, Any],
    canonical_tracks: int | None,
    scoring: dict[str, Any],
    breakdown: _Breakdown,
) -> None:
    if canonical_tracks is None:
        return
    count = int(release.get("track_count") or 0)
    if count == 0:
        return
    if count == canonical_tracks:
        breakdown.push(
            f"tracks={count} (canonical)",
            _weight(scoring, "bonus", "canonical_track_count"),
        )
    else:
        breakdown.data["components"].append(
            {"label": f"tracks={count} vs canonical {canonical_tracks}", "points": 0.0}
        )


def _score_metadata(release: dict[str, Any], scoring: dict[str, Any], breakdown: _Breakdown) -> None:
    if release.get("label") is not None:
        breakdown.push("label", _weight(scoring, "bonus", "label_present"))


def _score_terms(release: dict[str, Any], scoring: dict[str, Any], breakdown: _Breakdown) -> None:
    title_penalty = _term_penalty(release.get("title") if isinstance(release.get("title"), str) else "", scoring)
    if title_penalty > 0.0:
        breakdown.push(f"title terms ({_fmt_float(title_penalty)})", -title_penalty)

    tracklist = _tracklist_penalty(release, scoring)
    if tracklist > 0.0:
        breakdown.push(f"tracklist terms ({_fmt_float(tracklist)})", -tracklist)


def _score_bonus_tracks(release: dict[str, Any], scoring: dict[str, Any], breakdown: _Breakdown) -> None:
    title = _rust_lower(release.get("title") if isinstance(release.get("title"), str) else "")
    if "deluxe" not in title and "expanded" not in title and "bonus" not in title:
        return
    tracklist = _tracklist_penalty(release, scoring)
    if tracklist > _weight(scoring, "bonus", "edition_filler_cap"):
        return
    breakdown.push("edition bonus", _weight(scoring, "bonus", "edition_bonus"))


def _score_group(
    group: dict[str, Any] | None,
    scoring: dict[str, Any],
    breakdown: _Breakdown,
) -> None:
    if group is None:
        return
    primary = group.get("primary_type")
    if isinstance(primary, str):
        lowered = _rust_lower(primary)
        if lowered == "album":
            breakdown.push("type=Album", _weight(scoring, "release_group", "primary_album"))
        elif lowered == "ep":
            breakdown.push("type=EP", _weight(scoring, "release_group", "primary_ep"))
        elif lowered == "single":
            breakdown.push("type=Single", _weight(scoring, "release_group", "primary_single"))

    penalised = _section(scoring, "release_group").get(
        "penalised_secondary_types", _DEFAULTS["release_group"]["penalised_secondary_types"]
    )
    if not isinstance(penalised, list):
        penalised = []
    for secondary in group.get("secondary_types") or []:
        if not isinstance(secondary, str):
            continue
        lowered = _rust_lower(secondary)
        if any(item == lowered for item in penalised):
            breakdown.push(
                f"secondary={secondary}",
                _weight(scoring, "release_group", "secondary_penalty"),
            )


def score_release(
    release: dict[str, Any],
    scoring: dict[str, Any],
    group: dict[str, Any] | None = None,
    canonical_tracks: int | None = None,
) -> dict[str, Any]:
    breakdown = _Breakdown(release)
    _score_status(release, scoring, breakdown)
    _score_media(list(release.get("formats") or []), scoring, breakdown)
    _score_tracks(release, canonical_tracks, scoring, breakdown)
    _score_metadata(release, scoring, breakdown)
    _score_terms(release, scoring, breakdown)
    _score_bonus_tracks(release, scoring, breakdown)
    _score_group(group, scoring, breakdown)
    return breakdown.data


def _date_key(date: str | None) -> tuple[int, str]:
    if isinstance(date, str) and _rust_trim(date) != "":
        return (0, date)
    return (1, "")


def _compare(a: dict[str, Any], b: dict[str, Any]) -> int:
    a_total = float(a.get("total") or 0.0)
    b_total = float(b.get("total") or 0.0)
    if a_total < b_total:
        return 1
    if a_total > b_total:
        return -1
    a_key = _date_key(a.get("date") if isinstance(a.get("date"), str) else None)
    b_key = _date_key(b.get("date") if isinstance(b.get("date"), str) else None)
    if a_key < b_key:
        return -1
    if a_key > b_key:
        return 1
    a_id = a.get("release_id") if isinstance(a.get("release_id"), str) else ""
    b_id = b.get("release_id") if isinstance(b.get("release_id"), str) else ""
    if a_id < b_id:
        return -1
    if a_id > b_id:
        return 1
    return 0


def rank(scored: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return sorted(scored, key=cmp_to_key(_compare))


def rank_releases(
    releases: list[dict[str, Any]],
    group: dict[str, Any] | None,
    scoring: dict[str, Any],
) -> list[dict[str, Any]]:
    canonical = canonical_track_count(releases)
    scored = [score_release(release, scoring, group, canonical) for release in releases]
    return rank(scored)
