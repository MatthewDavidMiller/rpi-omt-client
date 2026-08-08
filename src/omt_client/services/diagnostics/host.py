"""The correlated request channel to the privileged host collector.

Everything here crosses the container/host trust boundary. The container writes
a versioned nonce to a pre-created fixed-inode request file, then accepts only
an answer carrying that nonce: a report left behind by an earlier run, or a
packet capture whose metadata describes different bytes, is a rejection with a
stated reason rather than a member the operator would read as current.
"""

from __future__ import annotations

import hashlib
import os
import re
import stat
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import IO

from ...records import parse_key_value_record
from ...safe_io import file_snapshot, read_text, write_fixed_inode
from ...settings import AppSettings

HOST_REPORT_LIMIT = 16 * 1024 * 1024
PCAP_METADATA_LIMIT = 64 * 1024
PCAP_MAX_BYTES = 64 * 1024 * 1024
DIAGNOSTIC_REQUEST_LIMIT = 512
RAW_CAPTURE_STATUSES = frozenset({"complete", "time_limit", "size_limit"})
CAPTURE_STATUSES = RAW_CAPTURE_STATUSES | {
    "disabled",
    "unavailable",
    "failed",
    "oversized",
    "invalid",
}
CAPTURE_METADATA_FIELDS = {
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
PCAP_MAGIC = frozenset(
    {
        bytes.fromhex("d4c3b2a1"),
        bytes.fromhex("a1b2c3d4"),
        bytes.fromhex("4d3cb2a1"),
        bytes.fromhex("a1b23c4d"),
        bytes.fromhex("0a0d0d0a"),
    }
)
# How often the report file is re-examined while the host budget runs.
POLL_INTERVAL_SECONDS = 0.05


def unavailable(detail: str) -> bytes:
    return f"unavailable: {detail}\n".encode("utf-8", "replace")


def _parse_header(value: str, required: set[str]) -> dict[str, str] | None:
    """Read the leading header of a host report, ignoring its payload."""
    return parse_key_value_record(value, required, allow_body=True)


@dataclass(frozen=True)
class HostReport:
    """The privileged collector's answer to one correlated request.

    `capture_requested` is carried rather than inferred: an absent capture that
    nobody asked for writes no member at all, while one that was asked for owes
    the operator `packet_capture_error` as its stated reason.
    """

    report: bytes
    capture_metadata: bytes
    packet_capture: IO[bytes] | None
    packet_capture_error: str
    capture_requested: bool

    def close(self) -> None:
        if self.packet_capture is not None:
            self.packet_capture.close()


class HostCollector:
    """Submits one diagnostic request and reads back only its own answer."""

    def __init__(self, settings: AppSettings) -> None:
        self._settings = settings

    def submit(self, request_id: str, include_pcap: bool) -> str:
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

    def _fresh_report(self, request_id: str, deadline: float) -> tuple[bytes, str]:
        timeout_deadline = min(
            deadline,
            time.monotonic() + self._settings.diagnostics_host_timeout_seconds,
        )
        last_detail = "host diagnostic report was not published"
        # The report is published atomically and can reach 16 MiB, while this
        # loop polls at 20 Hz for as long as the host budget allows. Re-reading
        # and re-decoding an unchanged file hundreds of times can only produce
        # the answer it already produced, so the poll is on the inode.
        observed: tuple[int, int, int, int, int] | None = None
        while time.monotonic() < timeout_deadline:
            snapshot = file_snapshot(self._settings.diagnostics_host_report_file)
            if snapshot is not None and snapshot == observed:
                time.sleep(POLL_INTERVAL_SECONDS)
                continue
            observed = snapshot
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
            else:
                # Same fallback as `_capture_metadata`: a typed read failure
                # always names itself, and the status value is the answer when
                # it does not.
                last_detail = result.detail or result.status.value
            time.sleep(POLL_INTERVAL_SECONDS)
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
        fields = _parse_header(result.text, CAPTURE_METADATA_FIELDS)
        if fields is None:
            return result.data, None, "capture metadata schema is invalid"
        if fields["version"] != "1" or fields["request_id"] != request_id:
            return result.data, None, "capture metadata does not match this request"
        status_value = fields["capture_status"]
        if status_value not in CAPTURE_STATUSES:
            return result.data, None, "capture metadata status is invalid"
        # The hard ceiling is eight decimal digits. Bounding the text before
        # int() also avoids Python's deliberate huge-integer conversion error
        # turning malformed host metadata into an unhandled bundle failure.
        if not re.fullmatch(r"[0-9]{1,8}", fields["size_bytes"]):
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

    def collect(
        self,
        request_id: str,
        deadline: float,
        capture_requested: bool,
        request_error: str,
    ) -> HostReport:
        """Wait for the privileged collector's answer to exactly this request."""
        report = b""
        report_error = request_error
        if not request_error:
            report, report_error = self._fresh_report(request_id, deadline)
        metadata_data, metadata, metadata_error = self._capture_metadata(request_id)

        packet_spool: IO[bytes] | None = None
        packet_error = ""
        if capture_requested:
            if metadata is None:
                packet_error = metadata_error
            elif metadata["capture_status"] not in RAW_CAPTURE_STATUSES:
                packet_error = "host packet capture status is " + metadata["capture_status"]
            else:
                packet_spool, packet_error = self._validated_pcap(metadata)
        return HostReport(
            report=report or unavailable(report_error),
            # Only embed raw metadata when it validated for this request. Truthy
            # stale bytes from a prior capture must not win over the typed error
            # explaining why this run rejected them.
            capture_metadata=(
                metadata_data if metadata is not None else unavailable(metadata_error)
            ),
            packet_capture=packet_spool,
            packet_capture_error=packet_error,
            capture_requested=capture_requested,
        )
