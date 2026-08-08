#!/usr/bin/env python3
"""Run the real Flask factory and templates with side-effect-free services."""

from __future__ import annotations

import os
import sys
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PROJECT_ROOT / "src"))

from omt_client import create_app  # noqa: E402
from omt_client.settings import load_settings  # noqa: E402
from omt_client_preview import preview_services  # noqa: E402

PASSWORD = os.environ.get("WEB_PASSWORD", "omt-client")
app = create_app(
    load_settings(
        {
            "OMT_PROJECT_LICENSE_FILE": str(PROJECT_ROOT / "LICENSE"),
            "OMT_THIRD_PARTY_NOTICES_FILE": str(PROJECT_ROOT / "THIRD_PARTY_NOTICES.txt"),
        }
    ),
    preview_services(PASSWORD),
)
app.config.update(SESSION_COOKIE_SECURE=False)


if __name__ == "__main__":
    print("OMT Client production UI preview")
    print("Open http://localhost:5000")
    print(f"Password: {PASSWORD}")
    print("All appliance services are replaced by in-memory fakes.")
    debug = os.environ.get("OMT_PREVIEW_DEBUG") == "1"
    app.run(host="127.0.0.1", port=5000, debug=debug)
