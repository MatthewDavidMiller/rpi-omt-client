"""Shared pytest configuration for the factory-based OMT Web GUI."""

from __future__ import annotations

import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# The application is imported from src/ without installing the package, so both
# omt_client and its dev-only sibling omt_client_preview resolve here.
sys.path.insert(0, str(REPO_ROOT / "src"))
