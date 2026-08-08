"""Production OMT service-boundary tests."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import threading
import time
import zipfile
from pathlib import Path
from typing import Any, cast
from unittest import mock

import pytest
from conftest import VirtualClock, raises
from flask import Flask, session
from werkzeug.security import generate_password_hash

from omt_client.models import CommandResult, SourceConfigurationView
from omt_client.safe_io import atomic_replace
from omt_client.services import (
    HostSystem,
    PersistentAuthentication,
    RuntimeAbout,
    RuntimeDiagnostics,
    RuntimeNetwork,
    RuntimeSourcePlayback,
    production_services,
)
from omt_client.services import command as command_module
from omt_client.services.command import COMMAND_OUTPUT_LIMIT, run_command
from omt_client.settings import load_settings
from omt_client.state_store import SourceTarget, save_source_target

PASSWORD = "correct"
# scrypt derivation costs ~85ms; the hash of a constant password is invariant, so
# deriving it once per session instead of per test keeps the suite fast.
PASSWORD_HASH = generate_password_hash(PASSWORD)


@pytest.fixture
def request_context():
    """A minimal request context, so auth tests can drive `flask.session`."""
    application = Flask(__name__)
    application.secret_key = "test"
    with application.test_request_context("/"):
        yield


def settings_for(tmp_path: Path, **overrides: str):
    (tmp_path / "run").mkdir(exist_ok=True)
    (tmp_path / "omt").mkdir(exist_ok=True)
    values = {
        "OMT_CONFIG_DIR": str(tmp_path),
        "OMT_RUNTIME_CONFIG_FILE": str(tmp_path / "omt/settings.xml"),
        "OMT_RECEIVER_COMMAND": "/receiver",
        "OMT_CONTROL_COMMAND": "/control",
        "RPI_OMT_CLIENT_VERSION_FILE": str(tmp_path / "version"),
        "OMT_RUNTIME_INTEGRITY_MANIFEST": str(tmp_path / "integrity"),
        "OMT_DIAGNOSTICS_HOST_REPORT_FILE": str(tmp_path / "host-diagnostics.txt"),
        "OMT_DIAGNOSTICS_HOST_REQUEST_FILE": str(tmp_path / "host-diagnostics.request"),
        "OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS": "0.05",
        "OMT_REBOOT_REQUEST_FILE": str(tmp_path / "reboot.request"),
        "OMT_REBOOT_RESULT_FILE": str(tmp_path / "reboot.result"),
        "OMT_REBOOT_ACK_TIMEOUT_SECONDS": "0.1",
    }
    values.update(overrides)
    return load_settings(values)


def command_result(returncode=0, stdout="", stderr="", error=""):
    return CommandResult(
        command="test",
        returncode=returncode,
        stdout=stdout,
        stderr=stderr,
        error=error,
    )


def test_persistent_authentication_hash_sessions_rotation_and_revocation(tmp_path, request_context):
    auth = _authentication(tmp_path)
    assert auth.authenticate("wrong", None) is None
    first = auth.authenticate(PASSWORD, None)
    second = auth.authenticate(PASSWORD, first)
    assert first and second and first != second
    session.update(
        authenticated=True,
        session_id=second,
        password_digest=auth.password_digest,
    )
    assert auth.is_current()
    session["password_digest"] = "bad"
    assert not auth.is_current()
    session["password_digest"] = auth.password_digest
    auth.revoke(second)
    assert not auth.is_current()
    auth.revoke(None)


def test_persistent_authentication_rejects_missing_secret_and_supports_env_password(
    tmp_path, monkeypatch
):
    (tmp_path / "web_password").write_text("unused", encoding="utf-8")
    with pytest.raises(RuntimeError, match="Flask secret"):
        PersistentAuthentication(settings_for(tmp_path))
    (tmp_path / "flask_secret").write_text("secret", encoding="utf-8")
    monkeypatch.setenv("OMT_WEB_PASSWORD", "environment-secret")
    auth = PersistentAuthentication(settings_for(tmp_path))
    assert auth.authenticate("environment-secret", None)


def _authentication(tmp_path) -> PersistentAuthentication:
    (tmp_path / "flask_secret").write_text("a" * 64, encoding="utf-8")
    (tmp_path / "web_password").write_text(PASSWORD_HASH, encoding="utf-8")
    return PersistentAuthentication(settings_for(tmp_path))


def test_session_registry_evicts_the_oldest_entries_past_the_cap(
    tmp_path, monkeypatch, request_context
):
    """A bounded registry stops an attacker from growing the on-disk session
    file without limit by replaying logins."""
    # The eviction rule reads the cap off the class, so a small cap exercises the
    # same code without paying for 72 scrypt verifications.
    cap = 4
    monkeypatch.setattr(PersistentAuthentication, "_maximum_sessions", cap)
    auth = _authentication(tmp_path)
    issued = [auth.authenticate(PASSWORD, None) for _index in range(cap + 8)]
    assert all(issued)
    registry = json.loads((tmp_path / "web_sessions.json").read_text(encoding="utf-8"))
    assert registry["version"] == 1
    assert len(registry["sessions"]) == cap

    session.update(authenticated=True, password_digest=auth.password_digest)
    session["session_id"] = issued[-1]
    assert auth.is_current()
    session["session_id"] = issued[0]
    assert not auth.is_current()


@pytest.mark.parametrize(
    "document",
    [
        "not json",
        '{"version":2,"sessions":{}}',
        '{"version":1,"sessions":[]}',
        '{"version":1}',
        '{"version":1,"sessions":{"short":1.0}}',
        '{"version":1,"sessions":{"' + "a" * 64 + '":"soon"}}',
        '{"version":1,"sessions":{"' + "a" * 64 + '":true}}',
        '{"version":1,"sessions":{"' + "a" * 64 + '":1e999}}',
        '{"version":true,"sessions":{}}',
        '{"version":1,"sessions":{},"extra":true}',
        '{"version":2,"version":1,"sessions":{}}',
        '{"version":1,"sessions":{"' + "a" * 64 + '":1,"' + "a" * 64 + '":2}}',
        '{"version":1,"sessions":{"' + "a" * 64 + '":NaN}}',
        '{"version":1,"sessions":{"' + "a" * 64 + '":1' + "0" * 1000 + "}}",
    ],
)
def test_malformed_session_registry_reads_as_empty(tmp_path, document, request_context):
    auth = _authentication(tmp_path)
    (tmp_path / "web_sessions.json").write_text(document, encoding="utf-8")
    session.update(
        authenticated=True,
        session_id="any-session",
        password_digest=auth.password_digest,
    )
    assert not auth.is_current()


def test_one_corrupt_session_row_does_not_wipe_valid_sessions(tmp_path, request_context):
    """A single bad registry entry must not clear every other operator out.

    authenticate() rewrites the registry from whatever _read_registry returns, so
    treating one corrupt row as a total wipe would revoke every valid session on
    the next successful login."""
    auth = _authentication(tmp_path)
    first = auth.authenticate(PASSWORD, None)
    second = auth.authenticate(PASSWORD, None)
    assert first and second
    registry_path = tmp_path / "web_sessions.json"
    document = json.loads(registry_path.read_text(encoding="utf-8"))
    document["sessions"]["not-a-digest"] = 9999999999.0
    document["sessions"]["a" * 64] = "soon"
    registry_path.write_text(json.dumps(document), encoding="utf-8")
    session.update(
        authenticated=True,
        session_id=first,
        password_digest=auth.password_digest,
    )
    assert auth.is_current()
    third = auth.authenticate(PASSWORD, None)
    assert third
    rewritten = json.loads(registry_path.read_text(encoding="utf-8"))
    assert "not-a-digest" not in rewritten["sessions"]
    assert len(rewritten["sessions"]) >= 2


def test_is_current_requires_authenticated_typed_session_fields(tmp_path, request_context):
    auth = _authentication(tmp_path)
    session_id = auth.authenticate(PASSWORD, None)
    assert not auth.is_current()
    session.update(authenticated=True, session_id=session_id, password_digest=1)
    assert not auth.is_current()
    session.update(password_digest=auth.password_digest, session_id=1)
    assert not auth.is_current()
    session["session_id"] = session_id
    assert auth.is_current()


def test_is_current_fails_closed_when_the_registry_lock_is_unusable(
    tmp_path, monkeypatch, request_context
):
    auth = _authentication(tmp_path)
    session_id = auth.authenticate(PASSWORD, None)
    session.update(
        authenticated=True,
        session_id=session_id,
        password_digest=auth.password_digest,
    )
    monkeypatch.setattr(os, "open", raises(OSError("locked")))
    assert not auth.is_current()


def test_plaintext_password_file_is_compared_without_hashing(tmp_path):
    (tmp_path / "flask_secret").write_text("a" * 64, encoding="utf-8")
    (tmp_path / "web_password").write_text("plain-text-secret\n", encoding="utf-8")
    auth = PersistentAuthentication(settings_for(tmp_path))
    assert auth.authenticate("plain-text-secret", None)
    assert auth.authenticate("plain-text-secre", None) is None
    assert auth.authenticate("", None) is None


@pytest.mark.parametrize(
    "stored",
    [
        # Structurally parseable but semantically impossible parameters. Werkzeug
        # returns False for a hash it cannot even split, so those inputs never
        # reach _verify's exception handler; these do raise.
        "scrypt:0:8:1$salt$abcd",
        "pbkdf2:sha256:0$salt$abcd",
        # UnsupportedDigestmodError, a ValueError subclass, from OpenSSL.
        "pbkdf2:bogusalg$salt$abcd",
    ],
)
def test_corrupt_password_hash_is_rejected_rather_than_raising(tmp_path, stored):
    (tmp_path / "flask_secret").write_text("a" * 64, encoding="utf-8")
    (tmp_path / "web_password").write_text(stored, encoding="utf-8")
    with pytest.raises(RuntimeError, match="unsupported format"):
        PersistentAuthentication(settings_for(tmp_path))


def test_unparseable_password_hash_verifies_as_a_mismatch(tmp_path):
    """Werkzeug returns False rather than raising for a hash it cannot split, so
    the startup probe accepts it and every login attempt simply fails."""
    (tmp_path / "flask_secret").write_text("a" * 64, encoding="utf-8")
    (tmp_path / "web_password").write_text("scrypt:not-a-valid-hash", encoding="utf-8")
    auth = PersistentAuthentication(settings_for(tmp_path))
    assert auth.authenticate("anything", None) is None


def test_argon2_password_hash_fails_startup_instead_of_locking_operators_out(tmp_path):
    """`argon2:` matches the hash-prefix list but Werkzeug cannot verify it. A
    silent False here would reject every correct password with "Invalid
    password" and no diagnosable cause."""
    (tmp_path / "flask_secret").write_text("a" * 64, encoding="utf-8")
    (tmp_path / "web_password").write_text("argon2:v=19$m=65536$abcd", encoding="utf-8")
    with pytest.raises(RuntimeError, match="unsupported format"):
        PersistentAuthentication(settings_for(tmp_path))


