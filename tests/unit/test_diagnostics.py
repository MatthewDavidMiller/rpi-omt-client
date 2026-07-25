"""Request correlation, PCAP validation, and archive-content tests."""

from __future__ import annotations

import hashlib
import os
import time
import zipfile
from pathlib import Path

import pytest
from conftest import VirtualClock, raises

from omt_client.models import CommandResult
from omt_client.services.diagnostics import (
    PCAP_MAX_BYTES,
    RuntimeDiagnostics,
    _parse_header,
)
from omt_client.services.playback import RuntimeSourcePlayback
from omt_client.settings import load_settings

REQUEST_ID = "11" * 16
PCAP = bytes.fromhex("d4c3b2a1") + b"\0" * 20


def _settings(tmp_path: Path, **overrides: str):
    (tmp_path / "run").mkdir(exist_ok=True)
    (tmp_path / "omt").mkdir(exist_ok=True)
    values = {
        "OMT_CONFIG_DIR": str(tmp_path),
        "OMT_RUNTIME_CONFIG_FILE": str(tmp_path / "omt/settings.xml"),
        "OMT_RECEIVER_COMMAND": "/receiver",
        "OMT_CONTROL_COMMAND": "/control",
        "OMT_DIAGNOSTICS_HOST_REPORT_FILE": str(tmp_path / "host-report.txt"),
        "OMT_DIAGNOSTICS_HOST_REQUEST_FILE": str(tmp_path / "request"),
        "OMT_DIAGNOSTICS_HOST_PCAP_FILE": str(tmp_path / "capture.pcap"),
        "OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE": str(tmp_path / "capture.txt"),
        "OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS": "0.01",
        "OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS": "1",
        "RPI_OMT_CLIENT_VERSION_FILE": str(tmp_path / "version"),
        "OMT_RUNTIME_INTEGRITY_MANIFEST": str(tmp_path / "integrity"),
    }
    values.update(overrides)
    return load_settings(values)


def _diagnostics(tmp_path: Path, **overrides: str) -> RuntimeDiagnostics:
    settings = _settings(tmp_path, **overrides)
    return RuntimeDiagnostics(settings, RuntimeSourcePlayback(settings))


def _metadata(
    *,
    request_id: str = REQUEST_ID,
    status: str = "complete",
    size: str = str(len(PCAP)),
    digest: str = hashlib.sha256(PCAP).hexdigest(),
    magic: str = "d4c3b2a1",
    maximum: str = str(PCAP_MAX_BYTES),
) -> str:
    return (
        "version=1\n"
        f"request_id={request_id}\n"
        f"capture_status={status}\n"
        "capture_interface=any\n"
        "capture_filter=none\n"
        "capture_snaplen=full\n"
        "capture_seconds=1\n"
        f"max_bytes={maximum}\n"
        f"size_bytes={size}\n"
        f"sha256={digest}\n"
        f"pcap_magic={magic}\n"
        "tcpdump_exit_status=0\n\nstats\n"
    )


def test_header_parser_is_exact_and_stops_at_body():
    assert _parse_header("a=1\nb=2\n\nbody", {"a", "b"}) == {"a": "1", "b": "2"}
    assert _parse_header("a=1\na=2", {"a"}) is None
    assert _parse_header("missing separator", {"a"}) is None
    assert _parse_header("a=1", {"a", "b"}) is None


def test_host_request_is_fixed_inode_and_correlated(tmp_path: Path):
    diagnostics = _diagnostics(tmp_path)
    request = Path(diagnostics._settings.diagnostics_host_request_file)
    request.touch(mode=0o600)
    os.chmod(request, 0o600)
    inode = request.stat().st_ino
    assert diagnostics._request_host_report(REQUEST_ID, True) == ""
    assert request.stat().st_ino == inode
    assert f"request_id={REQUEST_ID}" in request.read_text()
    assert "capture_pcap=1" in request.read_text()
    request.unlink()
    assert "unable to submit" in diagnostics._request_host_report(REQUEST_ID, False)


