import logging
import time
from pathlib import Path
from typing import Callable, List, Tuple

import pytz

from autolog.constants import COOLDOWN_EVERY, COOLDOWN_SEC
from autolog.consumers.base import ConsumerBase
from autolog.consumers.factory import select_consumer
from autolog.exceptions import DuplicateWorklogError
from autolog.models import ProcessingResult, WorklogEntry
from autolog.parsers.issue_parser import IssueKeyParser
from autolog.providers.factory import select_provider

logger = logging.getLogger(__file__)


class WorklogProcessor:
    """Handles business logic for sending worklog entries to a consumer."""

    def __init__(self, timezone: str):
        self.timezone = timezone
        self.consumer: ConsumerBase | None = None
        self.results: List[ProcessingResult] = []
        self.failed_entries: List[ProcessingResult] = []
        self.total: int = 0

    def configure_consumer(
        self,
        name: str,
        credentials: dict[str, str],
        prevent_duplicates: bool = True,
    ) -> ConsumerBase:
        self.consumer = select_consumer(name, credentials, prevent_duplicates)
        return self.consumer

    def load_entries(
        self, file_path: Path, provider: str
    ) -> Tuple[List[WorklogEntry], str]:
        """Load and preprocess worklog entries from a source file."""
        provider = select_provider(name=provider, file_path=file_path)
        entries = provider.parse()
        total_seconds = 0
        for idx, entry in enumerate(entries):
            entry.status = "pending"
            entry.issue_key = IssueKeyParser.parse(entry.raw_issue_key)
            entry.project_key = IssueKeyParser.parse_project(entry.raw_issue_key)
            entry._idx = idx
            entry.timezone = self.timezone
            tz = pytz.timezone(entry.timezone)
            entry.started = entry.started.astimezone(tz)
            total_seconds += entry.duration
        total_hours = f"{total_seconds // 3600}:{(total_seconds % 3600) // 60}"
        return entries, total_hours

    def resolve_targets(self, entries: List[WorklogEntry]) -> None:
        """Connect and let the consumer guess each entry's destination."""
        self.consumer.connect()
        self.consumer.assign_targets(entries)

    def process_entries(
        self,
        entries: List[WorklogEntry],
        callback: Callable[[int, int, WorklogEntry, ProcessingResult], None],
    ) -> None:
        """Process worklog entries, invoking callback for UI updates."""
        try:
            self.results = []
            self.failed_entries = []
            self.consumer.connect()
            entries_to_process = [
                e for e in entries if e.status in ("pending", "failed", "skipped")
            ]
            self.total = len(entries_to_process)

            if self.consumer.prevent_duplicates:
                self.consumer.preload(entries_to_process)

            for idx, entry in enumerate(entries_to_process, 1):
                result = self._process_single_entry(entry)
                callback(idx, self.total, entry, result)
                if idx % COOLDOWN_EVERY == 0 and idx != len(entries_to_process):
                    time.sleep(COOLDOWN_SEC)

        except Exception as e:
            logger.exception(f"Unexpected error: {e}")
            raise

    def _process_single_entry(self, entry: WorklogEntry) -> ProcessingResult:
        """Process a single worklog entry and update its status."""
        result = self.consumer.create_worklog(entry)
        if result.success:
            entry.status = "success"
        elif isinstance(result.error, DuplicateWorklogError):
            entry.status = "skipped"
        else:
            entry.status = "failed"
        self.results.append(result)
        if not result.success:
            self.failed_entries.append(result)
        return result
