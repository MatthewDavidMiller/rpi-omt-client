"""Request-correlated diagnostics and bounded support archives."""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import stat
import tempfile
import time
import zipfile
from dataclasses import replace
from datetime import UTC, datetime
from pathlib import Path
from typing import IO

from ..discovery import is_valid_direct_target, parse_omt_sources
from ..models import CommandResult, DiagnosticResult
from ..records import parse_key_value_record
from ..safe_io import read_bytes, read_text, write_fixed_inode
from ..settings import AppSettings
from .command import run_command
from .protocols import SourcePlaybackService

HOST_REPORT_LIMIT = 16 * 1024 * 1024
PCAP_METADATA_LIMIT = 64 * 1024
PCAP_MAX_BYTES = 64 * 1024 * 1024
DIAGNOSTIC_REQUEST_LIMIT = 512
RAW_CAPTURE_STATUSES = frozenset({"complete", "time_limit", "size_limit"})
PCAP_MAGIC = frozenset(
    {
        bytes.fromhex("d4c3b2a1"),
        bytes.fromhex("a1b2c3d4"),
        bytes.fromhex("4d3cb2a1"),
        bytes.fromhex("a1b23c4d"),
        bytes.fromhex("0a0d0d0a"),
    }
)


def _unavailable(detail: str) -> bytes:
    return f"unavailable: {detail}\n".encode("utf-8", "replace")


def _parse_header(value: str, required: set[str]) -> dict[str, str] | None:
    """Read the leading header of a host report, ignoring its payload."""
    return parse_key_value_record(value, required, allow_body=True)


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


LEGAL_FILE_LIMIT = 2 * 1024 * 1024