def test_fresh_host_report_accepts_only_matching_bounded_schema(tmp_path: Path):
    diagnostics = _diagnostics(tmp_path)
    report = Path(diagnostics._settings.diagnostics_host_report_file)
    report.write_text(
        f"version=1\nrequest_id={REQUEST_ID}\nstatus=partial\n\nbody\n",
        encoding="utf-8",
    )
    value, error = diagnostics._fresh_host_report(REQUEST_ID, time.monotonic() + 1)
    assert value.endswith(b"body\n") and not error
    report.write_text(
        "version=1\nrequest_id=wrong\nstatus=complete\n\nbody\n",
        encoding="utf-8",
    )
    value, error = diagnostics._fresh_host_report(REQUEST_ID, time.monotonic() + 0.02)
    assert not value and "did not match" in error
    report.unlink()
    value, error = diagnostics._fresh_host_report(REQUEST_ID, time.monotonic() + 0.02)
    assert not value and "does not exist" in error


@pytest.mark.parametrize(
    ("mutate", "expected"),
    [
        (lambda value: value.replace("version=1", "version=2"), "does not match"),
        (lambda value: value.replace("capture_status=complete", "capture_status=odd"), "status"),
        (lambda value: value.replace(f"size_bytes={len(PCAP)}", "size_bytes=bad"), "size"),
        (lambda value: value.replace(f"max_bytes={PCAP_MAX_BYTES}", "max_bytes=1"), "limit"),
        (lambda value: value.replace("sha256=", "sha256=nothex"), "digest"),
        (lambda value: value.replace("pcap_magic=d4c3b2a1", "pcap_magic=00"), "magic"),
        (lambda value: value.replace("capture_interface=any\n", ""), "schema"),
    ],
)
def test_capture_metadata_failures_are_typed(tmp_path: Path, mutate, expected: str):
    diagnostics = _diagnostics(tmp_path)
    path = Path(diagnostics._settings.diagnostics_host_pcap_metadata_file)
    path.write_text(mutate(_metadata()), encoding="utf-8")
    data, fields, error = diagnostics._capture_metadata(REQUEST_ID)
    assert data and fields is None and expected in error


def test_capture_metadata_accepts_disabled_and_missing(tmp_path: Path):
    diagnostics = _diagnostics(tmp_path)
    data, fields, error = diagnostics._capture_metadata(REQUEST_ID)
    assert not data and fields is None and "does not exist" in error
    path = Path(diagnostics._settings.diagnostics_host_pcap_metadata_file)
    path.write_text(
        _metadata(status="disabled", size="0", digest="0" * 64, magic="unavailable"),
        encoding="utf-8",
    )
    data, fields, error = diagnostics._capture_metadata(REQUEST_ID)
    assert data and fields is not None and fields["capture_status"] == "disabled"
    assert not error


def test_validated_pcap_checks_size_magic_hash_and_symlink(tmp_path: Path):
    diagnostics = _diagnostics(tmp_path)
    path = Path(diagnostics._settings.diagnostics_host_pcap_file)
    path.write_bytes(PCAP)
    metadata = {
        "size_bytes": str(len(PCAP)),
        "sha256": hashlib.sha256(PCAP).hexdigest(),
    }
    spool, error = diagnostics._validated_pcap(metadata)
    assert spool is not None and spool.read() == PCAP and not error
    spool.close()
    metadata["sha256"] = "0" * 64
    spool, error = diagnostics._validated_pcap(metadata)
    assert spool is None and "SHA-256" in error
    metadata["sha256"] = hashlib.sha256(PCAP).hexdigest()
    metadata["size_bytes"] = "1"
    assert "unexpected size" in diagnostics._validated_pcap(metadata)[1]
    path.unlink()
    path.symlink_to(tmp_path / "missing")
    assert "unsafe" in diagnostics._validated_pcap(metadata)[1]


def test_capture_metadata_rejects_a_non_hex_magic(tmp_path: Path):
    """A magic value that is not even hex must be rejected on its own, not left
    to bytes.fromhex to raise."""
    diagnostics = _diagnostics(tmp_path)
    path = Path(diagnostics._settings.diagnostics_host_pcap_metadata_file)
    path.write_text(_metadata(magic="zz"), encoding="utf-8")
    assert "magic is invalid" in diagnostics._capture_metadata(REQUEST_ID)[2]
    path.write_text(_metadata(magic="d4c3b2a"), encoding="utf-8")
    assert "magic is invalid" in diagnostics._capture_metadata(REQUEST_ID)[2]


