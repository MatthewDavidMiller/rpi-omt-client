"""Production OMT service-boundary tests."""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest
from conftest import VirtualClock, raises
from flask import Flask, session
from werkzeug.security import generate_password_hash

from omt_client.models import CommandResult
from omt_client.safe_io import atomic_replace
from omt_client.services import (
    HostSystem,
    PersistentAuthentication,
    RuntimeDiagnostics,
    RuntimeNetwork,
    RuntimeSourcePlayback,
    production_services,
)
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
    assert service.configuration() == ("Camera", "")
    assert service.restart().ok
    assert not service.save_direct("host:6400").ok
    assert service.save_direct("omt://192.0.2.1:6400").ok
    assert service.configuration() == (
        "omt://192.0.2.1:6400",
        "omt://192.0.2.1:6400",
    )
    assert service.clear().ok
    assert service.configuration() == ("", "")
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

    monkeypatch.setattr("omt_client.services.diagnostics.run_command", run)
    diagnostics = RuntimeDiagnostics(settings, source)
    assert diagnostics.version() == "v2.0.0"
    assert diagnostics.status() == "ok"
    assert diagnostics.discovery().command.sources == ("Camera",)
    assert diagnostics.runtime().command.returncode == 0
    assert diagnostics.direct("bad").command.skipped
    assert diagnostics.direct("omt://host:6400").command.returncode == 0
    bundle, filename = diagnostics.bundle()
    assert filename.startswith("omt-diagnostics-")
    assert bundle.read(2) == b"PK"


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
    assert isinstance(services.source, RuntimeSourcePlayback)
    assert isinstance(services.network, RuntimeNetwork)
    assert isinstance(services.diagnostics, RuntimeDiagnostics)
    assert isinstance(services.system, HostSystem)