def test_missing_password_file_is_a_startup_error(tmp_path):
    (tmp_path / "flask_secret").write_text("a" * 64, encoding="utf-8")
    with pytest.raises(RuntimeError, match="password file"):
        PersistentAuthentication(settings_for(tmp_path))


def test_revoking_an_unknown_session_leaves_the_registry_untouched(tmp_path):
    auth = _authentication(tmp_path)
    auth.authenticate(PASSWORD, None)
    registry = tmp_path / "web_sessions.json"
    before = registry.read_bytes()
    auth.revoke("never-issued")
    assert registry.read_bytes() == before


def test_bounded_command_success_timeout_and_os_error():
    result = run_command(["/bin/sh", "-c", "printf ok; printf err >&2"], 2)
    assert result.returncode == 0 and result.stdout == "ok" and result.stderr == "err"

    timed_out = run_command(["/bin/sh", "-c", "sleep 5"], 0.2)
    assert timed_out.timed_out and timed_out.returncode is None

    missing = run_command(["/nonexistent/omt-receiver-missing"], 1)
    assert missing.error


def test_command_output_is_truncated_to_the_limit_and_flagged():
    """A runaway receiver must not be able to grow a worker's memory through a
    diagnostics page. The cap is applied on both streams and reported, so an
    operator can tell a truncated capture from a complete one."""
    oversized = COMMAND_OUTPUT_LIMIT + 4096
    result = run_command(
        [
            "/bin/sh",
            "-c",
            f"yes x | head -c {oversized}; yes y | head -c {oversized} >&2",
        ],
        30,
    )
    assert len(result.stdout) == COMMAND_OUTPUT_LIMIT
    assert len(result.stderr) == COMMAND_OUTPUT_LIMIT
    assert result.stdout_truncated and result.stderr_truncated

    within = run_command(["/bin/sh", "-c", "printf ok"], 5)
    assert not within.stdout_truncated and not within.stderr_truncated