def _valid_metadata(payload: bytes = PCAP) -> dict[str, str]:
    return {"size_bytes": str(len(payload)), "sha256": hashlib.sha256(payload).hexdigest()}


def test_validated_pcap_rejects_a_capture_swapped_while_opening(tmp_path: Path, monkeypatch):
    """lstat and the open target must be the same inode; otherwise a capture
    substituted in that window would be streamed to the operator."""
    diagnostics = _diagnostics(tmp_path)
    Path(diagnostics._settings.diagnostics_host_pcap_file).write_bytes(PCAP)
    original_fstat = os.fstat
    first = True

    def swapped_fstat(descriptor: int):
        nonlocal first
        result = original_fstat(descriptor)
        if first:
            first = False
            values = list(result)
            values[1] += 1
            return os.stat_result(values)
        return result

    monkeypatch.setattr(os, "fstat", swapped_fstat)
    spool, error = diagnostics._validated_pcap(_valid_metadata())
    assert spool is None and "changed while opening" in error


def test_validated_pcap_rejects_a_capture_that_outgrows_its_declared_size(
    tmp_path: Path, monkeypatch
):
    """The reader stops at the declared length, then proves nothing follows."""
    diagnostics = _diagnostics(tmp_path)
    path = Path(diagnostics._settings.diagnostics_host_pcap_file)
    path.write_bytes(PCAP)
    original_read = os.read
    grown = False

    def growing_read(descriptor: int, count: int) -> bytes:
        nonlocal grown
        if not grown:
            grown = True
            with open(path, "ab") as handle:
                handle.write(b"extra")
        return original_read(descriptor, count)

    monkeypatch.setattr(os, "read", growing_read)
    spool, error = diagnostics._validated_pcap(_valid_metadata())
    assert spool is None and "exceeds its declared size" in error


def test_validated_pcap_rejects_a_short_read_before_the_declared_size(tmp_path: Path, monkeypatch):
    diagnostics = _diagnostics(tmp_path)
    Path(diagnostics._settings.diagnostics_host_pcap_file).write_bytes(PCAP)
    monkeypatch.setattr(os, "read", lambda *_args: b"")
    spool, error = diagnostics._validated_pcap(_valid_metadata())
    assert spool is None and "ended before its declared size" in error


def test_validated_pcap_rejects_a_capture_replaced_during_the_read(tmp_path: Path, monkeypatch):
    diagnostics = _diagnostics(tmp_path)
    path = Path(diagnostics._settings.diagnostics_host_pcap_file)
    path.write_bytes(PCAP)
    replacement = tmp_path / "replacement.pcap"
    replacement.write_bytes(PCAP)
    original_read = os.read
    swapped = False

    def swapping_read(descriptor: int, count: int) -> bytes:
        nonlocal swapped
        chunk = original_read(descriptor, count)
        if not swapped:
            swapped = True
            os.replace(replacement, path)
        return chunk

    monkeypatch.setattr(os, "read", swapping_read)
    spool, error = diagnostics._validated_pcap(_valid_metadata())
    assert spool is None and "changed while being read" in error


def test_validated_pcap_rejects_on_disk_content_that_is_not_a_capture(tmp_path: Path):
    """The metadata magic and the bytes on disk are checked separately, so a
    truthful-looking header cannot vouch for the payload."""
    diagnostics = _diagnostics(tmp_path)
    payload = b"NOPE" + b"\0" * 20
    Path(diagnostics._settings.diagnostics_host_pcap_file).write_bytes(payload)
    spool, error = diagnostics._validated_pcap(_valid_metadata(payload))
    assert spool is None and "magic is invalid" in error


def test_validated_pcap_reports_an_os_error_during_the_read(tmp_path: Path, monkeypatch):
    diagnostics = _diagnostics(tmp_path)
    Path(diagnostics._settings.diagnostics_host_pcap_file).write_bytes(PCAP)

    def failing_read(*_args):
        raise OSError("device error")

    monkeypatch.setattr(os, "read", failing_read)
    spool, error = diagnostics._validated_pcap(_valid_metadata())
    assert spool is None and "unable to read packet capture" in error


