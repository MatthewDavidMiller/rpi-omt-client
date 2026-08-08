"""Build version and legal texts for the About page."""

from __future__ import annotations

from ..safe_io import read_text
from ..settings import AppSettings

LEGAL_FILE_LIMIT = 2 * 1024 * 1024


class RuntimeAbout:
    """Presentation-neutral product metadata, separate from diagnostics checks."""

    def __init__(self, settings: AppSettings) -> None:
        # Version and legal texts are immutable in the image, so read once.
        version = read_text(settings.version_file, 256)
        self._version = version.text.strip() if version.ok and version.text.strip() else "unknown"
        self._legal_texts = (
            self._legal_text(settings.project_license_file, "Project license"),
            self._legal_text(settings.third_party_notices_file, "Third-party notices"),
        )

    def version(self) -> str:
        return self._version

    def legal_texts(self) -> tuple[str, str]:
        """Return the project license and third-party notices."""
        return self._legal_texts

    @staticmethod
    def _legal_text(path: str, label: str) -> str:
        result = read_text(path, LEGAL_FILE_LIMIT)
        if result.ok:
            return result.text
        return f"{label} is unavailable in this image."
