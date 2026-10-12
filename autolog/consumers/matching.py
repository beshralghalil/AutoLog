import re
from difflib import SequenceMatcher
from typing import Iterable

_SEPARATORS = re.compile(r"[^a-z0-9]+")
_DIGITS = re.compile(r"\d+")

MATCH_THRESHOLD = 0.6


def normalize(text: str) -> str:
    return _SEPARATORS.sub(" ", (text or "").lower()).strip()


def _comparable(target: str, value: str) -> bool:
    """Where two names differ only in their numbers, as ABC-6055 and ABC-6061
    do, the number is the whole meaning and fuzzy scoring must not pair them."""
    if _DIGITS.findall(target) == _DIGITS.findall(value):
        return True
    return _DIGITS.sub("", target).split() != _DIGITS.sub("", value).split()


def best_match(
    candidates: Iterable[str], *hints: str, threshold: float = MATCH_THRESHOLD
) -> str | None:
    """Pick the candidate best describing any hint, strongest hint first."""
    normalized = {candidate: normalize(candidate) for candidate in candidates}
    if not normalized:
        return None

    for hint in hints:
        target = normalize(hint)
        if not target:
            continue

        for candidate, value in normalized.items():
            if value == target:
                return candidate

        target_words = target.split()
        for candidate, value in normalized.items():
            if value in target_words or target in value.split():
                return candidate

        scored = [
            (SequenceMatcher(None, target, value).ratio(), candidate)
            for candidate, value in normalized.items()
            if _comparable(target, value)
        ]
        if scored:
            score, candidate = max(scored)
            if score >= threshold:
                return candidate

    return None