def test_timed_out_command_still_reports_the_partial_output_it_captured():
    """Whatever the child printed before the kill must reach the operator.
    Discarding it would leave a bare "exceeded N seconds" for the failure mode
    that most often explains itself in that partial text."""
    result = run_command(
        ["/bin/sh", "-c", "printf 'partial stdout'; printf 'partial stderr' >&2; sleep 5"],
        0.3,
    )
    assert result.timed_out and result.returncode is None
    assert result.stdout == "partial stdout" and result.stderr == "partial stderr"
    # `error` outranks the partial streams: the timeout is the actual failure.
    assert result.failure_detail.startswith("Command exceeded")


def test_timing_out_kills_the_whole_process_group_not_just_the_child():
    """`control-omt.sh` launches the receiver; `host-diagnostics.sh` launches
    `tcpdump`. Signalling only the direct child on a timeout would reap the
    script and leave what it started holding /dev/dri or a capture socket, with
    nothing left watching it. Every child is started in its own session so the
    group can be signalled as a unit."""
    result = run_command(
        ["/bin/sh", "-c", 'sleep 60 & printf "%s" "$!"; sleep 60'],
        0.5,
    )
    assert result.timed_out
    grandchild = int(result.stdout)
    for _ in range(100):
        try:
            os.kill(grandchild, 0)
        except ProcessLookupError:
            return
        time.sleep(0.05)
    os.kill(grandchild, signal.SIGKILL)
    raise AssertionError(f"grandchild {grandchild} survived the group kill")


def test_a_child_that_will_not_die_is_abandoned_rather_than_waited_on():
    """The bound on `_reap` is the point of it. `Popen.wait()` and the `with`
    statement's `__exit__` both block forever on a process stuck in an
    uninterruptible read -- a wedged /dev/dri or ALSA device is exactly that --
    which would turn a reported command timeout into a Gunicorn worker killed at
    `--timeout`, ending the operator's session instead of one request."""
    killed: list[int] = []

    class Undying:
        pid = 4242
        returncode = None

        def wait(self, timeout=None):
            raise subprocess.TimeoutExpired("stuck", timeout)

    def record(pid, number):
        assert number == signal.SIGKILL
        killed.append(pid)

    with mock.patch("os.killpg", record):
        command_module._reap(cast(Any, Undying()), timed_out=True)
    # Escalated once for the timeout and once more when the grace expired, and
    # returned either way rather than blocking on the process.
    assert killed == [4242, 4242]


def test_killing_a_group_that_already_exited_is_not_an_error():
    """The child usually dies on its own between the drain loop and the kill.
    A raised ProcessLookupError there would replace a perfectly good result with
    a spurious failure."""

    class Gone:
        pid = 2**30
        returncode = 0

    command_module._kill_group(cast(Any, Gone()))


def test_a_read_failure_mid_drain_reports_the_error_and_stops_the_child(monkeypatch):
    """An OSError while draining still leaves a spawned child behind, so the
    failure path has to take it down instead of returning and forgetting it."""
    reaped: list[bool] = []
    monkeypatch.setattr(
        command_module,
        "_drain_bounded",
        raises(OSError("pipe read failed")),
    )
    monkeypatch.setattr(
        command_module,
        "_reap",
        lambda _process, timed_out: reaped.append(timed_out),
    )
    result = run_command(["/bin/sh", "-c", "printf ok"], 5)
    assert result.error == "pipe read failed" and result.returncode is None
    assert reaped == [True]


