import logging
from abc import ABC, abstractmethod
from dataclasses import dataclass

from autolog.models import ProcessingResult, WorklogEntry

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class CredentialField:
    key: str
    label: str
    secret: bool = False


@dataclass(frozen=True)
class TargetField:
    """An entry attribute the user may pick per row, e.g. a Kimai project."""

    attr: str
    label: str


class ConsumerBase(ABC):
    name: str
    credential_fields: tuple[CredentialField, ...] = ()
    target_fields: tuple[TargetField, ...] = ()

    def __init__(self, credentials: dict[str, str], prevent_duplicates: bool = True):
        self.credentials = credentials
        self.prevent_duplicates = prevent_duplicates

    @abstractmethod
    def connect(self) -> None: ...

    @abstractmethod
    def create_worklog(self, entry: WorklogEntry) -> ProcessingResult: ...

    def assign_targets(self, entries: list[WorklogEntry]) -> None:  # noqa: B027
        """Fill in each entry's target fields, guessing from its source data."""

    def choices(self, field: TargetField, entry: WorklogEntry) -> list[str]:
        return []

    def retarget(self, entry: WorklogEntry, field: TargetField, value: str) -> None:
        setattr(entry, field.attr, value)

    def preload(self, entries: list[WorklogEntry]) -> None:  # noqa: B027
        """Warm whatever cache duplicate detection needs."""

    def format_error(self, error: Exception) -> str:
        return str(error)
