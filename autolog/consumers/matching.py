import re
from difflib import SequenceMatcher
from typing import Iterable

_SEPARATORS = re.compile(r"[^a-z0-9]+")

MATCH_THRESHOLD = 0.6


def normalize(text: str) -> str:
    return _SEPARATORS.sub(" ", (text or "").lower()).strip()


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

        score, candidate = max(
            (SequenceMatcher(None, target, value).ratio(), candidate)
            for candidate, value in normalized.items()
        )
        if score >= threshold:
            return candidate

    return None
