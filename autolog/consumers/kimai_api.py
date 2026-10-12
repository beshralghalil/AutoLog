"""Thin wrapper over the Kimai REST API"""

import logging
from datetime import datetime

import requests

KIMAI_TIMEOUT = 30
PAGE_SIZE = 100
DATETIME_FORMAT = "%Y-%m-%dT%H:%M:%S"

logger = logging.getLogger(__name__)


class KimaiError(Exception):
    pass


def format_datetime(value: datetime) -> str:
    return value.strftime(DATETIME_FORMAT)


class KimaiAPI:
    def __init__(self, base_url: str, api_token: str, timeout: int = KIMAI_TIMEOUT):
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self.session = requests.Session()
        self.session.headers["Authorization"] = f"Bearer {api_token}"

    def _request(self, method: str, path: str, **kwargs) -> requests.Response:
        url = f"{self.base_url}/api/{path.lstrip('/')}"
        try:
            response = self.session.request(method, url, timeout=self.timeout, **kwargs)
        except requests.RequestException as e:
            raise KimaiError(f"Could not reach {url}: {e}") from e

        if not response.ok:
            raise KimaiError(self._error_message(response))
        return response

    @staticmethod
    def _error_message(response: requests.Response) -> str:
        try:
            payload = response.json()
        except ValueError:
            return f"{response.status_code} {response.reason}"

        message = payload.get("message") or payload.get("detail")
        errors = payload.get("errors")
        if isinstance(errors, dict):
            details = "; ".join(
                f"{field}: {', '.join(messages)}"
                for field, messages in errors.get("children", errors).items()
                if messages
            )
            if details:
                message = f"{message or 'Invalid request'} ({details})"
        return message or f"{response.status_code} {response.reason}"

    def _collection(self, path: str, params: dict | None = None) -> list[dict]:
        params = dict(params or {}, size=PAGE_SIZE, page=1)
        items: list[dict] = []
        while True:
            response = self._request("GET", path, params=params)
            items.extend(response.json())
            total_pages = int(response.headers.get("X-Total-Pages") or 1)
            if params["page"] >= total_pages:
                return items
            params["page"] += 1

    def current_user(self) -> dict:
        return self._request("GET", "users/me").json()

    def projects(self) -> list[dict]:
        return self._collection("projects", {"visible": 1})

    def activities(self) -> list[dict]:
        return self._collection("activities", {"visible": 1})

    def timesheets(self, begin: datetime, end: datetime) -> list[dict]:
        return self._collection(
            "timesheets",
            {"begin": format_datetime(begin), "end": format_datetime(end)},
        )

    def create_timesheet(self, payload: dict) -> dict:
        return self._request("POST", "timesheets", json=payload).json()
