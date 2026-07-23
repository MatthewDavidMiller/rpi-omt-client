"""Typed application services and production OMT runtime adapters."""

from __future__ import annotations

import fcntl
import hashlib
import hmac
import io
import json
import math
import os
import re
import secrets
import stat
import subprocess
import tempfile
import threading
import time
import zipfile
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Protocol

from flask import session
from werkzeug.security import check_password_hash

try:
    from ..discovery import (
        OmtSourceChoice,
        is_valid_direct_target,
        parse_omt_sources,
        parse_source_selection,
    )
    from ..network_config import (
        OmtNetworkConfigurationError,
        empty_settings_xml,
        network_configuration_from_xml,
        normalize_discovery_server,
        update_network_configuration_xml,
    )
    from ..settings import AppSettings, load_settings
    from ..state_store import (
        ReadStatus,
        SourceConfigurationError,
        SourceTarget,
        read_bytes,
        read_source_target,
        read_text,
        save_source_target,
    )
except ImportError:
    from discovery import (  # type: ignore[no-redef]
        OmtSourceChoice,
        is_valid_direct_target,
        parse_omt_sources,
        parse_source_selection,
    )
    from network_config import (  # type: ignore[no-redef]
        OmtNetworkConfigurationError,
        empty_settings_xml,
        network_configuration_from_xml,
        normalize_discovery_server,
        update_network_configuration_xml,
    )
    from settings import AppSettings, load_settings  # type: ignore[no-redef]
    from state_store import (  # type: ignore[no-redef]
        ReadStatus,
        SourceConfigurationError,
        SourceTarget,
        read_bytes,
        read_source_target,
        read_text,
        save_source_target,
    )

from .models import ActionResult, CommandResult, DiagnosticResult, PlaybackSummary

COMMAND_OUTPUT_LIMIT = 256 * 1024
STATUS_FILE_LIMIT = 4096
LEGAL_FILE_LIMIT = 2 * 1024 * 1024
REBOOT_RECORD_LIMIT = 512


class AuthenticationService(Protocol):
    secret_key: str | bytes

    def authenticate(self, password: str, previous_session_id: str | None) -> str | None: ...
    def is_current(self) -> bool: ...
    def revoke(self, session_id: str | None) -> None: ...


class SourcePlaybackService(Protocol):
    def sources(self) -> list[Any]: ...
    def configuration(self) -> tuple[str, str]: ...
    def playback(self) -> PlaybackSummary: ...
    def select(self, selection: str) -> ActionResult: ...
    def refresh(self) -> None: ...
    def restart(self) -> ActionResult: ...
    def clear(self) -> ActionResult: ...
    def save_direct(self, source: str, address: str) -> ActionResult: ...


class NetworkService(Protocol):
    def read(self) -> dict[str, Any]: ...
    def save(self, discovery_server: str) -> ActionResult: ...


class DiagnosticsService(Protocol):
    def version(self) -> str: ...
    def status(self) -> str: ...
    def discovery(self) -> DiagnosticResult: ...
    def runtime(self) -> DiagnosticResult: ...
    def direct(self, source: str, address: str) -> DiagnosticResult: ...
    def bundle(self) -> tuple[Any, str]: ...


class SystemService(Protocol):
    def request_reboot(self) -> ActionResult: ...


@dataclass(frozen=True)
class ServiceContainer:
    auth: AuthenticationService
    source: SourcePlaybackService
    network: NetworkService
    diagnostics: DiagnosticsService
    system: SystemService


