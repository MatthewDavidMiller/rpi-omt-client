"""Shared pytest configuration for the factory-based OMT Web GUI."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "app"))

from omt_client import create_app  # noqa: E402
from omt_client.preview import preview_services  # noqa: E402
from settings import load_settings  # noqa: E402

TEST_PASSWORD = "test-password-pytest"


@pytest.fixture
def app(tmp_path):
    application = create_app(
        load_settings(
            {
                "OMT_CONFIG_DIR": str(tmp_path),
                "OMT_PROJECT_LICENSE_FILE": str(REPO_ROOT / "LICENSE"),
                "OMT_THIRD_PARTY_NOTICES_FILE": str(
                    REPO_ROOT / "THIRD_PARTY_NOTICES.txt"
                ),
            }
        ),
        preview_services(TEST_PASSWORD),
    )
    application.config.update(
        TESTING=True,
        WTF_CSRF_ENABLED=False,
        SESSION_COOKIE_SECURE=False,
    )
    return application


@pytest.fixture
def client(app):
    return app.test_client()


@pytest.fixture
def authenticated_client(client):
    response = client.post("/login", data={"password": TEST_PASSWORD})
    assert response.status_code == 302
    return client
