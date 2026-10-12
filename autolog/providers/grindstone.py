from .base import ProviderBase


def _duration_to_seconds(value: str) -> int:
    """Grindstone durations look like `H:MM:SS`, or `D.HH:MM:SS` past a day."""
    days, _, clock = value.rpartition(".")
    parts = [int(p) for p in clock.split(":")]
    while len(parts) < 3:
        parts.append(0)
    hours, minutes, seconds = parts[:3]
    return int(days or 0) * 86400 + hours * 3600 + minutes * 60 + seconds


class GrindstoneProvider(ProviderBase):
    field_map = {
        "started": "Start",
        "duration": "Duration",
        "description": "Notes",
        "activity": "Work Item",
        "raw_issue_key": "Work Item",
    }

    def _post_process(self, mapped_data: dict[str, str]) -> dict[str, str]:
        try:
            mapped_data["duration"] = _duration_to_seconds(mapped_data["duration"])
        except ValueError:
            pass
        return mapped_data
