"""Validated environment configuration for the Flask application."""

from __future__ import annotations

import math
import os
from collections.abc import Mapping
from dataclasses import dataclass


class SettingsError(ValueError):
    """Raised when a runtime setting is malformed or outside its contract."""


@dataclass(frozen=True)
class EnvironmentSpec:
    """Document one public numeric setting and its accepted values."""

    name: str
    default: str
    value_type: str
    minimum: float
    minimum_inclusive: bool = True


ENVIRONMENT_SPECS = (
    EnvironmentSpec("OMT_SESSION_LIFETIME_SECONDS", "43200", "integer", 1),
    EnvironmentSpec("OMT_CONTROL_TIMEOUT_SECONDS", "8", "number", 0, False),
    EnvironmentSpec("OMT_SOURCE_CACHE_TTL_SECONDS", "5", "number", 0),
    EnvironmentSpec("OMT_PLAYBACK_STATUS_STALE_SECONDS", "5", "integer", 1),
    EnvironmentSpec("OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS", "30", "number", 0, False),
    # Host oneshot budget (install.sh systemd Environment=). Mirrored here so
    # support bundles report the expected host ceiling; changing the container
    # env alone does not reconfigure the host unit.
    EnvironmentSpec("OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS", "25", "integer", 1),
    EnvironmentSpec("OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS", "60", "number", 0, False),
    EnvironmentSpec("OMT_REBOOT_ACK_TIMEOUT_SECONDS", "3", "number", 0, False),
    EnvironmentSpec("OMT_MAX_REQUEST_BYTES", "16384", "integer", 1024),
)


def _parse_number(environment: Mapping[str, str], spec: EnvironmentSpec) -> float | int:
    raw = environment.get(spec.name, spec.default)
    try:
        value = int(raw) if spec.value_type == "integer" else float(raw)
    except (TypeError, ValueError) as exc:
        raise SettingsError(f"{spec.name} must be a {spec.value_type}; received {raw!r}") from exc
    if isinstance(value, float) and not math.isfinite(value):
        raise SettingsError(f"{spec.name} must be finite; received {raw!r}")
    valid = value >= spec.minimum if spec.minimum_inclusive else value > spec.minimum
    if not valid:
        relation = "at least" if spec.minimum_inclusive else "greater than"
        raise SettingsError(f"{spec.name} must be {relation} {spec.minimum:g}; received {raw!r}")
    return value


def _enabled(value: str) -> bool:
    return value.lower() not in {"0", "false", "no", "off"}


@dataclass(frozen=True)
class AppSettings:
    """Effective non-secret application settings."""

    config_dir: str
    runtime_dir: str
    password_file: str
    session_lifetime_seconds: int
    max_request_bytes: int
    login_rate_limit: str
    control_command: str
    receiver_command: str
    control_timeout_seconds: float
    source_cache_ttl_seconds: float
    source_target_file: str
    playback_status_file: str
    sdk_config_dir: str
    runtime_config_file: str
    playback_status_stale_seconds: int
    diagnostics_host_report_file: str
    diagnostics_host_request_file: str
    diagnostics_host_pcap_file: str
    diagnostics_host_pcap_metadata_file: str
    diagnostics_host_timeout_seconds: float
    diagnostics_host_budget_seconds: int
    diagnostics_bundle_budget_seconds: float
    diagnostics_receive_probe: bool
    diagnostics_download_limit: str
    diagnostics_action_limit: str
    version_file: str
    runtime_integrity_manifest: str
    project_license_file: str
    third_party_notices_file: str
    reboot_request_file: str
    reboot_result_file: str
    reboot_ack_timeout_seconds: float
    reboot_action_limit: str

    def diagnostic_lines(self) -> list[str]:
        """Return a stable, secret-free representation for support bundles."""
        return [
            f"session_lifetime_seconds={self.session_lifetime_seconds}",
            f"max_request_bytes={self.max_request_bytes}",
            f"login_rate_limit={self.login_rate_limit}",
            f"control_timeout_seconds={self.control_timeout_seconds:g}",
            f"source_cache_ttl_seconds={self.source_cache_ttl_seconds:g}",
            f"playback_status_stale_seconds={self.playback_status_stale_seconds}",
            f"diagnostics_host_timeout_seconds={self.diagnostics_host_timeout_seconds:g}",
            f"diagnostics_host_budget_seconds={self.diagnostics_host_budget_seconds}",
            f"diagnostics_bundle_budget_seconds={self.diagnostics_bundle_budget_seconds:g}",
            f"diagnostics_receive_probe_enabled={self.diagnostics_receive_probe}",
            f"sdk_config_dir={self.sdk_config_dir}",
            f"runtime_config_file={self.runtime_config_file}",
            f"reboot_ack_timeout_seconds={self.reboot_ack_timeout_seconds:g}",
        ]


