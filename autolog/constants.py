from typing import Dict

APP_WIDTH = 1000
APP_HEIGHT = 600

APP_MIN_WIDTH = 750
APP_MIN_HEIGHT = 500

COOLDOWN_SEC: float = 2.0
COOLDOWN_EVERY: int = 10

BASE_TABLE_COLUMNS = ("Started", "Duration", "Issue")
TRAILING_TABLE_COLUMNS = ("Status", "Error")

DEFAULT_COLUMN_WIDTH = 150
TABLE_ROW_HEIGHT = 28
TABLE_COLUMN_WIDTHS: Dict[str, int] = {
    "Started": 130,
    "Duration": 75,
    "Issue": 140,
    "Project": 140,
    "Activity": 200,
    "Status": 80,
    "Error": 190,
}

# Tk renders every non-ASCII character as a literal \uXXXX escape on some
# Linux setups, so all user facing strings stay ASCII.
UNMATCHED_PREFIX = "? "
APPLY_TO_ALL_PLACEHOLDER = "-- apply to all --"

STATUS_DISPLAY = {
    "pending": "Pending",
    "success": "Success",
    "failed": "Failed",
    "skipped": "Skipped",
}