class PersistentAuthentication:
    """Password verification and a bounded, cross-worker session registry."""

    _maximum_sessions = 64
    _maximum_registry_bytes = 64 * 1024
    _hash_prefixes = ("scrypt:", "pbkdf2:", "argon2:")

    def __init__(self, settings: AppSettings) -> None:
        self._settings = settings
        self._registry_file = os.path.join(settings.config_dir, "web_sessions.json")
        self._lock_file = os.path.join(settings.config_dir, "web_sessions.lock")
        secret_result = read_text(os.path.join(settings.config_dir, "flask_secret"), 256)
        if not secret_result.ok or not secret_result.text.strip():
            raise RuntimeError("The Flask secret is missing or unsafe.")
        self.secret_key = secret_result.text.strip()
        environment_password = os.environ.get("OMT_WEB_PASSWORD", "")
        if environment_password:
            self._password = environment_password
            self._password_is_hash = False
        else:
            password_result = read_text(settings.password_file, 16 * 1024)
            if not password_result.ok or not password_result.text.strip():
                raise RuntimeError("The Web GUI password file is missing or unsafe.")
            self._password = password_result.text.strip()
            self._password_is_hash = self._password.startswith(self._hash_prefixes)

    @property
    def password_digest(self) -> str:
        return hmac.new(
            self._secret_bytes, self._password.encode("utf-8"), hashlib.sha256
        ).hexdigest()

    @property
    def _secret_bytes(self) -> bytes:
        return self.secret_key.encode("utf-8")

    def _session_digest(self, session_id: str) -> str:
        return hmac.new(
            self._secret_bytes, session_id.encode("utf-8"), hashlib.sha256
        ).hexdigest()

    @contextmanager
    def _locked(self, *, exclusive: bool = False):
        os.makedirs(self._settings.config_dir, mode=0o700, exist_ok=True)
        flags = os.O_RDWR | os.O_CREAT
        flags |= getattr(os, "O_CLOEXEC", 0)
        flags |= getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(self._lock_file, flags, 0o600)
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                raise OSError("session registry lock is not a regular file")
            os.fchmod(descriptor, 0o600)
            fcntl.flock(
                descriptor, fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH
            )
            yield
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
            os.close(descriptor)

    def _read_registry(self) -> dict[str, float]:
        result = read_text(self._registry_file, self._maximum_registry_bytes)
        if result.status is ReadStatus.MISSING:
            return {}
        if not result.ok:
            return {}
        try:
            document = json.loads(result.text)
            raw_sessions = document["sessions"]
            if document["version"] != 1 or not isinstance(raw_sessions, dict):
                return {}
            sessions: dict[str, float] = {}
            for digest, raw_expiry in raw_sessions.items():
                expiry = float(raw_expiry)
                if (
                    not re.fullmatch(r"[0-9a-f]{64}", digest)
                    or isinstance(raw_expiry, bool)
                    or not math.isfinite(expiry)
                ):
                    return {}
                sessions[digest] = expiry
            return sessions
        except (KeyError, TypeError, ValueError, json.JSONDecodeError):
            return {}

    def _write_registry(self, sessions: dict[str, float]) -> None:
        document = json.dumps(
            {"sessions": sessions, "version": 1},
            sort_keys=True,
            separators=(",", ":"),
        ) + "\n"
        directory = self._settings.config_dir
        descriptor, staged = tempfile.mkstemp(
            prefix=".web_sessions.", suffix=".tmp", dir=directory
        )
        try:
            os.fchmod(descriptor, 0o600)
            with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
                descriptor = -1
                stream.write(document)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(staged, self._registry_file)
            staged = ""
            directory_descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(directory_descriptor)
            finally:
                os.close(directory_descriptor)
        finally:
            if descriptor >= 0:
                os.close(descriptor)
            if staged:
                try:
                    os.unlink(staged)
                except FileNotFoundError:
                    pass

    def _verify(self, supplied: str) -> bool:
        if self._password_is_hash:
            try:
                return check_password_hash(self._password, supplied)
            except ValueError:
                return False
        return hmac.compare_digest(
            self._password.encode("utf-8"), supplied.encode("utf-8")
        )

    def authenticate(self, password: str, previous_session_id: str | None) -> str | None:
        if not self._verify(password):
            return None
        session_id = secrets.token_urlsafe(32)
        now = time.time()
        expiry = now + self._settings.session_lifetime_seconds
        with self._locked(exclusive=True):
            sessions = {
                digest: valid_until
                for digest, valid_until in self._read_registry().items()
                if valid_until > now
            }
            if previous_session_id:
                sessions.pop(self._session_digest(previous_session_id), None)
            sessions[self._session_digest(session_id)] = expiry
            while len(sessions) > self._maximum_sessions:
                oldest = min(sessions, key=lambda key: (sessions[key], key))
                sessions.pop(oldest)
            self._write_registry(sessions)
        return session_id

    def is_current(self) -> bool:
        if not session.get("authenticated"):
            return False
        digest = session.get("password_digest")
        session_id = session.get("session_id")
        if not isinstance(digest, str) or not isinstance(session_id, str):
            return False
        if not hmac.compare_digest(digest, self.password_digest):
            return False
        try:
            with self._locked():
                return (
                    self._read_registry().get(self._session_digest(session_id), 0)
                    > time.time()
                )
        except OSError:
            return False

    def revoke(self, session_id: str | None) -> None:
        if not session_id:
            return
        digest = self._session_digest(session_id)
        with self._locked(exclusive=True):
            sessions = self._read_registry()
            if digest in sessions:
                sessions.pop(digest)
                self._write_registry(sessions)


