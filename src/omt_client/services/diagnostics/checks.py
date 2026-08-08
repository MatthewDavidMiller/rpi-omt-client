"""The checks the container can answer on its own.

Everything here runs inside the container: the receiver's own subcommands and
the playback controller. Nothing in this module crosses the host boundary --
that is `host.py` -- and nothing lays out an archive -- that is `bundle.py`.
"""

from __future__ import annotations

import json
import time
from dataclasses import dataclass, replace

from ...discovery import is_valid_direct_target, parse_omt_sources
from ...json_document import JsonDocumentError, load_json_document
from ...models import CommandResult, DiagnosticResult
from ...settings import AppSettings
from ..command import run_command
from ..protocols import SourcePlaybackService


def _run_before_deadline(
    command: list[str],
    maximum_seconds: float,
    deadline: float,
) -> CommandResult:
    remaining = min(maximum_seconds, deadline - time.monotonic())
    if remaining <= 0:
        return CommandResult(
            command=" ".join(command),
            error="Skipped: diagnostic bundle budget exhausted.",
            skipped=True,
        )
    return run_command(command, max(0.001, remaining))


def _runtime_report(version: CommandResult, status: CommandResult) -> DiagnosticResult:
    """Compose the operator-facing runtime check from its two command results."""
    output = "\n\n".join(
        f"$ {result.command}\n{result.stdout}{result.stderr}{result.error}"
        for result in (version, status)
    )
    # `--version` has one successful exit code. Controller status also uses
    # 3 for the valid "not running" state; applying that allowance to both
    # commands would report a broken receiver version check as healthy.
    ok = version.returncode == 0 and status.returncode in (0, 3)
    return DiagnosticResult(
        "Runtime check",
        CommandResult(
            command="OMT runtime checks",
            returncode=0 if ok else 1,
            stdout=output,
            error="" if ok else "One or more runtime checks failed.",
            duration_seconds=version.duration_seconds + status.duration_seconds,
        ),
    )


def _json_member(text: str, expected: type[list[object]] | type[dict[str, object]]) -> str:
    """Return `text` as a compact JSON member of the declared shape, or an error.

    Every archive member has to be parseable JSON even when the command that
    produced it failed or a damaged receiver printed something else entirely,
    so anything that is not a document of `expected` becomes a stated
    `{"ok": false, "error": ...}` rather than raw output the reader must guess
    at. The discovery and receive-probe members differ only in that shape and
    in the sentence they fall back to, which is the caller's to supply.
    """
    try:
        document = load_json_document(text)
    except JsonDocumentError:
        document = None
    if isinstance(document, expected):
        return _compact(document)
    return ""


def _compact(document: object) -> str:
    return json.dumps(document, ensure_ascii=False, separators=(",", ":"))


def _error_member(detail: str, fallback: str) -> str:
    return _compact({"ok": False, "error": detail.strip() or fallback})


def discovery_member(result: CommandResult) -> str:
    """Return a valid JSON member for one receiver discovery result."""
    if result.returncode == 0:
        member = _json_member(result.stdout, list)
        if member:
            return member
        return _error_member("", "Receiver returned invalid discovery JSON.")
    return _error_member(result.failure_detail, "Discovery command failed.")


def receive_probe_member(text: str, *, skipped: bool = False) -> str:
    """Return a valid JSON member for the current-target receive probe."""
    if skipped:
        return _error_member(text, "Receive probe skipped.")
    member = _json_member(text, dict)
    if member:
        return member
    return _error_member(text, "Receiver returned invalid receive-probe JSON.")


@dataclass(frozen=True)
class ContainerReport:
    """The container's own answers, already resolved into archive members."""

    runtime: str
    discovery: str
    controller_status: str
    receive_probe: str


class ContainerChecks:
    """Runs the receiver and controller commands the operator can ask for."""

    def __init__(self, settings: AppSettings, source: SourcePlaybackService) -> None:
        self._settings = settings
        self._source = source

    def status(self) -> str:
        result = run_command(
            [self._settings.control_command, "status"],
            self._settings.control_timeout_seconds,
        )
        return result.report_text

    def discovery(self, deadline: float | None = None) -> DiagnosticResult:
        result = _run_before_deadline(
            [
                self._settings.receiver_command,
                "discover",
                "--wait-ms",
                "3000",
                "--json",
            ],
            5,
            time.monotonic() + 5 if deadline is None else deadline,
        )
        enriched = replace(
            result,
            sources=(tuple(parse_omt_sources(result.stdout)) if result.returncode == 0 else ()),
        )
        return DiagnosticResult("OMT discovery check", enriched)

    def runtime(self, deadline: float | None = None) -> tuple[DiagnosticResult, str]:
        """Return the runtime check and the controller status it observed.

        Both halves come from the one `runtime_checks` call, so the diagnostics
        page's header and the check printed beneath it describe the same
        observation rather than two that are free to disagree.
        """
        version_result, status_result = self.runtime_checks(
            time.monotonic() + 6 if deadline is None else deadline
        )
        return _runtime_report(version_result, status_result), status_result.report_text

    def runtime_checks(self, deadline: float) -> tuple[CommandResult, CommandResult]:
        """Run the receiver version and controller status checks exactly once.

        A support bundle reports both the runtime check and the controller status
        as separate members. Asking the controller twice would spend a second
        flock and /proc walk out of the bundle budget to obtain a *second*
        observation, which is free to disagree with the first -- leaving the
        archive contradicting itself about whether the receiver was running.
        """
        return (
            _run_before_deadline(
                [self._settings.receiver_command, "--version"],
                3,
                deadline,
            ),
            _run_before_deadline(
                [self._settings.control_command, "status"],
                3,
                deadline,
            ),
        )

    def direct(self, address: str) -> DiagnosticResult:
        if not is_valid_direct_target(address):
            return DiagnosticResult(
                "Direct-connect check",
                CommandResult(error="Invalid OMT direct target.", skipped=True),
            )
        return DiagnosticResult(
            "Direct-connect check",
            run_command(
                [
                    self._settings.receiver_command,
                    "probe",
                    "--target",
                    address,
                    "--timeout-ms",
                    "3000",
                    "--json",
                ],
                5,
            ),
        )

    def collect(self, deadline: float) -> ContainerReport:
        """Answer everything the container can see, inside the shared deadline."""
        version_result, status_result = self.runtime_checks(deadline)
        runtime = _runtime_report(version_result, status_result).command.stdout
        discovery_result = self.discovery(deadline).command
        return ContainerReport(
            runtime=runtime,
            # Keep the archive member valid JSON even when the command fails or
            # a damaged receiver emits malformed output.
            discovery=discovery_member(discovery_result),
            controller_status=status_result.report_text + "\n",
            receive_probe=self._receive_probe(deadline),
        )

    def _receive_probe(self, deadline: float) -> str:
        configuration = self._source.configuration()
        if configuration.error:
            return receive_probe_member(f"skipped: {configuration.error}", skipped=True)
        if not (self._settings.diagnostics_receive_probe and configuration.source):
            return receive_probe_member(
                "skipped: no current target or receive probe disabled",
                skipped=True,
            )
        remaining = max(0.0, deadline - time.monotonic())
        probe = _run_before_deadline(
            [
                self._settings.receiver_command,
                "probe",
                "--target",
                configuration.source,
                "--timeout-ms",
                str(max(1, min(3000, int(remaining * 1000)))),
                "--json",
            ],
            min(5, remaining),
            deadline,
        )
        return receive_probe_member(probe.stdout or probe.error or probe.stderr)