def _drain_batches(batches: tuple[bytes, ...], limit: int):
    """Drive `_drain_bounded` over a real pipe fed one discrete batch at a time.

    Each batch is separated by a pause so it arrives as its own `os.read`,
    which is what makes the boundary cases below reproducible rather than
    dependent on how the kernel happens to coalesce pipe writes.
    """
    read_fd, write_fd = os.pipe()

    def write_batches():
        for batch in batches:
            os.write(write_fd, batch)
            time.sleep(0.2)
        os.close(write_fd)

    writer = threading.Thread(target=write_batches)
    try:
        writer.start()

        class Stub:
            def __init__(self, descriptor):
                self._descriptor = descriptor

            def fileno(self):
                return self._descriptor

        class FakeProcess:
            stdout = Stub(read_fd)
            stderr = None

        with mock.patch.object(command_module, "COMMAND_OUTPUT_LIMIT", limit):
            return command_module._drain_bounded(cast(Any, FakeProcess()), 5)
    finally:
        writer.join(5)
        os.close(read_fd)


@pytest.mark.parametrize("first_batch", [100, 4096], ids=["exact-fill", "overshoot"])
def test_drain_discards_everything_past_the_limit_without_buffering_it(first_batch):
    """Exercised directly rather than through a subprocess: whether a real child
    happens to fill the buffer exactly or overshoot it depends on kernel pipe
    scheduling, and both are contractual.

    The two arrive at the cap by different routes. An overshooting batch is
    sliced and flags the stream in the same step. A batch that lands exactly on
    the limit leaves the buffer full but *not* yet flagged, so the following
    batch is the only thing that can mark the stream truncated -- and it must do
    so without buffering a byte of what it discards.
    """
    limit = 100
    stdout, stderr, stdout_truncated, stderr_truncated, timed_out = _drain_batches(
        (b"a" * first_batch, b"b" * 4096), limit
    )
    assert stdout == b"a" * limit and stdout_truncated
    # A process without a stderr pipe contributes an empty, untruncated stream
    # rather than a KeyError.
    assert stderr == b"" and not stderr_truncated
    assert not timed_out


@pytest.mark.parametrize(
    ("result", "failure_detail", "report_text"),
    [
        (command_result(error="spawn failed", stderr="noise", stdout="out"), "spawn failed", "out"),
        (command_result(stderr=" stderr \n", stdout=" stdout "), "stderr", "stdout"),
        (command_result(stdout=" stdout "), "stdout", "stdout"),
        (command_result(), "", "unavailable"),
    ],
    ids=["error-wins", "stderr-over-stdout", "stdout-only", "silent"],
)
def test_command_detail_and_report_use_opposite_precedence(result, failure_detail, report_text):
    """The two accessors answer different questions. `failure_detail` is appended
    to a caller's own error sentence, so a silent failure must contribute nothing
    rather than the word "unavailable"; `report_text` is shown on its own and so
    always renders something."""
    assert result.failure_detail == failure_detail
    assert result.report_text == report_text


def test_atomic_write_is_private_bounded_and_rejects_unsafe_targets(tmp_path):
    target = tmp_path / "value"
    atomic_replace(str(target), b"safe", 4)
    assert target.read_bytes() == b"safe"
    assert target.stat().st_mode & 0o777 == 0o600
    with pytest.raises(OSError, match="exceeds"):
        atomic_replace(str(target), b"large", 4)
    target.unlink()
    target.symlink_to("elsewhere")
    with pytest.raises(OSError, match="regular"):
        atomic_replace(str(target), b"x", 4)


def test_source_discovery_cache_selection_direct_clear_and_restart(tmp_path, monkeypatch):
    settings = settings_for(tmp_path)
    calls: list[list[str]] = []

    def run(command, _timeout):
        calls.append(command)
        if "discover" in command:
            return command_result(
                stdout='[{"name":"Camera","target":"Camera"},{"name":"Bad","target":"Other"}]'
            )
        return command_result()

    monkeypatch.setattr("omt_client.services.playback.run_command", run)
    service = RuntimeSourcePlayback(settings)
    assert [choice.name for choice in service.sources()] == ["Camera"]
    assert [choice.name for choice in service.sources()] == ["Camera"]
    assert len([call for call in calls if "discover" in call]) == 1
    assert not service.select("bad\nselection").ok
    assert service.select("discovered|Camera").ok
    assert service.configuration().source == "Camera"
    assert service.configuration().direct_address == ""
    assert service.restart().ok
    assert not service.save_direct("host:6400").ok
    assert service.save_direct("omt://192.0.2.1:6400").ok
    assert service.configuration().source == "omt://192.0.2.1:6400"
    assert service.configuration().direct_address == "omt://192.0.2.1:6400"
    assert service.clear().ok
    assert service.configuration().source == ""
    assert not service.configuration().configured
    assert not service.restart().ok


def test_discovery_cache_ttl_starts_when_the_answer_arrives(tmp_path, monkeypatch):
    """The TTL must cover the cached answer's own lifetime, not overlap the
    discovery that produced it. `discover --wait-ms 1500` blocks for seconds, so
    anchoring the expiry to a pre-command clock spends that time out of the TTL
    -- and once the discovery outlasts the TTL the entry is born expired, so
    every dashboard render pays for another multi-second discovery."""
    clock = VirtualClock()
    monkeypatch.setattr("omt_client.services.playback.time", clock.module())
    settings = settings_for(tmp_path, OMT_SOURCE_CACHE_TTL_SECONDS="2")
    discoveries = 0

    def run(command, _timeout):
        nonlocal discoveries
        discoveries += 1
        clock.sleep(3)  # longer than the TTL, as a real --wait-ms 1500 can be
        return command_result(stdout='[{"name":"Camera","target":"Camera"}]')

    monkeypatch.setattr("omt_client.services.playback.run_command", run)
    service = RuntimeSourcePlayback(settings)

    assert [choice.name for choice in service.sources()] == ["Camera"]
    assert [choice.name for choice in service.sources()] == ["Camera"]
    assert discoveries == 1, "a cache entry must not expire before it is first read"

    clock.sleep(2)
    assert [choice.name for choice in service.sources()] == ["Camera"]
    assert discoveries == 2, "the entry must still expire a full TTL after it was stored"


