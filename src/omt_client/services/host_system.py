"""Validated request bridge for privileged host actions."""

from __future__ import annotations

import secrets
import threading
import time

from ..models import ActionResult
from ..records import parse_key_value_record
from ..safe_io import read_text, write_fixed_inode
from ..settings import AppSettings

REBOOT_RECORD_LIMIT = 512


class HostSystem:
    def __init__(self, settings: AppSettings) -> None:
        self._settings = settings
        self._lock = threading.Lock()

    @staticmethod
    def _write_request(path: str, value: bytes) -> None:
        try:
            write_fixed_inode(path, value, REBOOT_RECORD_LIMIT)
        except OSError as exc:
            message = str(exc).replace("request file", "host reboot request file")
            raise OSError(message) from exc

    @staticmethod
    def _result_for(path: str, request_id: str) -> tuple[str, str] | None:
        result = read_text(path, REBOOT_RECORD_LIMIT)
        if not result.ok:
            return None
        # No `allow_body`: the reboot result is exactly these four lines, so
        # anything trailing is a rejection rather than an ignored payload.
        fields = parse_key_value_record(
            result.text,
            {"version", "request_id", "status", "detail"},
        )
        if fields is None:
            return None
        if fields["version"] != "1" or fields["request_id"] != request_id:
            return None
        if fields["status"] not in {"accepted", "rejected"}:
            return None
        return fields["status"], fields["detail"]

    def request_reboot(self) -> ActionResult:
        with self._lock:
            request_id = secrets.token_hex(16)
            record = (
                "version=1\n"
                "action=reboot\n"
                f"request_id={request_id}\n"
                f"requested_at_epoch={int(time.time())}\n"
            ).encode("ascii")
            try:
                self._write_request(self._settings.reboot_request_file, record)
            except OSError as exc:
                return ActionResult(False, error=f"Unable to submit the host reboot request: {exc}")
            deadline = time.monotonic() + self._settings.reboot_ack_timeout_seconds
            while time.monotonic() < deadline:
                acknowledged = self._result_for(self._settings.reboot_result_file, request_id)
                if acknowledged is not None:
                    status, detail = acknowledged
                    if status == "accepted":
                        return ActionResult(
                            True,
                            message=(
                                "OS reboot scheduled. This appliance will go offline shortly."
                            ),
                        )
                    return ActionResult(
                        False,
                        error=f"The host rejected the reboot request: {detail}",
                    )
                time.sleep(0.05)
            return ActionResult(
                False,
                error=(
                    "The reboot request was submitted but the host did not acknowledge it. "
                    "Check rc-service omt-client-reboot status and /var/log/messages "
                    "before retrying."
                ),
            )
