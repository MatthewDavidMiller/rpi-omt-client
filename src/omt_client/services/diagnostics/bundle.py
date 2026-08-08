"""`RuntimeDiagnostics`: the service routes use, and the archive it lays out.

This module composes the two collectors and owns the zip. It deliberately
decides nothing about *what* a member should contain -- `checks.py` and
`host.py` hand it members that are already resolved, including their stated
failure reasons -- so the layout below reads as the archive's contract.
"""

from __future__ import annotations

import os
import shutil
import tempfile
import time
import zipfile
from datetime import UTC, datetime
from typing import IO

from ...models import DiagnosticResult
from ...safe_io import read_bytes
from ...settings import AppSettings
from ..protocols import AboutService, SourcePlaybackService
from .checks import ContainerChecks, ContainerReport
from .host import HostCollector, HostReport, unavailable

# Members copied verbatim from a path, with the ceiling each is read under.
COPIED_MEMBERS = (
    ("playback-status.json", "playback_status_file", 4096),
    ("omt-settings.xml", "runtime_config_file", 65536),
    ("runtime-sha256.manifest", "runtime_integrity_manifest", 262144),
)


class RuntimeDiagnostics:
    def __init__(
        self,
        settings: AppSettings,
        source: SourcePlaybackService,
        about: AboutService,
    ) -> None:
        self._settings = settings
        # The build version has one owner. A support bundle that disagreed with
        # the About page about which build produced it is worse than useless.
        self._about = about
        self._checks = ContainerChecks(settings, source)
        self._host = HostCollector(settings)

    def status(self) -> str:
        return self._checks.status()

    def discovery(self) -> DiagnosticResult:
        return self._checks.discovery()

    def runtime(self) -> tuple[DiagnosticResult, str]:
        return self._checks.runtime()

    def direct(self, address: str) -> DiagnosticResult:
        return self._checks.direct(address)

    def bundle(self, include_packet_capture: bool = False) -> tuple[IO[bytes], str]:
        """Collect one support archive: submit, gather, then lay out the zip."""
        deadline = time.monotonic() + self._settings.diagnostics_bundle_budget_seconds
        request_id = os.urandom(16).hex()
        # Submit first so the host collector runs while the container gathers
        # its own answers, rather than after them.
        request_error = self._host.submit(request_id, include_packet_capture)
        container = self._checks.collect(deadline)
        host = self._host.collect(
            request_id,
            deadline,
            include_packet_capture,
            request_error,
        )

        bundle = tempfile.SpooledTemporaryFile(max_size=4 * 1024 * 1024)
        timestamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
        # Both spools can have rolled over to real files by now, and a packet
        # capture is allowed to reach 64 MB. The container's /tmp is a small
        # tmpfs, so a failure part-way through the archive must release them
        # rather than leave them for whenever the collector next runs.
        try:
            self._write_archive(bundle, container, host)
        except BaseException:
            bundle.close()
            raise
        finally:
            host.close()
        bundle.seek(0)
        return bundle, f"omt-diagnostics-{timestamp}.zip"

    def _write_archive(
        self,
        bundle: IO[bytes],
        container: ContainerReport,
        host: HostReport,
    ) -> None:
        """Write every archive member. Members arrive resolved, not as outcomes.

        The collection steps have already decided what each member should
        contain, so this only lays out the archive. The caller owns the spools.
        """
        with zipfile.ZipFile(bundle, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("version.txt", self._about.version() + "\n")
            archive.writestr(
                "runtime-settings.txt",
                "\n".join(self._settings.diagnostic_lines()) + "\n",
            )
            archive.writestr("runtime.txt", container.runtime)
            archive.writestr("discovery.json", container.discovery)
            archive.writestr("controller-status.txt", container.controller_status)
            archive.writestr("current-target-receive-probe.json", container.receive_probe)
            for name, setting, limit in COPIED_MEMBERS:
                result = read_bytes(getattr(self._settings, setting), limit)
                archive.writestr(
                    name,
                    result.data if result.ok else unavailable(result.detail or result.status.value),
                )
            archive.writestr("host-report.txt", host.report)
            archive.writestr("host-network-pcap.txt", host.capture_metadata)
            if host.packet_capture is not None:
                # Raw captures are already dense binary data and may reach
                # 64 MiB. Deflating them burns the Pi's CPU inside the fixed
                # Gunicorn request budget for little benefit; store this one
                # member while retaining compression for the text diagnostics.
                packet_info = zipfile.ZipInfo(
                    "host-network.pcap",
                    date_time=datetime.now(UTC).timetuple()[:6],
                )
                packet_info.compress_type = zipfile.ZIP_STORED
                with archive.open(packet_info, "w") as destination:
                    shutil.copyfileobj(host.packet_capture, destination, 1024 * 1024)
            elif host.capture_requested:
                # Opting in must never silently omit the capture, so an absent
                # one still owes the operator a stated reason. Opting out owes
                # nothing, and is the only case that writes neither member.
                archive.writestr(
                    "host-network.pcap.unavailable.txt",
                    unavailable(host.packet_capture_error),
                )
