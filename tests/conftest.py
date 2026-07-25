"""Shared pytest configuration and helpers for the factory-based OMT Web GUI."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from omt_client import create_app
from omt_client.services import ServiceContainer
from omt_client.settings import AppSettings, load_settings

REPO_ROOT = Path(__file__).resolve().parent.parent

TESTING_CONFIG = {
    "TESTING": True,
    "WTF_CSRF_ENABLED": False,
    "SESSION_COOKIE_SECURE": False,
}


def raises(error: BaseException):
    """Return a callable that raises `error`, for monkeypatching failure paths."""

    def raiser(*_args: Any, **_kwargs: Any):
        raise error

    return raiser


def build_app(
    settings: AppSettings | None = None,
    services: ServiceContainer | None = None,
    **overrides: Any,
):
    """Build an app carrying the suite-wide testing configuration."""
    application = create_app(load_settings({}) if settings is None else settings, services)
    application.config.update(TESTING_CONFIG | overrides)
    return application


def signed_in(application, password: str):
    """Return a test client that has completed the login round trip."""
    client = application.test_client()
    assert client.post("/login", data={"password": password}).status_code == 302
    return client