def _run(command: list[str], timeout: float) -> CommandResult:
    started = time.monotonic()
    try:
        completed = subprocess.run(
            command,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=timeout,
            check=False,
            start_new_session=True,
        )
        stdout = completed.stdout[:COMMAND_OUTPUT_LIMIT].decode("utf-8", "replace")
        stderr = completed.stderr[:COMMAND_OUTPUT_LIMIT].decode("utf-8", "replace")
        return CommandResult(
            command=" ".join(command),
            returncode=completed.returncode,
            stdout=stdout,
            stderr=stderr,
            duration_seconds=time.monotonic() - started,
            stdout_truncated=len(completed.stdout) > COMMAND_OUTPUT_LIMIT,
            stderr_truncated=len(completed.stderr) > COMMAND_OUTPUT_LIMIT,
        )
    except subprocess.TimeoutExpired as exc:
        stdout = (exc.stdout or b"")[:COMMAND_OUTPUT_LIMIT].decode("utf-8", "replace")
        stderr = (exc.stderr or b"")[:COMMAND_OUTPUT_LIMIT].decode("utf-8", "replace")
        return CommandResult(
            command=" ".join(command),
            stdout=stdout,
            stderr=stderr,
            duration_seconds=time.monotonic() - started,
            timed_out=True,
            error=f"Command exceeded {timeout:g} seconds.",
        )
    except OSError as exc:
        return CommandResult(
            command=" ".join(command),
            duration_seconds=time.monotonic() - started,
            error=str(exc),
        )


