"""Build version and legal texts for the About page."""

from __future__ import annotations

from ..safe_io import read_text
from ..settings import AppSettings

LEGAL_FILE_LIMIT = 2 * 1024 * 1024


class RuntimeAbout:
    """Presentation-neutral product metadata, separate from diagnostics checks."""

    def __init__(self, settings: AppSettings) -> None:
        self._settings = settings

    def version(self) -> str:
        result = read_text(self._settings.version_file, 256)
        return result.text.strip() if result.ok and result.text.strip() else "unknown"

    def legal_texts(self) -> tuple[str, str]:
        """Return the project license and third-party notices."""
        return (
            self._legal_text(self._settings.project_license_file, "Project license"),
            self._legal_text(
                self._settings.third_party_notices_file,
                "Third-party notices",
            ),
        )

    def _legal_text(self, path: str, label: str) -> str:
        result = read_text(path, LEGAL_FILE_LIMIT)
        if result.ok:
            return result.text
        return f"{label} is unavailable in this image."
