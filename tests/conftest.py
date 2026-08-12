"""Shared repository-test configuration."""

from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

_EXCUSED_CASES: list[str] = []


def pytest_runtest_logreport(report) -> None:
    if report.skipped:
        reason = report.longrepr[2] if isinstance(report.longrepr, tuple) else "skipped"
        _EXCUSED_CASES.append(f"{report.nodeid} ({reason})")


def pytest_deselected(items) -> None:
    _EXCUSED_CASES.extend(f"{item.nodeid} (deselected)" for item in items)


def pytest_sessionfinish(session, exitstatus) -> None:  # noqa: ARG001
    if not _EXCUSED_CASES:
        return
    print(f"\nERROR: every case must run, but {len(_EXCUSED_CASES)} did not execute:")
    for case in _EXCUSED_CASES:
        print(f"  {case}")
    session.exitstatus = 1