def load_settings(environment: Mapping[str, str] | None = None) -> AppSettings:
    """Parse the environment once and reject ambiguous timeout behavior."""
    env = os.environ if environment is None else environment
    obsolete = sorted(
        name
        for name in env
        if name == "PIPELINE_STATUS_STALE_SECONDS"
        or name.startswith("OMT_DEBUG_")
        or name.startswith("OMT_HOST_DEBUG_")
    )
    if obsolete:
        raise SettingsError(
            "Obsolete diagnostics settings are not supported: "
            + ", ".join(obsolete)
            + ". Migrate to OMT_DIAGNOSTICS_* and "
            "OMT_PLAYBACK_STATUS_STALE_SECONDS."
        )
    parsed = {spec.name: _parse_number(env, spec) for spec in ENVIRONMENT_SPECS}
    config_dir = env.get("OMT_CONFIG_DIR", "/etc/omt")
    # Ephemeral per-boot state (lock, PID record, published status). The shipped
    # image points this at a tmpfs, because the status file is rewritten
    # continuously and the config volume is SD-card-backed flash. The fallback
    # keeps a bare `docker run` without that mount working.
    runtime_dir = env.get("OMT_RUNTIME_DIR", os.path.join(config_dir, "run"))
    runtime_override = env.get("OMT_RUNTIME_CONFIG_FILE")
    sdk_override = env.get("OMT_STORAGE_PATH")
    if runtime_override:
        runtime_file = runtime_override
        sdk_dir = sdk_override or os.path.dirname(runtime_file)
    else:
        sdk_dir = sdk_override or os.path.join(config_dir, "omt")
        runtime_file = os.path.join(sdk_dir, "settings.xml")
    return AppSettings(
        config_dir=config_dir,
        runtime_dir=runtime_dir,
        password_file=env.get("OMT_PASSWORD_FILE", os.path.join(config_dir, "web_password")),
        session_lifetime_seconds=int(parsed["OMT_SESSION_LIFETIME_SECONDS"]),
        max_request_bytes=int(parsed["OMT_MAX_REQUEST_BYTES"]),
        login_rate_limit=env.get("OMT_LOGIN_RATE_LIMIT", "5 per minute"),
        control_command=env.get("OMT_CONTROL_COMMAND", "/usr/local/bin/control-omt.sh"),
        receiver_command=env.get("OMT_RECEIVER_COMMAND", "/usr/local/bin/omt-receiver"),
        control_timeout_seconds=float(parsed["OMT_CONTROL_TIMEOUT_SECONDS"]),
        source_cache_ttl_seconds=float(parsed["OMT_SOURCE_CACHE_TTL_SECONDS"]),
        source_target_file=env.get(
            "OMT_SOURCE_TARGET_FILE", os.path.join(config_dir, "source_target.json")
        ),
        playback_status_file=env.get(
            "OMT_PLAYBACK_STATUS_FILE", os.path.join(runtime_dir, "playback-status.json")
        ),
        sdk_config_dir=sdk_dir,
        runtime_config_file=runtime_file,
        playback_status_stale_seconds=int(parsed["OMT_PLAYBACK_STATUS_STALE_SECONDS"]),
        diagnostics_host_report_file=env.get(
            "OMT_DIAGNOSTICS_HOST_REPORT_FILE",
            "/host-diagnostics/host-report.txt",
        ),
        diagnostics_host_request_file=env.get(
            "OMT_DIAGNOSTICS_HOST_REQUEST_FILE",
            "/host-diagnostics/request",
        ),
        diagnostics_host_pcap_file=env.get(
            "OMT_DIAGNOSTICS_HOST_PCAP_FILE",
            "/host-diagnostics/host-network.pcap",
        ),
        diagnostics_host_pcap_metadata_file=env.get(
            "OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE",
            "/host-diagnostics/host-network-pcap.txt",
        ),
        diagnostics_host_timeout_seconds=float(parsed["OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS"]),
        diagnostics_host_budget_seconds=int(parsed["OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS"]),
        diagnostics_bundle_budget_seconds=float(parsed["OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS"]),
        diagnostics_receive_probe=_enabled(env.get("OMT_DIAGNOSTICS_RECEIVE_PROBE", "1")),
        diagnostics_download_limit=env.get("OMT_DIAGNOSTICS_DOWNLOAD_LIMIT", "10 per hour"),
        diagnostics_action_limit=env.get("OMT_DIAGNOSTICS_ACTION_LIMIT", "30 per hour"),
        version_file=env.get("RPI_OMT_CLIENT_VERSION_FILE", "/app/RPI_OMT_CLIENT_VERSION"),
        runtime_integrity_manifest=env.get(
            "OMT_RUNTIME_INTEGRITY_MANIFEST", "/app/runtime-sha256.manifest"
        ),
        project_license_file=env.get("OMT_PROJECT_LICENSE_FILE", "/app/legal/LICENSE"),
        third_party_notices_file=env.get(
            "OMT_THIRD_PARTY_NOTICES_FILE", "/app/legal/THIRD_PARTY_NOTICES.txt"
        ),
        reboot_request_file=env.get("OMT_REBOOT_REQUEST_FILE", "/host-actions/reboot.request"),
        reboot_result_file=env.get("OMT_REBOOT_RESULT_FILE", "/host-actions/reboot.result"),
        reboot_ack_timeout_seconds=float(parsed["OMT_REBOOT_ACK_TIMEOUT_SECONDS"]),
        reboot_action_limit=env.get("OMT_REBOOT_ACTION_LIMIT", "3 per hour"),
    )