def test_a_failed_discovery_is_cached_so_a_broken_receiver_is_not_hammered(tmp_path, monkeypatch):
    """The dashboard calls sources() on every render. A receiver that fails fast
    would otherwise be re-invoked once per request for as long as it stays
    broken, so the empty answer is cached exactly like a successful one."""
    clock = VirtualClock()
    monkeypatch.setattr("omt_client.services.playback.time", clock.module())
    settings = settings_for(tmp_path, OMT_SOURCE_CACHE_TTL_SECONDS="5")
    attempts = 0

    def run(command, _timeout):
        nonlocal attempts
        attempts += 1
        return command_result(returncode=1, stderr="receiver is not running")

    monkeypatch.setattr("omt_client.services.playback.run_command", run)
    service = RuntimeSourcePlayback(settings)

    assert service.sources() == []
    assert service.sources() == []
    assert attempts == 1

    # An operator pressing Refresh must still get a real retry immediately.
    service.refresh()
    assert service.sources() == []
    assert attempts == 2


@pytest.mark.parametrize("cache_ttl", ["0", "5"])
def test_concurrent_source_requests_share_one_discovery(tmp_path, monkeypatch, cache_ttl):
    """Two dashboard callers must not launch duplicate receiver discoveries."""
    settings = settings_for(tmp_path, OMT_SOURCE_CACHE_TTL_SECONDS=cache_ttl)
    discovery_started = threading.Event()
    release_discovery = threading.Event()
    second_discovery_started = threading.Event()
    attempts = 0

    def run(command, _timeout):
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            discovery_started.set()
        else:
            second_discovery_started.set()
        assert release_discovery.wait(2)
        return command_result(stdout='[{"name":"Camera","target":"Camera"}]')

    monkeypatch.setattr("omt_client.services.playback.run_command", run)
    service = RuntimeSourcePlayback(settings)
    results: list[list[str]] = []

    def discover():
        results.append([choice.name for choice in service.sources()])

    first = threading.Thread(target=discover)
    second = threading.Thread(target=discover)
    first.start()
    assert discovery_started.wait(1)
    second.start()
    assert not second_discovery_started.wait(0.1)
    release_discovery.set()
    first.join(2)
    second.join(2)

    assert not first.is_alive() and not second.is_alive()
    assert results == [["Camera"], ["Camera"]]
    assert attempts == 1


def test_failed_discovery_releases_waiters_for_a_retry(tmp_path, monkeypatch):
    """An unexpected adapter failure must not leave discovery marked in flight."""
    settings = settings_for(tmp_path)
    attempts = 0

    def run(command, _timeout):
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            raise RuntimeError("adapter failed")
        return command_result(stdout='[{"name":"Camera","target":"Camera"}]')

    monkeypatch.setattr("omt_client.services.playback.run_command", run)
    service = RuntimeSourcePlayback(settings)

    with pytest.raises(RuntimeError, match="adapter failed"):
        service.sources()
    assert [choice.name for choice in service.sources()] == ["Camera"]
    assert attempts == 2


def test_a_waiter_runs_its_own_discovery_when_the_one_it_waited_on_fails(tmp_path, monkeypatch):
    """The failure path wakes waiters without publishing a cache entry.

    A waiter released that way sees an unchanged generation, so it must go back
    to the in-flight check and take over the discovery itself. Returning the
    cache on the strength of the wakeup alone would hand the dashboard the empty
    startup list as though discovery had answered with no sources.
    """
    settings = settings_for(tmp_path)
    first_started = threading.Event()
    release_first = threading.Event()
    attempts = 0

    def run(command, _timeout):
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            first_started.set()
            assert release_first.wait(2)
            raise RuntimeError("adapter failed")
        return command_result(stdout='[{"name":"Camera","target":"Camera"}]')

    monkeypatch.setattr("omt_client.services.playback.run_command", run)
    service = RuntimeSourcePlayback(settings)

    # Park the waiter deterministically: releasing the first discovery before
    # the second thread has actually blocked would let it run its own discovery
    # from the start and never reach the wakeup this test is about.
    waiting = threading.Event()
    condition_wait = service._cache_condition.wait

    def announce_wait(timeout=None):
        waiting.set()
        return condition_wait(timeout)

    monkeypatch.setattr(service._cache_condition, "wait", announce_wait)

    waiter_result: list[list[str]] = []
    failure: list[BaseException] = []

    def wait_for_the_first_discovery():
        waiter_result.append([choice.name for choice in service.sources()])

    def fail_the_first_discovery():
        try:
            service.sources()
        except RuntimeError as exc:
            failure.append(exc)

    waiter = threading.Thread(target=wait_for_the_first_discovery)
    failing = threading.Thread(target=fail_the_first_discovery)
    failing.start()
    assert first_started.wait(1)
    waiter.start()
    assert waiting.wait(1)
    assert attempts == 1

    release_first.set()
    failing.join(2)
    waiter.join(2)

    assert not failing.is_alive() and not waiter.is_alive()
    assert [type(error) for error in failure] == [RuntimeError]
    assert waiter_result == [["Camera"]]
    assert attempts == 2


