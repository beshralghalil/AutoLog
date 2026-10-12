"""Handles secure credential storage"""

import keyring

_LEGACY_JIRA_KEYS = ("base_url", "email", "api_key")


class CredentialManager:
    SERVICE_NAME = "jira_worklog_app"

    @classmethod
    def save_credentials(cls, consumer: str, credentials: dict[str, str]) -> None:
        for key, value in credentials.items():
            keyring.set_password(cls.SERVICE_NAME, f"{consumer}.{key}", value)

    @classmethod
    def get_credentials(cls, consumer: str, keys: tuple[str, ...]) -> dict[str, str]:
        stored = {
            key: keyring.get_password(cls.SERVICE_NAME, f"{consumer}.{key}") or ""
            for key in keys
        }
        if consumer == "jira" and not any(stored.values()):
            stored.update(cls._legacy_jira_credentials())
        return stored

    @classmethod
    def _legacy_jira_credentials(cls) -> dict[str, str]:
        """Credentials saved before consumers were namespaced."""
        return {
            key: keyring.get_password(cls.SERVICE_NAME, key) or ""
            for key in _LEGACY_JIRA_KEYS
        }
