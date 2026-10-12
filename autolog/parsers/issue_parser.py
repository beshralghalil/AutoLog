"""Issue key parsing"""

import re


class IssueKeyParser:
    _patterns = (
        re.compile(r"\[([A-Za-z0-9]+)\s*-\s*(\d+)\]"),  # [ABC123-456]
        re.compile(r"\[([A-Za-z0-9]+)\]\s*(\d+)"),  # [ABC123] 456
        re.compile(r"\b([A-Za-z0-9]+)\s*-\s*(\d+)\b"),  # ABC123-456
    )

    @staticmethod
    def parse(activity: str) -> str | None:
        for pat in IssueKeyParser._patterns:
            m = pat.search(activity)
            if m:
                # group(1) is project (letters+digits), group(2) is number
                return f"{m.group(1)}-{m.group(2)}"
        return None

    @staticmethod
    def parse_project(activity: str) -> str | None:
        key = IssueKeyParser.parse(activity)
        return key.split("-")[0] if key else None