def test_validated_pcap_reports_a_capture_that_cannot_be_opened(tmp_path: Path, monkeypatch):
    """lstat can succeed and the open still fail; nothing may be left open or
    spooled when it does."""
    diagnostics = _diagnostics(tmp_path)
    Path(diagnostics._settings.diagnostics_host_pcap_file).write_bytes(PCAP)
    monkeypatch.setattr(os, "open", raises(PermissionError("denied")))
    spool, error = diagnostics._validated_pcap(_valid_metadata())
    assert spool is None and "unable to read packet capture" in error


def test_validated_pcap_assembles_the_magic_across_short_reads(tmp_path: Path, monkeypatch):
    """A capture arrives in whatever chunks the kernel returns, so the magic
    must be accumulated across them rather than read from the first chunk."""
    diagnostics = _diagnostics(tmp_path)
    Path(diagnostics._settings.diagnostics_host_pcap_file).write_bytes(PCAP)
    original_read = os.read

    def short_read(descriptor: int, count: int) -> bytes:
        return original_read(descriptor, min(2, count))

    monkeypatch.setattr(os, "read", short_read)
    spool, error = diagnostics._validated_pcap(_valid_metadata())
    assert spool is not None and not error
    assert spool.read() == PCAP
    spool.close()


def _prepare_bundle(diagnostics: RuntimeDiagnostics, monkeypatch) -> Path:
    request = Path(diagnostics._settings.diagnostics_host_request_file)
    request.touch(mode=0o600)
    os.chmod(request, 0o600)
    monkeypatch.setattr(os, "urandom", lambda _size: bytes.fromhex(REQUEST_ID))
    monkeypatch.setattr(
        "omt_client.services.diagnostics.run_command",
        lambda command, _timeout: CommandResult(command=" ".join(command), returncode=0),
    )
    return request


@pytest.mark.parametrize(
    ("metadata_text", "expected"),
    [
        (None, "does not exist"),
        (
            _metadata(status="unavailable", magic="none"),
            "host packet capture status is unavailable",
        ),
        (_metadata(request_id="00" * 16), "does not match this request"),
    ],
)
def test_bundle_reports_why_a_requested_capture_is_absent(
    tmp_path: Path, monkeypatch, metadata_text, expected
):
    """Opting in must never silently omit the capture: the archive carries a
    stated reason instead."""
    diagnostics = _diagnostics(tmp_path, OMT_DIAGNOSTICS_RECEIVE_PROBE="0")
    _prepare_bundle(diagnostics, monkeypatch)
    if metadata_text is not None:
        Path(diagnostics._settings.diagnostics_host_pcap_metadata_file).write_text(
            metadata_text, encoding="utf-8"
        )
    bundle, _name = diagnostics.bundle(include_packet_capture=True)
    with zipfile.ZipFile(bundle) as archive:
        assert "host-network.pcap" not in archive.namelist()
        reason = archive.read("host-network.pcap.unavailable.txt").decode()
    assert expected in reason


def test_bundle_reports_a_capture_that_fails_validation(tmp_path: Path, monkeypatch):
    """Valid metadata but an unreadable capture still yields a stated reason."""
    diagnostics = _diagnostics(tmp_path, OMT_DIAGNOSTICS_RECEIVE_PROBE="0")
    _prepare_bundle(diagnostics, monkeypatch)
    Path(diagnostics._settings.diagnostics_host_pcap_metadata_file).write_text(
        _metadata(), encoding="utf-8"
    )
    bundle, _name = diagnostics.bundle(include_packet_capture=True)
    with zipfile.ZipFile(bundle) as archive:
        reason = archive.read("host-network.pcap.unavailable.txt").decode()
    assert "unable to inspect packet capture" in reason


