import logging
from pathlib import Path
from typing import Callable, Type

import pandas as pd

logger = logging.getLogger(__name__)


class FileParserBase:

    # Subclasses should provide a reader function
    _reader: Callable[[str, Type], str] = None

    @classmethod
    def read(cls, file_path: Path) -> list[dict]:
        """Return raw rows as list of dicts."""
        df = cls._reader(file_path, dtype=str).fillna("")
        df.columns = [str(col).lstrip("\ufeff").strip() for col in df.columns]
        return df.to_dict(orient="records")


class CSVParser(FileParserBase):
    _reader = pd.read_csv


class ExcelParser(FileParserBase):
    _reader = pd.read_excel


# parsers registry
_parsers: dict[str, Type[FileParserBase]] = {
    ".csv": CSVParser,
    ".xls": ExcelParser,
    ".xlsx": ExcelParser,
}


def get_supported_formats() -> list[str]:
    return list(_parsers.keys())


def select_file_parser(file_path: Path) -> FileParserBase:
    suffix = file_path.suffix.lower()
    parser_cls = _parsers.get(suffix)
    if not parser_cls:
        raise ValueError(f"Unsupported file type: {suffix}")
    return parser_cls()