def _atomic_write(path: str, value: bytes, maximum_bytes: int) -> None:
    if len(value) > maximum_bytes:
        raise OSError(f"content exceeds {maximum_bytes} bytes")
    destination = Path(path)
    parent = destination.parent
    if parent.is_symlink() or not parent.is_dir():
        raise OSError("destination directory is unsafe")
    if destination.exists() or destination.is_symlink():
        target_stat = os.lstat(destination)
        if stat.S_ISLNK(target_stat.st_mode) or not stat.S_ISREG(target_stat.st_mode):
            raise OSError("destination is not a regular file")
    temporary = parent / f".{destination.name}.{os.getpid()}.{secrets.token_hex(8)}"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(temporary, flags, 0o600)
    try:
        os.fchmod(descriptor, 0o600)
        view = memoryview(value)
        while view:
            written = os.write(descriptor, view)
            if written <= 0:
                raise OSError("state write made no progress")
            view = view[written:]
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    try:
        os.replace(temporary, destination)
        os.chmod(destination, 0o600, follow_symlinks=False)
        directory_fd = os.open(parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if temporary.exists():
            temporary.unlink()


class RuntimeSourcePlayback:
    def __init__(self, settings: AppSettings) -> None:
        self._settings = settings
        self._cache: tuple[float, list[OmtSourceChoice]] = (0.0, [])
        self._cache_lock = threading.Lock()

    def _target(self) -> SourceTarget | None:
        return read_source_target(self._settings.source_target_file)

    def sources(self) -> list[OmtSourceChoice]:
        now = time.monotonic()
        with self._cache_lock:
            if now < self._cache[0]:
                return list(self._cache[1])
        result = _run(
            [
                self._settings.receiver_command,
                "discover",
                "--wait-ms",
                "1500",
                "--json",
            ],
            max(3.0, self._settings.control_timeout_seconds),
        )
        choices = (
            [OmtSourceChoice(name) for name in parse_omt_sources(result.stdout)]
            if result.returncode == 0
            else []
        )
        with self._cache_lock:
            self._cache = (
                now + self._settings.source_cache_ttl_seconds,
                choices,
            )
        return list(choices)

    def configuration(self) -> tuple[str, str]:
        try:
            target = self._target()
        except SourceConfigurationError:
            return "", ""
        if target is None:
            return "", ""
        return (
            (target.value, "")
            if target.kind == "discovered"
            else (target.value, target.value)
        )

    def _control(self, action: str) -> CommandResult:
        return _run(
            [self._settings.control_command, action],
            self._settings.control_timeout_seconds,
        )

    def _save_and_restart(self, target: SourceTarget, label: str) -> ActionResult:
        try:
            save_source_target(self._settings.source_target_file, target)
        except SourceConfigurationError as exc:
            return ActionResult(False, error=str(exc))
        self.refresh()
        restarted = self._control("restart")
        if restarted.returncode == 0:
            return ActionResult(True, message=f"{label} saved and running.")
        detail = restarted.error or restarted.stderr.strip() or restarted.stdout.strip()
        return ActionResult(
            False,
            error=f"{label} was saved, but playback could not be restarted. {detail}",
        )

    def select(self, selection: str) -> ActionResult:
        parsed = parse_source_selection(selection.strip())
        if not parsed:
            return ActionResult(False, error="Invalid OMT source selection.")
        source, address, backend = parsed
        target = SourceTarget("direct" if address else "discovered", address or source)
        return self._save_and_restart(target, backend)

    def refresh(self) -> None:
        with self._cache_lock:
            self._cache = (0.0, [])

    def restart(self) -> ActionResult:
        if self._target() is None:
            return ActionResult(False, error="No OMT source is configured.")
        result = self._control("restart")
        if result.returncode == 0:
            return ActionResult(True, message="OMT playback restarted.")
        return ActionResult(
            False,
            error=(
                "Unable to restart OMT playback. "
                + (result.error or result.stderr.strip() or result.stdout.strip())
            ),
        )

    def clear(self) -> ActionResult:
        stopped = self._control("stop")
        if stopped.returncode not in (0, 3):
            return ActionResult(
                False,
                error=(
                    "Playback could not be stopped, so the saved target was retained. "
                    + (stopped.error or stopped.stderr.strip() or stopped.stdout.strip())
                ),
            )
        try:
            save_source_target(self._settings.source_target_file, None)
        except SourceConfigurationError as exc:
            return ActionResult(
                False,
                error=f"Playback stopped, but the saved target could not be cleared. {exc}",
            )
        self.refresh()
        return ActionResult(True, message="Playback stopped and the saved target was cleared.")

    def save_direct(self, source: str, address: str) -> ActionResult:
        del source
        if not is_valid_direct_target(address):
            return ActionResult(
                False,
                error="Direct target must use omt://host:port with no path or credentials.",
            )
        return self._save_and_restart(SourceTarget("direct", address), "OMT direct target")

    def playback(self) -> PlaybackSummary:
        source, address = self.configuration()
        if not source:
            return PlaybackSummary(
                "unconfigured",
                "No source configured",
                "Select a discovered source or configure a direct OMT target.",
                "neutral",
            )
        result = read_bytes(self._settings.playback_status_file, STATUS_FILE_LIMIT)
        if not result.ok:
            control = self._control("status")
            if control.returncode == 0:
                return PlaybackSummary(
                    "starting",
                    "Starting playback",
                    "The receiver is running and has not published fresh status yet.",
                    "warning",
                    source,
                    address,
                )
            return PlaybackSummary(
                "stopped",
                "Playback stopped",
                "A target is saved but the receiver is not running.",
                "neutral",
                source,
                address,
            )
        try:
            status = json.loads(result.data)
            updated = datetime.fromisoformat(str(status["updated_at"]).replace("Z", "+00:00"))
            age = (datetime.now(UTC) - updated.astimezone(UTC)).total_seconds()
        except (KeyError, TypeError, ValueError, json.JSONDecodeError):
            status = {}
            age = float("inf")
        if status.get("schema") != 1 or age > self._settings.pipeline_status_stale_seconds:
            return PlaybackSummary(
                "stale",
                "Playback status stale",
                "The receiver status record is unavailable or stale.",
                "warning",
                source,
                address,
            )
        state = str(status.get("state", "failed"))
        mapping = {
            "running": ("playing", "Playing", "success"),
            "waiting-for-hdmi": ("waiting-for-hdmi", "Waiting for HDMI", "warning"),
            "retrying": ("retrying", "Retrying playback", "warning"),
            "degraded": ("degraded", "Playback degraded", "warning"),
            "unsupported-format": (
                "unsupported-format",
                "Unsupported video format",
                "danger",
            ),
            "starting": ("starting", "Starting playback", "warning"),
            "stopped": ("stopped", "Playback stopped", "neutral"),
        }
        public_state, label, tone = mapping.get(
            state, ("failed", "Playback failed", "danger")
        )
        return PlaybackSummary(
            public_state,
            label,
            str(status.get("detail", "Receiver status unavailable.")),
            tone,
            source,
            address,
        )


class RuntimeNetwork:
    def __init__(self, settings: AppSettings, source: RuntimeSourcePlayback) -> None:
        self._settings = settings
        self._source = source

    def read(self) -> dict[str, Any]:
        result = read_bytes(self._settings.runtime_config_file, 64 * 1024)
        if result.status is ReadStatus.MISSING:
            return {"discovery_server": "", "discovery_server_text": "", "error": ""}
        if not result.ok:
            return {
                "discovery_server": "",
                "discovery_server_text": "",
                "error": result.detail or result.status.value,
            }
        try:
            return network_configuration_from_xml(result.data)
        except OmtNetworkConfigurationError as exc:
            return {
                "discovery_server": "",
                "discovery_server_text": "",
                "error": str(exc),
            }

    def save(self, discovery_server: str) -> ActionResult:
        try:
            normalized = normalize_discovery_server(discovery_server)
            result = read_bytes(self._settings.runtime_config_file, 64 * 1024)
            current = empty_settings_xml() if result.status is ReadStatus.MISSING else result.data
            if result.status is not ReadStatus.MISSING and not result.ok:
                raise OmtNetworkConfigurationError(
                    result.detail or result.status.value
                )
            updated = update_network_configuration_xml(current, normalized)
            _atomic_write(self._settings.runtime_config_file, updated, 64 * 1024)
        except (OmtNetworkConfigurationError, OSError) as exc:
            return ActionResult(False, error=str(exc))
        self._source.refresh()
        if self._source.configuration()[0]:
            restarted = self._source.restart()
            if not restarted.ok:
                return ActionResult(
                    False,
                    error=(
                        "Discovery Server was saved, but playback could not be restarted. "
                        f"{restarted.error}"
                    ),
                )
        return ActionResult(
            True,
            message="OMT discovery settings saved"
            + (" and playback restarted." if self._source.configuration()[0] else "."),
        )


class RuntimeDiagnostics:
    def __init__(self, settings: AppSettings, source: RuntimeSourcePlayback) -> None:
        self._settings = settings
        self._source = source

    def version(self) -> str:
        result = read_text(self._settings.version_file, 256)
        return result.text.strip() if result.ok and result.text.strip() else "unknown"

    def status(self) -> str:
        result = _run(
            [self._settings.control_command, "status"],
            self._settings.control_timeout_seconds,
        )
        return result.stdout.strip() or result.error or result.stderr.strip() or "unavailable"

    def discovery(self) -> DiagnosticResult:
        result = _run(
            [
                self._settings.receiver_command,
                "discover",
                "--wait-ms",
                "3000",
                "--json",
            ],
            5,
        )
        return DiagnosticResult(
            "OMT discovery check",
            CommandResult(
                **{
                    **result.__dict__,
                    "sources": tuple(parse_omt_sources(result.stdout)),
                }
            ),
        )

    def runtime(self) -> DiagnosticResult:
        checks = [
            [self._settings.receiver_command, "--version"],
            [self._settings.control_command, "status"],
        ]
        results = [_run(command, 3) for command in checks]
        output = "\n\n".join(
            f"$ {result.command}\n{result.stdout}{result.stderr}" for result in results
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

    def direct(self, source: str, address: str) -> DiagnosticResult:
        del source
        if not is_valid_direct_target(address):
            return DiagnosticResult(
                "Direct-connect check",
                CommandResult(error="Invalid OMT direct target.", skipped=True),
            )
        return DiagnosticResult(
            "Direct-connect check",
            _run(
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

    def bundle(self) -> tuple[io.BytesIO, str]:
        bundle = io.BytesIO()
        timestamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
        with zipfile.ZipFile(bundle, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("version.txt", self.version() + "\n")
            archive.writestr("runtime.txt", self.runtime().command.stdout)
            archive.writestr("discovery.json", self.discovery().command.stdout)
            archive.writestr("controller-status.txt", self.status() + "\n")
            for name, path, limit in (
                ("playback-status.json", self._settings.playback_status_file, 4096),
                ("omt-settings.xml", self._settings.runtime_config_file, 65536),
                ("runtime-sha256.manifest", self._settings.runtime_integrity_manifest, 262144),
                ("host-debug.txt", self._settings.host_debug_file, 2 * 1024 * 1024),
            ):
                result = read_bytes(path, limit)
                archive.writestr(
                    name,
                    result.data
                    if result.ok
                    else f"unavailable: {result.detail or result.status.value}\n",
                )
        bundle.seek(0)
        return bundle, f"omt-debug-{timestamp}.zip"


class HostSystem:
    def __init__(self, settings: AppSettings) -> None:
        self._settings = settings
        self._lock = threading.Lock()

    @staticmethod
    def _write_request(path: str, value: bytes) -> None:
        before = os.lstat(path)
        if (
            not stat.S_ISREG(before.st_mode)
            or stat.S_ISLNK(before.st_mode)
            or stat.S_IMODE(before.st_mode) != 0o600
            or before.st_uid != os.geteuid()
        ):
            raise OSError("host reboot request file has unsafe ownership or mode")
        flags = os.O_WRONLY | os.O_TRUNC
        if hasattr(os, "O_CLOEXEC"):
            flags |= os.O_CLOEXEC
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        descriptor = os.open(path, flags)
        try:
            opened = os.fstat(descriptor)
            if (
                not stat.S_ISREG(opened.st_mode)
                or (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino)
            ):
                raise OSError("host reboot request changed while opening")
            view = memoryview(value)
            while view:
                written = os.write(descriptor, view)
                if written <= 0:
                    raise OSError("host reboot request write made no progress")
                view = view[written:]
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        after = os.lstat(path)
        if (after.st_dev, after.st_ino) != (before.st_dev, before.st_ino):
            raise OSError("host reboot request changed during write")

    @staticmethod
    def _result_for(path: str, request_id: str) -> tuple[str, str] | None:
        result = read_text(path, REBOOT_RECORD_LIMIT)
        if not result.ok:
            return None
        fields: dict[str, str] = {}
        for line in result.text.splitlines():
            key, separator, value = line.partition("=")
            if not separator or key in fields:
                return None
            fields[key] = value
        if set(fields) != {"version", "request_id", "status", "detail"}:
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
                return ActionResult(
                    False, error=f"Unable to submit the host reboot request: {exc}"
                )
            deadline = time.monotonic() + self._settings.reboot_ack_timeout_seconds
            while time.monotonic() < deadline:
                acknowledged = self._result_for(
                    self._settings.reboot_result_file, request_id
                )
                if acknowledged is not None:
                    status, detail = acknowledged
                    if status == "accepted":
                        return ActionResult(
                            True,
                            message="OS reboot scheduled. This appliance will go offline shortly.",
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
                    "Check the omt-client-reboot service journal before retrying."
                ),
            )


def production_services(settings: AppSettings | None = None) -> ServiceContainer:
    effective = settings or load_settings()
    source = RuntimeSourcePlayback(effective)
    return ServiceContainer(
        auth=PersistentAuthentication(effective),
        source=source,
        network=RuntimeNetwork(effective, source),
        diagnostics=RuntimeDiagnostics(effective, source),
        system=HostSystem(effective),
    )


def controller_pid(status: str) -> int | None:
    match = re.match(r"^running:([1-9][0-9]*)(?:\s|$)", status.strip())
    return int(match.group(1)) if match else None