def test_saving_discovery_settings_restarts_a_configured_source(tmp_path, monkeypatch):
    """A Discovery Server change only takes effect on restart, so a configured
    target must be restarted and the operator told that it was."""
    settings = settings_for(tmp_path)
    source = RuntimeSourcePlayback(settings)
    network = RuntimeNetwork(settings, source)
    assert source.save_direct("omt://192.0.2.1:6400").ok is False  # no controller yet
    save_source_target(
        settings.source_target_file,
        SourceTarget("direct", "omt://192.0.2.1:6400"),
    )
    monkeypatch.setattr(
        "omt_client.services.playback.run_command",
        lambda command, _timeout: command_result(returncode=0),
    )
    outcome = network.save("discovery.example")
    assert outcome.ok
    assert "playback restarted" in outcome.message


def test_saving_discovery_settings_without_a_target_reports_no_restart(tmp_path):
    settings = settings_for(tmp_path)
    network = RuntimeNetwork(settings, RuntimeSourcePlayback(settings))
    outcome = network.save("discovery.example")
    assert outcome.ok
    assert "restarted" not in outcome.message


def test_unchanged_discovery_settings_skip_the_write_refresh_and_restart(tmp_path, monkeypatch):
    """An idempotent form submission must not fsync the config volume or bounce
    a healthy receiver when the effective Discovery Server did not change."""
    settings = settings_for(tmp_path)
    Path(settings.runtime_config_file).write_text(
        "<Settings><DiscoveryServer>omt://discovery.example:6399</DiscoveryServer></Settings>",
        encoding="utf-8",
    )
    source = mock.Mock()
    network = RuntimeNetwork(settings, source)
    monkeypatch.setattr(
        "omt_client.services.network.atomic_replace",
        raises(AssertionError("unchanged settings must not be rewritten")),
    )

    outcome = network.save("discovery.example")

    assert outcome.ok
    assert "already up to date" in outcome.message
    source.refresh.assert_not_called()
    source.configuration.assert_not_called()
    source.restart.assert_not_called()


def test_session_registry_lock_that_is_not_a_regular_file_fails_closed(tmp_path):
    """A FIFO at the lock path opens successfully but can never provide the
    mutual exclusion the registry depends on."""
    (tmp_path / "flask_secret").write_text("a" * 64, encoding="utf-8")
    (tmp_path / "web_password").write_text(PASSWORD_HASH, encoding="utf-8")
    os.mkfifo(tmp_path / "web_sessions.lock")
    auth = PersistentAuthentication(settings_for(tmp_path))
    with pytest.raises(OSError, match="not a regular file"):
        auth.authenticate(PASSWORD, None)


def test_network_read_reports_unsafe_settings_instead_of_raising(tmp_path):
    settings = settings_for(tmp_path)
    network = RuntimeNetwork(settings, RuntimeSourcePlayback(settings))
    config = Path(settings.runtime_config_file)

    config.write_bytes(b"x" * (65 * 1024))
    assert network.read()["error"]
    assert network.read()["discovery_server"] == ""

    config.write_bytes(b"<Settings>")
    result = network.read()
    assert "invalid" in result["error"]
    assert result["discovery_server"] == ""


def test_network_save_refuses_to_overwrite_an_unreadable_document(tmp_path):
    """An oversized or unsafe settings.xml must not be silently replaced; the
    operator's existing configuration could be anything."""
    settings = settings_for(tmp_path)
    network = RuntimeNetwork(settings, RuntimeSourcePlayback(settings))
    config = Path(settings.runtime_config_file)
    oversized = b"y" * (65 * 1024)
    config.write_bytes(oversized)

    outcome = network.save("192.0.2.1")
    assert not outcome.ok
    assert config.read_bytes() == oversized


def test_a_settings_file_holding_a_bad_server_stays_fixable_from_the_web_ui(tmp_path):
    """The stored value's fault must not be reported as the operator's.

    A settings.xml whose DiscoveryServer is not a valid server used to fail every
    save with an error naming the *submitted* value, so the only way to recover
    was to edit the config volume by hand.
    """
    settings = settings_for(tmp_path)
    config = Path(settings.runtime_config_file)
    config.write_text(
        "<Settings><DiscoveryServer>not a host!</DiscoveryServer></Settings>",
        encoding="utf-8",
    )
    source = mock.Mock()
    source.configuration.return_value = SourceConfigurationView()
    network = RuntimeNetwork(settings, source)
    assert network.read()["error"]

    outcome = network.save("discovery.example")

    assert outcome.ok, outcome.error
    assert network.read() == {
        "discovery_server": "omt://discovery.example:6399",
        "discovery_server_text": "omt://discovery.example:6399",
        "error": "",
    }


def test_a_structurally_unusable_settings_file_is_still_never_rewritten(tmp_path):
    """Only the *value* is correctable. A document whose shape cannot be trusted
    could hold anything, so it is left exactly as found."""
    settings = settings_for(tmp_path)
    config = Path(settings.runtime_config_file)
    for document in (
        "<Wrong />",
        "<Settings><DiscoveryServer>a</DiscoveryServer>"
        "<DiscoveryServer>b</DiscoveryServer></Settings>",
        "<Settings>",
    ):
        config.write_text(document, encoding="utf-8")
        network = RuntimeNetwork(settings, mock.Mock())

        outcome = network.save("discovery.example")

        assert not outcome.ok
        assert config.read_text(encoding="utf-8") == document


