from pathlib import Path

from .base import ProviderBase
from .grindstone import GrindstoneProvider
from .kimai import KimaiProvider
from .odoo import OdooProvider

_PROVIDERS = {
    "kimai": KimaiProvider,
    "odoo": OdooProvider,
    "grindstone": GrindstoneProvider,
}


def select_provider(name: str, file_path: Path) -> ProviderBase:
    name = name.lower()
    cls = _PROVIDERS.get(name)
    if not cls:
        raise ValueError(
            f"Unknown provider '{name}'. Available: {', '.join(_PROVIDERS)}"
        )
    return cls(file_path)


def get_providers_names() -> list[str]:
    return list(_PROVIDERS.keys())
