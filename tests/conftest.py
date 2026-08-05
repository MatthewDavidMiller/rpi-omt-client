"""Shared pytest configuration and helpers for the factory-based OMT Web GUI."""

from __future__ import annotations

import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any

from omt_client import create_app
from omt_client.services import ServiceContainer
from omt_client.settings import AppSettings, load_settings

REPO_ROOT = Path(__file__).resolve().parent.parent


# Cases that did not run: skipped, expected-failure, or deselected.
_EXCUSED_CASES: list[str] = []


def pytest_runtest_logreport(report) -> None:
    """Record any case that reported an outcome other than run-and-judged."""
    if report.skipped:
        reason = report.longrepr[2] if isinstance(report.longrepr, tuple) else "skipped"
        _EXCUSED_CASES.append(f"{report.nodeid} ({reason})")


def pytest_deselected(items) -> None:
    _EXCUSED_CASES.extend(f"{item.nodeid} (deselected)" for item in items)


def pytest_sessionfinish(session, exitstatus) -> None:  # noqa: ARG001
    """Fail the run if any case opted out of executing.

    Every gate here runs on one provisioned workstation, so a case that excuses
    itself is reporting a broken environment rather than an inapplicable test.
    Without this, `make install` drift reads as a green run over a quietly
    smaller suite.
    """
    if not _EXCUSED_CASES:
        return
    print(
        f"\nERROR: every case must run on every commit, but {len(_EXCUSED_CASES)} did not execute:"
    )
    for case in _EXCUSED_CASES:
        print(f"  {case}")
    session.exitstatus = 1


TESTING_CONFIG = {
    "TESTING": True,
    "WTF_CSRF_ENABLED": False,
    "SESSION_COOKIE_SECURE": False,
}


class VirtualClock:
    """A monotonic clock that advances only when the code under test waits.

    Deadline and budget assertions must describe what the code does with its
    own arithmetic, not how fast this host happens to fsync. Replace a module's
    `time` with `clock.module()` and every `monotonic`/`sleep` call in it
    becomes deterministic and instant.
    """

    def __init__(self, start: float = 1_000.0) -> None:
        self.now = start
        self.slept: list[float] = []

    def monotonic(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.slept.append(seconds)
        self.now += seconds

    def module(self) -> SimpleNamespace:
        """Return a stand-in for the `time` module bound to this clock."""
        return SimpleNamespace(monotonic=self.monotonic, sleep=self.sleep, time=time.time)


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