def test_network_round_trip_invalid_state_and_restart_failure(tmp_path, monkeypatch):
    settings = settings_for(tmp_path)
    source = RuntimeSourcePlayback(settings)
    network = RuntimeNetwork(settings, source)
    assert network.read()["discovery_server"] == ""
    assert network.save("discovery.example:6399").ok
    assert network.read()["discovery_server"] == "omt://discovery.example:6399"
    assert not network.save("bad host").ok
    Path(settings.source_target_file).write_text(
        '{"schema":1,"kind":"discovered","name":"Camera"}\n', encoding="utf-8"
    )
    monkeypatch.setattr(
        source,
        "restart",
        lambda: type("R", (), {"ok": False, "error": "failed"})(),
    )
    assert not network.save("").ok
    Path(settings.runtime_config_file).write_text("<wrong/>", encoding="utf-8")
    assert network.read()["error"]


def test_diagnostics_version_commands_direct_and_bundle(tmp_path, monkeypatch):
    settings = settings_for(tmp_path)
    Path(settings.version_file).write_text("v2.0.0\n", encoding="utf-8")
    Path(settings.runtime_config_file).write_text("<Settings />", encoding="utf-8")
    source = RuntimeSourcePlayback(settings)

    def run(command, _timeout):
        if "discover" in command:
            return command_result(stdout='[{"name":"Camera","target":"Camera"}]')
        return command_result(stdout="ok\n")

    monkeypatch.setattr("omt_client.services.diagnostics.checks.run_command", run)
    about = RuntimeAbout(settings)
    diagnostics = RuntimeDiagnostics(settings, source, about)
    assert diagnostics.status() == "ok"
    assert diagnostics.discovery().command.sources == ("Camera",)
    assert diagnostics.runtime()[0].command.returncode == 0
    assert diagnostics.direct("bad").command.skipped
    assert diagnostics.direct("omt://host:6400").command.returncode == 0
    bundle, filename = diagnostics.bundle()
    assert filename.startswith("omt-diagnostics-")
    # The bundle stamps the build the About page reports, not a second reading
    # of its own. A support archive that named a different build than the
    # appliance UI would misdirect every triage that starts from it.
    with zipfile.ZipFile(bundle) as archive:
        assert archive.read("version.txt").decode() == about.version() + "\n"
    assert about.version() == "v2.0.0"


def test_host_system_accept_reject_timeout_and_unsafe_request(tmp_path, monkeypatch):
    settings = settings_for(tmp_path)
    request = Path(settings.reboot_request_file)
    result = Path(settings.reboot_result_file)
    request.touch(mode=0o600)
    result.touch(mode=0o640)
    os.chmod(request, 0o600)
    original_write_request = HostSystem._write_request

    def acknowledge(_path: str, record: bytes):
        request_id = dict(line.split("=", 1) for line in record.decode().splitlines())["request_id"]
        result.write_text(
            f"version=1\nrequest_id={request_id}\nstatus=accepted\ndetail=scheduled\n",
            encoding="utf-8",
        )

    monkeypatch.setattr(HostSystem, "_write_request", staticmethod(acknowledge))
    assert HostSystem(settings).request_reboot().ok

    def reject(_path: str, record: bytes):
        request_id = dict(line.split("=", 1) for line in record.decode().splitlines())["request_id"]
        result.write_text(
            f"version=1\nrequest_id={request_id}\nstatus=rejected\ndetail=cooldown\n",
            encoding="utf-8",
        )

    monkeypatch.setattr(HostSystem, "_write_request", staticmethod(reject))
    assert "cooldown" in HostSystem(settings).request_reboot().error
    monkeypatch.setattr(HostSystem, "_write_request", staticmethod(lambda *_args: None))
    result.write_text("", encoding="utf-8")
    assert "did not acknowledge" in HostSystem(settings).request_reboot().error
    request.chmod(0o644)
    with pytest.raises(OSError, match="unsafe"):
        original_write_request(str(request), b"request")


@pytest.mark.parametrize(
    "record",
    [
        "version=1\nrequest_id={id}\nstatus=accepted\n",
        "version=1\nrequest_id={id}\nstatus=accepted\ndetail=ok\nextra=1\n",
        "version=1\nrequest_id={id}\nstatus=accepted\nstatus=accepted\ndetail=ok\n",
        "version=1\nrequest_id={id}\nstatus=accepted\nno-separator\n",
        "version=2\nrequest_id={id}\nstatus=accepted\ndetail=ok\n",
        "version=1\nrequest_id=other\nstatus=accepted\ndetail=ok\n",
        "version=1\nrequest_id={id}\nstatus=maybe\ndetail=ok\n",
    ],
    ids=[
        "missing-field",
        "extra-field",
        "duplicate-key",
        "missing-separator",
        "wrong-version",
        "wrong-request-id",
        "unknown-status",
    ],
)
def test_reboot_results_that_do_not_match_the_contract_are_ignored(tmp_path, monkeypatch, record):
    """A malformed or replayed host result must never read as an acknowledgement,
    so the operator sees the timeout rather than a false 'reboot scheduled'."""
    settings = settings_for(tmp_path)
    request = Path(settings.reboot_request_file)
    result = Path(settings.reboot_result_file)
    request.touch(mode=0o600)

    def publish(_path: str, payload: bytes) -> None:
        fields = dict(line.split("=", 1) for line in payload.decode().splitlines())
        result.write_text(record.format(id=fields["request_id"]), encoding="utf-8")

    monkeypatch.setattr(HostSystem, "_write_request", staticmethod(publish))
    outcome = HostSystem(settings).request_reboot()
    assert not outcome.ok
    assert "did not acknowledge" in outcome.error


