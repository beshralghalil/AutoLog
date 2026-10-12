"""Jira API interaction"""

import logging

import pytz
from dateutil import parser
from jira import JIRA, JIRAError

from autolog.consumers.base import ConsumerBase, CredentialField
from autolog.exceptions import DuplicateWorklogError
from autolog.models import ProcessingResult, WorklogEntry

JIRA_TIMEOUT = 30

logger = logging.getLogger(__name__)


class JiraConsumer(ConsumerBase):
    name = "jira"
    credential_fields = (
        CredentialField("base_url", "Jira Base URL"),
        CredentialField("email", "Email"),
        CredentialField("api_key", "API Key", secret=True),
    )

    def __init__(self, credentials: dict[str, str], prevent_duplicates: bool = True):
        super().__init__(credentials, prevent_duplicates)
        self.client: JIRA | None = None
        self.worklog_cache: dict[str, list] = {}

    def connect(self) -> None:
        self.client = JIRA(
            server=self.credentials["base_url"],
            basic_auth=(self.credentials["email"], self.credentials["api_key"]),
            timeout=JIRA_TIMEOUT,
            async_=True,
        )

    def _convert_worklog(self, worklog) -> WorklogEntry:
        return WorklogEntry(
            activity=worklog.issueId,
            started=parser.parse(worklog.started),
            duration=worklog.timeSpentSeconds,
            description=getattr(worklog, "comment", ""),
            raw_issue_key="",
            issue_key=worklog.issueId,
            timezone="UTC",
        )

    def preload(self, entries: list[WorklogEntry]) -> None:
        if not self.prevent_duplicates:
            return

        for key in {e.issue_key for e in entries if e.issue_key}:
            if key not in self.worklog_cache:
                try:
                    self.worklog_cache[key] = self.client.worklogs(key)
                except JIRAError:
                    self.worklog_cache[key] = []

    def _find_duplicate(self, entry: WorklogEntry) -> WorklogEntry | None:
        for worklog in self.worklog_cache.get(entry.issue_key, []):
            cached = self._convert_worklog(worklog)
            if entry == cached:
                return cached
        return None

    def create_worklog(self, entry: WorklogEntry) -> ProcessingResult:
        if not entry.issue_key:
            return ProcessingResult(False, entry, ValueError("Missing issue key"))

        try:
            if self.prevent_duplicates:
                duplicate = self._find_duplicate(entry)
                if duplicate:
                    logger.warning(f"Duplicate worklog: {entry} conflicts {duplicate}")
                    return ProcessingResult(
                        False,
                        entry,
                        DuplicateWorklogError("Duplicate worklog entry detected"),
                    )

            worklog = self.client.add_worklog(
                issue=entry.issue_key,
                timeSpentSeconds=entry.duration,
                started=(
                    entry.started.astimezone(pytz.UTC)
                    if entry.started.tzinfo
                    else entry.started
                ),
                comment=entry.description,
            )

            if self.prevent_duplicates:
                self.worklog_cache.setdefault(entry.issue_key, []).append(worklog)

            return ProcessingResult(True, entry)
        except JIRAError as e:
            return ProcessingResult(False, entry, e)

    def format_error(self, error: Exception) -> str:
        return str(error.text) if isinstance(error, JIRAError) else str(error)
