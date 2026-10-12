from .base import ConsumerBase
from .jira import JiraConsumer
from .kimai import KimaiConsumer

_CONSUMERS = {
    "jira": JiraConsumer,
    "kimai": KimaiConsumer,
}


def select_consumer(
    name: str, credentials: dict[str, str], prevent_duplicates: bool = True
) -> ConsumerBase:
    cls = get_consumer_class(name)
    return cls(credentials, prevent_duplicates)


def get_consumer_class(name: str) -> type[ConsumerBase]:
    cls = _CONSUMERS.get(name.lower())
    if not cls:
        raise ValueError(
            f"Unknown consumer '{name}'. Available: {', '.join(_CONSUMERS)}"
        )
    return cls


def get_consumers_names() -> list[str]:
    return list(_CONSUMERS.keys())