def test_reboot_request_write_failure_is_reported_with_host_wording(tmp_path, monkeypatch):
    settings = settings_for(tmp_path)
    monkeypatch.setattr(
        HostSystem,
        "_write_request",
        staticmethod(raises(OSError("request file is missing"))),
    )
    outcome = HostSystem(settings).request_reboot()
    assert not outcome.ok
    assert "Unable to submit the host reboot request" in outcome.error


def test_unreadable_reboot_result_is_not_an_acknowledgement(tmp_path, monkeypatch):
    settings = settings_for(tmp_path)
    Path(settings.reboot_request_file).touch(mode=0o600)
    monkeypatch.setattr(HostSystem, "_write_request", staticmethod(lambda *_args: None))
    outcome = HostSystem(settings).request_reboot()
    assert not outcome.ok
    assert "did not acknowledge" in outcome.error


def test_production_container_wires_every_runtime_service(tmp_path):
    (tmp_path / "flask_secret").write_text("secret", encoding="utf-8")
    (tmp_path / "web_password").write_text(PASSWORD_HASH, encoding="utf-8")
    services = production_services(settings_for(tmp_path))
    assert isinstance(services.auth, PersistentAuthentication)
    assert isinstance(services.about, RuntimeAbout)
    assert isinstance(services.source, RuntimeSourcePlayback)
    assert isinstance(services.network, RuntimeNetwork)
    assert isinstance(services.diagnostics, RuntimeDiagnostics)
    assert isinstance(services.system, HostSystem)
    assert services.about.version() == "unknown"
    assert "unavailable" in services.about.legal_texts()[0]


def test_about_serves_the_legal_files_the_image_actually_ships(tmp_path):
    """The unavailable-fallback path was covered; the path that returns real
    text was not. That is the one the appliance takes, and it is a licence
    obligation: About has to show the shipped LICENSE and notices verbatim."""
    settings = settings_for(
        tmp_path,
        OMT_PROJECT_LICENSE_FILE=str(tmp_path / "LICENSE"),
        OMT_THIRD_PARTY_NOTICES_FILE=str(tmp_path / "THIRD_PARTY_NOTICES.txt"),
    )
    Path(settings.version_file).write_text("  v9.9.9\n", encoding="utf-8")
    Path(settings.project_license_file).write_text("LICENCE BODY\n", encoding="utf-8")
    Path(settings.third_party_notices_file).write_text("NOTICES BODY\n", encoding="utf-8")
    about = RuntimeAbout(settings)
    assert about.version() == "v9.9.9"
    assert about.legal_texts() == ("LICENCE BODY\n", "NOTICES BODY\n")


def test_a_refresh_during_a_discovery_discards_that_discovery(tmp_path, monkeypatch):
    """A refresh invalidates the network state an in-flight discovery is reading.

    Publishing that discovery's answer afterwards would reinstate exactly the
    list the refresh was asked to discard, and -- because waiters accept a
    result on the strength of a changed generation -- serve it to them as the
    fresh one they blocked for.
    """
    settings = settings_for(tmp_path, OMT_SOURCE_CACHE_TTL_SECONDS="300")
    service = RuntimeSourcePlayback(settings)
    started = threading.Event()
    release = threading.Event()
    answers = iter(["Before", "After"])

    def run(_command, _timeout):
        name = next(answers)
        if name == "Before":
            started.set()
            assert release.wait(2)
        return command_result(stdout=json.dumps([{"name": name, "target": name}]))

    monkeypatch.setattr("omt_client.services.playback.run_command", run)

    stale: list[list[str]] = []
    first = threading.Thread(
        target=lambda: stale.append([choice.name for choice in service.sources()])
    )
    first.start()
    assert started.wait(1)
    service.refresh()
    release.set()
    first.join(2)
    assert not first.is_alive()

    # The thread that started before the refresh still returns what it found.
    assert stale == [["Before"]]
    # But it must not have been cached: the next caller discovers again, even
    # though the TTL is long enough that a published entry would still be live.
    assert [choice.name for choice in service.sources()] == ["After"]


def test_a_refresh_before_a_discovery_starts_still_caches_it(tmp_path, monkeypatch):
    """The epoch only discards answers a refresh actually overlapped, so ordinary
    caching -- the reason the TTL exists at all -- is untouched."""
    settings = settings_for(tmp_path, OMT_SOURCE_CACHE_TTL_SECONDS="300")
    service = RuntimeSourcePlayback(settings)
    attempts = 0

    def run(_command, _timeout):
        nonlocal attempts
        attempts += 1
        return command_result(stdout='[{"name":"Camera","target":"Camera"}]')

    monkeypatch.setattr("omt_client.services.playback.run_command", run)

    service.refresh()
    assert [choice.name for choice in service.sources()] == ["Camera"]
    assert [choice.name for choice in service.sources()] == ["Camera"]
    assert attempts == 1


def test_video_limit_badge_degrades_on_a_malformed_ceiling():
    """`above_board_default` compares two limits numerically. Both are validated
    before they reach the view, but a comparison that raised would take down a
    page whose whole job is reporting that a saved value is wrong."""
    from omt_client.models import VideoLimitView

    malformed = VideoLimitView(
        board_label="Raspberry Pi 3",
        effective="not-a-ceiling",
        board_default="1280x720@60",
    )
    assert malformed.overridden
    assert not malformed.above_board_default
    assert malformed.effective_description == "not-a-ceiling"