class RuntimeDiagnostics:
    def __init__(self, settings: AppSettings, source: SourcePlaybackService) -> None:
        self._settings = settings
        self._source = source

    def version(self) -> str:
        result = read_text(self._settings.version_file, 256)
        return result.text.strip() if result.ok and result.text.strip() else "unknown"

    def legal_texts(self) -> tuple[str, str]:
        """Return the project license and third-party notices for the About page."""
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

    def status(self) -> str:
        result = run_command(
            [self._settings.control_command, "status"],
            self._settings.control_timeout_seconds,
        )
        return result.report_text

    def discovery(self) -> DiagnosticResult:
        return self._discovery(time.monotonic() + 5)

    def _discovery(self, deadline: float) -> DiagnosticResult:
        result = _run_before_deadline(
            [
                self._settings.receiver_command,
                "discover",
                "--wait-ms",
                "3000",
                "--json",
            ],
            5,
            deadline,
        )
        enriched = replace(
            result,
            sources=tuple(parse_omt_sources(result.stdout)),
        )
        return DiagnosticResult("OMT discovery check", enriched)

    def runtime(self) -> DiagnosticResult:
        return self._runtime(time.monotonic() + 6)

    def _runtime(self, deadline: float) -> DiagnosticResult:
        results = [
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
        ]
        output = "\n\n".join(
            f"$ {result.command}\n{result.stdout}{result.stderr}{result.error}"
            for result in results
        )
        ok = all(result.returncode in (0, 3) for result in results)
        return DiagnosticResult(
            "Runtime check",
            CommandResult(
                command="OMT runtime checks",
                returncode=0 if ok else 1,
                stdout=output,
                error="" if ok else "One or more runtime checks failed.",
                duration_seconds=sum(result.duration_seconds for result in results),
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

    def _request_host_report(self, request_id: str, include_pcap: bool) -> str:
        record = (
            "version=1\n"
            f"request_id={request_id}\n"
            f"capture_pcap={int(include_pcap)}\n"
            f"requested_at_epoch={int(time.time())}\n"
        ).encode("ascii")
        try:
            write_fixed_inode(
                self._settings.diagnostics_host_request_file,
                record,
                DIAGNOSTIC_REQUEST_LIMIT,
            )
        except OSError as exc:
            return f"unable to submit host diagnostic request: {exc}"
        return ""

    def _fresh_host_report(
        self,
        request_id: str,
        deadline: float,
    ) -> tuple[bytes, str]:
        timeout_deadline = min(
            deadline,
            time.monotonic() + self._settings.diagnostics_host_timeout_seconds,
        )
        last_detail = "host diagnostic report was not published"
        while time.monotonic() < timeout_deadline:
            result = read_text(
                self._settings.diagnostics_host_report_file,
                HOST_REPORT_LIMIT,
            )
            if result.ok:
                header = _parse_header(
                    result.text,
                    {"version", "request_id", "status"},
                )
                if (
                    header is not None
                    and header["version"] == "1"
                    and header["request_id"] == request_id
                    and header["status"] in {"complete", "partial"}
                ):
                    return result.data, ""
                last_detail = "host diagnostic report did not match this request"
            elif result.detail:
                last_detail = result.detail
            time.sleep(0.05)
        return b"", last_detail

    def _capture_metadata(
        self,
        request_id: str,
    ) -> tuple[bytes, dict[str, str] | None, str]:
        result = read_text(
            self._settings.diagnostics_host_pcap_metadata_file,
            PCAP_METADATA_LIMIT,
        )
        if not result.ok:
            return b"", None, result.detail or result.status.value
        required = {
            "version",
            "request_id",
            "capture_status",
            "capture_interface",
            "capture_filter",
            "capture_snaplen",
            "capture_seconds",
            "max_bytes",
            "size_bytes",
            "sha256",
            "pcap_magic",
            "tcpdump_exit_status",
        }
        fields = _parse_header(result.text, required)
        if fields is None:
            return result.data, None, "capture metadata schema is invalid"
        if fields["version"] != "1" or fields["request_id"] != request_id:
            return result.data, None, "capture metadata does not match this request"
        status_value = fields["capture_status"]
        if status_value not in RAW_CAPTURE_STATUSES | {
            "disabled",
            "unavailable",
            "failed",
            "oversized",
            "invalid",
        }:
            return result.data, None, "capture metadata status is invalid"
        if not re.fullmatch(r"[0-9]+", fields["size_bytes"]):
            return result.data, None, "capture metadata size is invalid"
        size = int(fields["size_bytes"])
        if size > PCAP_MAX_BYTES or fields["max_bytes"] != str(PCAP_MAX_BYTES):
            return result.data, None, "capture metadata size exceeds the limit"
        if not re.fullmatch(r"[0-9a-f]{64}", fields["sha256"]):
            return result.data, None, "capture metadata digest is invalid"
        if status_value in RAW_CAPTURE_STATUSES:
            try:
                magic = bytes.fromhex(fields["pcap_magic"])
            except ValueError:
                magic = b""
            if magic not in PCAP_MAGIC:
                return result.data, None, "capture metadata magic is invalid"
        return result.data, fields, ""

    def _validated_pcap(
        self,
        metadata: dict[str, str],
    ) -> tuple[IO[bytes] | None, str]:
        path = Path(self._settings.diagnostics_host_pcap_file)
        try:
            before = os.lstat(path)
        except OSError as exc:
            return None, f"unable to inspect packet capture: {exc}"
        expected_size = int(metadata["size_bytes"])
        if (
            stat.S_ISLNK(before.st_mode)
            or not stat.S_ISREG(before.st_mode)
            or before.st_size != expected_size
            or before.st_size > PCAP_MAX_BYTES
        ):
            return None, "packet capture is unsafe or has an unexpected size"
        flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
        descriptor = -1
        spool = tempfile.SpooledTemporaryFile(max_size=2 * 1024 * 1024)
        digest = hashlib.sha256()
        prefix = b""
        succeeded = False
        try:
            descriptor = os.open(path, flags)
            opened = os.fstat(descriptor)
            if not stat.S_ISREG(opened.st_mode) or (opened.st_dev, opened.st_ino) != (
                before.st_dev,
                before.st_ino,
            ):
                return None, "packet capture changed while opening"
            remaining = expected_size
            while remaining:
                chunk = os.read(descriptor, min(1024 * 1024, remaining))
                if not chunk:
                    return None, "packet capture ended before its declared size"
                if len(prefix) < 4:
                    prefix += chunk[: 4 - len(prefix)]
                spool.write(chunk)
                digest.update(chunk)
                remaining -= len(chunk)
            if os.read(descriptor, 1):
                return None, "packet capture exceeds its declared size"
            after_fd = os.fstat(descriptor)
            after_path = os.lstat(path)
            if (
                (after_fd.st_dev, after_fd.st_ino) != (before.st_dev, before.st_ino)
                or (after_path.st_dev, after_path.st_ino) != (before.st_dev, before.st_ino)
                or after_path.st_size != expected_size
            ):
                return None, "packet capture changed while being read"
            if prefix not in PCAP_MAGIC:
                return None, "packet capture magic is invalid"
            if digest.hexdigest() != metadata["sha256"]:
                return None, "packet capture SHA-256 does not match metadata"
            spool.seek(0)
            succeeded = True
            return spool, ""
        except OSError as exc:
            return None, f"unable to read packet capture: {exc}"
        finally:
            if descriptor >= 0:
                os.close(descriptor)
            if not succeeded:
                spool.close()

    def bundle(self, include_packet_capture: bool = False) -> tuple[IO[bytes], str]:
        started = time.monotonic()
        deadline = started + self._settings.diagnostics_bundle_budget_seconds
        request_id = os.urandom(16).hex()
        host_request_error = self._request_host_report(
            request_id,
            include_packet_capture,
        )

        runtime = self._runtime(deadline).command.stdout
        discovery_result = self._discovery(deadline).command
        discovery = discovery_result.stdout or discovery_result.error
        status_result = _run_before_deadline(
            [self._settings.control_command, "status"],
            self._settings.control_timeout_seconds,
            deadline,
        )
        controller_status = status_result.report_text + "\n"
        source, _address = self._source.configuration()
        if self._settings.diagnostics_receive_probe and source:
            remaining = max(0.0, deadline - time.monotonic())
            probe = _run_before_deadline(
                [
                    self._settings.receiver_command,
                    "probe",
                    "--target",
                    source,
                    "--timeout-ms",
                    str(max(1, min(3000, int(remaining * 1000)))),
                    "--json",
                ],
                min(5, remaining),
                deadline,
            )
            receive_probe = probe.stdout or probe.error or probe.stderr
        else:
            receive_probe = "skipped: no current target or receive probe disabled\n"

        host_report = b""
        host_report_error = host_request_error
        if not host_request_error:
            host_report, host_report_error = self._fresh_host_report(
                request_id,
                deadline,
            )
        metadata_data, metadata, metadata_error = self._capture_metadata(request_id)

        packet_spool: IO[bytes] | None = None
        packet_error = ""
        if include_packet_capture:
            if metadata is None:
                packet_error = metadata_error
            elif metadata["capture_status"] not in RAW_CAPTURE_STATUSES:
                packet_error = "host packet capture status is " + metadata["capture_status"]
            else:
                packet_spool, packet_error = self._validated_pcap(metadata)

        bundle = tempfile.SpooledTemporaryFile(max_size=4 * 1024 * 1024)
        timestamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
        with zipfile.ZipFile(bundle, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("version.txt", self.version() + "\n")
            archive.writestr(
                "runtime-settings.txt",
                "\n".join(self._settings.diagnostic_lines()) + "\n",
            )
            archive.writestr("runtime.txt", runtime)
            archive.writestr("discovery.json", discovery)
            archive.writestr("controller-status.txt", controller_status)
            archive.writestr("current-target-receive-probe.json", receive_probe)
            for name, path, limit in (
                ("playback-status.json", self._settings.playback_status_file, 4096),
                ("omt-settings.xml", self._settings.runtime_config_file, 65536),
                (
                    "runtime-sha256.manifest",
                    self._settings.runtime_integrity_manifest,
                    262144,
                ),
            ):
                result = read_bytes(path, limit)
                archive.writestr(
                    name,
                    result.data
                    if result.ok
                    else _unavailable(result.detail or result.status.value),
                )
            archive.writestr(
                "host-report.txt",
                host_report if host_report else _unavailable(host_report_error),
            )
            archive.writestr(
                "host-network-pcap.txt",
                # Only embed raw metadata when it validated for this request.
                # Truthy stale bytes from a prior capture must not win over the
                # typed error explaining why this run rejected them.
                metadata_data if metadata is not None else _unavailable(metadata_error),
            )
            if include_packet_capture:
                if packet_spool is None:
                    archive.writestr(
                        "host-network.pcap.unavailable.txt",
                        _unavailable(packet_error),
                    )
                else:
                    with archive.open("host-network.pcap", "w") as destination:
                        shutil.copyfileobj(packet_spool, destination, 1024 * 1024)
                    packet_spool.close()
        bundle.seek(0)
        return bundle, f"omt-diagnostics-{timestamp}.zip"
