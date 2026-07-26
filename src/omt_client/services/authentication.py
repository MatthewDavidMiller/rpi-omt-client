"""Persistent password authentication and bounded session storage."""

from __future__ import annotations

import fcntl
import hashlib
import hmac
import json
import math
import os
import re
import secrets
import stat
import time
from collections.abc import Iterator
from contextlib import contextmanager

from flask import session
from werkzeug.security import check_password_hash

from ..json_document import JsonDocumentError, load_json_document
from ..safe_io import atomic_replace, read_text
from ..settings import AppSettings


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
            if self._password_is_hash:
                self._require_supported_hash()

    def _require_supported_hash(self) -> None:
        """Fail startup on a hash Werkzeug cannot verify.

        Werkzeug raises for an unsupported method (`argon2:` is recognised as a
        hash prefix here but rejected there) and for structurally broken
        parameters. Left to `_verify`, both collapse into a plain "Invalid
        password", locking every operator out with no diagnosable cause.
        """
        try:
            check_password_hash(self._password, "")
        except ValueError as exc:
            raise RuntimeError(
                f"The Web GUI password hash uses an unsupported format: {exc}"
            ) from exc

    @property
    def password_digest(self) -> str:
        return hmac.new(
            self._secret_bytes, self._password.encode("utf-8"), hashlib.sha256
        ).hexdigest()

    @property
    def _secret_bytes(self) -> bytes:
        return self.secret_key.encode("utf-8")

    def _session_digest(self, session_id: str) -> str:
        return hmac.new(self._secret_bytes, session_id.encode("utf-8"), hashlib.sha256).hexdigest()

    @contextmanager
    def _locked(self, *, exclusive: bool = False) -> Iterator[None]:
        os.makedirs(self._settings.config_dir, mode=0o700, exist_ok=True)
        flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(self._lock_file, flags, 0o600)
        locked = False
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                raise OSError("session registry lock is not a regular file")
            os.fchmod(descriptor, 0o600)
            fcntl.flock(descriptor, fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH)
            locked = True
            yield
        finally:
            if locked:
                fcntl.flock(descriptor, fcntl.LOCK_UN)
            os.close(descriptor)

    def _read_registry(self) -> dict[str, float]:
        result = read_text(self._registry_file, self._maximum_registry_bytes)
        if not result.ok:
            return {}
        try:
            document = load_json_document(result.data)
            if (
                not isinstance(document, dict)
                or set(document) != {"version", "sessions"}
                or type(document.get("version")) is not int
                or document.get("version") != 1
            ):
                return {}
            raw_sessions = document.get("sessions")
            if not isinstance(raw_sessions, dict):
                return {}
            sessions: dict[str, float] = {}
            for digest, raw_expiry in raw_sessions.items():
                # JSON object keys are always strings, so only the shape needs
                # checking. Validate before converting, so a bool -- which is a
                # float subclass -- cannot slip through as an expiry. Skip a bad
                # row instead of discarding the whole registry: one corrupt entry
                # must not log every other operator out on the next successful
                # login rewrite.
                if not re.fullmatch(r"[0-9a-f]{64}", digest) or isinstance(raw_expiry, bool):
                    continue
                try:
                    expiry = float(raw_expiry)
                except (OverflowError, TypeError, ValueError):
                    continue
                if not math.isfinite(expiry):
                    continue
                sessions[digest] = expiry
            return sessions
        except (JsonDocumentError, TypeError, ValueError):
            return {}

    def _write_registry(self, sessions: dict[str, float]) -> None:
        document = (
            json.dumps(
                {"sessions": sessions, "version": 1},
                sort_keys=True,
                separators=(",", ":"),
            )
            + "\n"
        ).encode()
        atomic_replace(
            self._registry_file,
            document,
            self._maximum_registry_bytes,
        )

    def _verify(self, supplied: str) -> bool:
        if self._password_is_hash:
            # _require_supported_hash already proved this hash verifies without
            # raising. Werkzeug's failure modes depend on the stored method and
            # parameters, not on the supplied password, so no guard is needed.
            return check_password_hash(self._password, supplied)
        return hmac.compare_digest(self._password.encode("utf-8"), supplied.encode("utf-8"))

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
                return self._read_registry().get(self._session_digest(session_id), 0) > time.time()
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
