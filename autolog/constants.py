from typing import Dict

APP_WIDTH = 800
APP_HEIGHT = 600

APP_MIN_WIDTH = 650
APP_MIN_HEIGHT = 500

COOLDOWN_SEC: float = 2.0
COOLDOWN_EVERY: int = 10

BASE_TABLE_COLUMNS = ("Started", "Duration", "Issue")
TRAILING_TABLE_COLUMNS = ("Status", "Error")

DEFAULT_COLUMN_WIDTH = 150
TABLE_COLUMN_WIDTHS: Dict[str, int] = {
    "Started": 160,
    "Duration": 100,
    "Issue": 150,
    "Status": 80,
    "Error": 400,
}


STATUS_DISPLAY = {
    "pending": "🔄 Pending",
    "success": "✅ Success",
    "failed": "❌ Failed",
    "skipped": "⏭️ Skipped",
}
