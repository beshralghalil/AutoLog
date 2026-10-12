"""Kimai timesheet interaction"""

import logging
from collections import Counter
from datetime import timedelta

import pytz
from dateutil import parser as date_parser

from autolog.consumers.base import ConsumerBase, CredentialField, TargetField
from autolog.consumers.kimai_api import KimaiAPI, KimaiError
from autolog.consumers.matching import best_match
from autolog.exceptions import DuplicateWorklogError
from autolog.models import ProcessingResult, WorklogEntry

logger = logging.getLogger(__name__)

PROJECT_FIELD = TargetField("target_project", "Project")
ACTIVITY_FIELD = TargetField("target_activity", "Activity")


class KimaiConsumer(ConsumerBase):
    name = "kimai"
    credential_fields = (
        CredentialField("base_url", "Kimai Base URL"),
        CredentialField("api_token", "API Token", secret=True),
    )
    target_fields = (PROJECT_FIELD, ACTIVITY_FIELD)

    def __init__(self, credentials: dict[str, str], prevent_duplicates: bool = True):
        super().__init__(credentials, prevent_duplicates)
        self.api: KimaiAPI | None = None
        self.timezone = pytz.UTC
        self._project_ids: dict[str, int] = {}
        self._activities: list[dict] = []
        self._timesheet_cache: list[WorklogEntry] = []

    def connect(self) -> None:
        self.api = KimaiAPI(self.credentials["base_url"], self.credentials["api_token"])
        user = self.api.current_user()
        self.timezone = pytz.timezone(user.get("timezone") or "UTC")
        self._load_targets()

    def _load_targets(self) -> None:
        projects = self.api.projects()
        duplicates = Counter(project["name"] for project in projects)
        self._project_ids = {
            self._project_label(project, duplicates): project["id"]
            for project in projects
        }
        self._activities = self.api.activities()

    @staticmethod
    def _project_label(project: dict, duplicates: Counter) -> str:
        name = project["name"]
        if duplicates[name] == 1:
            return name
        customer = project.get("parentTitle") or project.get("customer")
        return f"{name} ({customer})" if customer else name

    def _project_activities(self, project: str | None) -> list[dict]:
        project_id = self._project_ids.get(project)
        return [a for a in self._activities if a["project"] in (None, project_id)]

    def choices(self, field: TargetField, entry: WorklogEntry) -> list[str]:
        if field == PROJECT_FIELD:
            return sorted(self._project_ids)
        activities = self._project_activities(entry.target_project)
        return sorted({activity["name"] for activity in activities})

    def assign_targets(self, entries: list[WorklogEntry]) -> None:
        for entry in entries:
            if not entry.target_project:
                entry.target_project = best_match(
                    self._project_ids,
                    entry.project_key,
                    entry.raw_issue_key,
                    entry.activity,
                )
            self._match_activity(entry)

    def _match_activity(self, entry: WorklogEntry) -> None:
        names = {a["name"] for a in self._project_activities(entry.target_project)}
        entry.target_activity = best_match(
            names, entry.issue_key, entry.raw_issue_key, entry.activity
        )

    def retarget(self, entry: WorklogEntry, field: TargetField, value: str) -> None:
        super().retarget(entry, field, value)
        if field == PROJECT_FIELD:
            self._match_activity(entry)

    def _activity_id(self, entry: WorklogEntry) -> int | None:
        for activity in self._project_activities(entry.target_project):
            if activity["name"] == entry.target_activity:
                return activity["id"]
        return None

    def _convert_timesheet(self, timesheet: dict) -> WorklogEntry:
        return WorklogEntry(
            activity="",
            started=date_parser.parse(timesheet["begin"]),
            duration=timesheet.get("duration") or 0,
            description=timesheet.get("description") or "",
            timezone="UTC",
        )

    def preload(self, entries: list[WorklogEntry]) -> None:
        if not self.prevent_duplicates or not entries:
            return

        starts = [entry.normalized_start_utc() for entry in entries]
        longest = timedelta(seconds=max(entry.duration for entry in entries))
        begin = min(starts).astimezone(self.timezone)
        end = (max(starts) + longest).astimezone(self.timezone)
        try:
            self._timesheet_cache = [
                self._convert_timesheet(t) for t in self.api.timesheets(begin, end)
            ]
        except KimaiError as e:
            logger.warning(f"Could not preload Kimai timesheets: {e}")
            self._timesheet_cache = []

    def _find_duplicate(self, entry: WorklogEntry) -> WorklogEntry | None:
        for cached in self._timesheet_cache:
            if entry == cached:
                return cached
        return None

    def create_worklog(self, entry: WorklogEntry) -> ProcessingResult:
        project_id = self._project_ids.get(entry.target_project)
        if not project_id:
            return ProcessingResult(False, entry, ValueError("Missing project"))

        activity_id = self._activity_id(entry)
        if not activity_id:
            return ProcessingResult(False, entry, ValueError("Missing activity"))

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

            begin = entry.normalized_start_utc().astimezone(self.timezone)
            self.api.create_timesheet(
                {
                    "begin": begin.strftime("%Y-%m-%dT%H:%M:%S"),
                    "end": (begin + timedelta(seconds=entry.duration)).strftime(
                        "%Y-%m-%dT%H:%M:%S"
                    ),
                    "project": project_id,
                    "activity": activity_id,
                    "description": entry.description or "",
                }
            )

            if self.prevent_duplicates:
                self._timesheet_cache.append(entry)

            return ProcessingResult(True, entry)
        except KimaiError as e:
            return ProcessingResult(False, entry, e)