def test_bundle_contains_correlated_report_metadata_and_opted_in_pcap(tmp_path: Path, monkeypatch):
    diagnostics = _diagnostics(tmp_path)
    settings = diagnostics._settings
    request = Path(settings.diagnostics_host_request_file)
    request.touch(mode=0o600)
    os.chmod(request, 0o600)
    Path(settings.diagnostics_host_report_file).write_text(
        f"version=1\nrequest_id={REQUEST_ID}\nstatus=complete\n\nhost body\n",
        encoding="utf-8",
    )
    Path(settings.diagnostics_host_pcap_file).write_bytes(PCAP)
    Path(settings.diagnostics_host_pcap_metadata_file).write_text(_metadata(), encoding="utf-8")
    Path(settings.version_file).write_text("v2", encoding="utf-8")
    Path(settings.runtime_config_file).write_text("<Settings />", encoding="utf-8")
    Path(settings.runtime_integrity_manifest).write_text("hash  file", encoding="utf-8")
    Path(settings.playback_status_file).write_text("{}", encoding="utf-8")
    Path(settings.source_target_file).write_text(
        '{"schema":1,"kind":"discovered","name":"Camera"}\n',
        encoding="utf-8",
    )
    monkeypatch.setattr(os, "urandom", lambda _size: bytes.fromhex(REQUEST_ID))
    monkeypatch.setattr(
        "omt_client.services.diagnostics.run_command",
        lambda command, _timeout: CommandResult(
            command=" ".join(command),
            returncode=0,
            stdout='[{"name":"Camera","target":"Camera"}]' if "discover" in command else "ok",
        ),
    )
    bundle, name = diagnostics.bundle(include_packet_capture=True)
    assert name.startswith("omt-diagnostics-")
    with zipfile.ZipFile(bundle) as archive:
        names = set(archive.namelist())
        assert {
            "runtime-settings.txt",
            "runtime.txt",
            "discovery.json",
            "controller-status.txt",
            "current-target-receive-probe.json",
            "playback-status.json",
            "omt-settings.xml",
            "runtime-sha256.manifest",
            "host-report.txt",
            "host-network-pcap.txt",
            "host-network.pcap",
        } <= names
        assert archive.read("host-network.pcap") == PCAP
        assert b"host body" in archive.read("host-report.txt")
    assert "capture_pcap=1" in request.read_text()


def test_bundle_opt_out_never_streams_raw_capture(tmp_path: Path, monkeypatch):
    diagnostics = _diagnostics(tmp_path, OMT_DIAGNOSTICS_RECEIVE_PROBE="0")
    request = Path(diagnostics._settings.diagnostics_host_request_file)
    request.touch(mode=0o600)
    os.chmod(request, 0o600)
    monkeypatch.setattr(os, "urandom", lambda _size: bytes.fromhex(REQUEST_ID))
    monkeypatch.setattr(
        "omt_client.services.diagnostics.run_command",
        lambda command, _timeout: CommandResult(command=" ".join(command), returncode=1),
    )
    bundle, _name = diagnostics.bundle(include_packet_capture=False)
    with zipfile.ZipFile(bundle) as archive:
        assert "host-network.pcap" not in archive.namelist()
        assert "host-network-pcap.txt" in archive.namelist()
        assert b"skipped" in archive.read("current-target-receive-probe.json")
    assert "capture_pcap=0" in request.read_text()


def test_bundle_budget_bounds_all_container_commands(tmp_path: Path, monkeypatch):
    """Every container command shares one budget, and the first one still runs.

    The clock is virtual: a real one measures this host's fsync latency for the
    correlated host request, which on a slow filesystem alone exhausts a budget
    this small and skips every command before the deadline arithmetic is
    exercised at all."""
    budget = 0.03
    diagnostics = _diagnostics(
        tmp_path,
        OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS=str(budget),
        OMT_DIAGNOSTICS_RECEIVE_PROBE="0",
    )
    request = Path(diagnostics._settings.diagnostics_host_request_file)
    request.touch(mode=0o600)
    os.chmod(request, 0o600)
    clock = VirtualClock()
    monkeypatch.setattr("omt_client.services.diagnostics.time", clock.module())
    timeouts: list[float] = []

    def consume_timeout(command, timeout):
        timeouts.append(timeout)
        clock.sleep(timeout)
        return CommandResult(command=" ".join(command), returncode=0, stdout="ok")

    monkeypatch.setattr("omt_client.services.diagnostics.run_command", consume_timeout)
    started = clock.now
    bundle, _name = diagnostics.bundle()
    bundle.close()

    assert timeouts
    assert sum(timeouts) <= budget
    assert clock.now - started <= budget
