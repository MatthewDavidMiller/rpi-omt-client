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
    EnvironmentSpec("PIPELINE_STATUS_STALE_SECONDS", "5", "integer", 1),
    EnvironmentSpec("OMT_HOST_DEBUG_TIMEOUT_SECONDS", "30", "number", 0, False),
    EnvironmentSpec("OMT_HOST_DEBUG_BUDGET_SECONDS", "25", "integer", 1),
    EnvironmentSpec("OMT_DEBUG_BUNDLE_BUDGET_SECONDS", "60", "number", 0, False),
    EnvironmentSpec("OMT_REBOOT_ACK_TIMEOUT_SECONDS", "3", "number", 0, False),
)


def _parse_number(environment: Mapping[str, str], spec: EnvironmentSpec) -> float | int:
    raw = environment.get(spec.name, spec.default)
    try:
        value = int(raw) if spec.value_type == "integer" else float(raw)
    except (TypeError, ValueError) as exc:
        raise SettingsError(
            f"{spec.name} must be a {spec.value_type}; received {raw!r}"
        ) from exc
    if isinstance(value, float) and not math.isfinite(value):
        raise SettingsError(f"{spec.name} must be finite; received {raw!r}")
    valid = value >= spec.minimum if spec.minimum_inclusive else value > spec.minimum
    if not valid:
        relation = "at least" if spec.minimum_inclusive else "greater than"
        raise SettingsError(
            f"{spec.name} must be {relation} {spec.minimum:g}; received {raw!r}"
        )
    return value


def _enabled(value: str) -> bool:
    return value.lower() not in {"0", "false", "no", "off"}


@dataclass(frozen=True)
class AppSettings:
    """Effective non-secret application settings."""

    config_dir: str
    password_file: str
    session_lifetime_seconds: int
    control_command: str
    receiver_command: str
    control_timeout_seconds: float
    source_cache_ttl_seconds: float
    source_target_file: str
    playback_status_file: str
    sdk_config_dir: str
    runtime_config_file: str
    pipeline_status_stale_seconds: int
    host_debug_file: str
    host_debug_request_file: str
    host_debug_pcap_file: str
    host_debug_pcap_metadata_file: str
    host_debug_timeout_seconds: float
    host_debug_budget_seconds: int
    debug_bundle_budget_seconds: float
    debug_receive_probe: bool
    debug_download_limit: str
    debug_action_limit: str
    version_file: str
    runtime_integrity_manifest: str
    project_license_file: str
    third_party_notices_file: str
    reboot_request_file: str
    reboot_result_file: str
    reboot_ack_timeout_seconds: float
    reboot_action_limit: str

    def debug_lines(self) -> list[str]:
        """Return a stable, secret-free representation for support bundles."""
        return [
            f"session_lifetime_seconds={self.session_lifetime_seconds}",
            f"control_timeout_seconds={self.control_timeout_seconds:g}",
            f"source_cache_ttl_seconds={self.source_cache_ttl_seconds:g}",
            f"pipeline_status_stale_seconds={self.pipeline_status_stale_seconds}",
            f"host_debug_timeout_seconds={self.host_debug_timeout_seconds:g}",
            f"host_debug_budget_seconds={self.host_debug_budget_seconds}",
            f"debug_bundle_budget_seconds={self.debug_bundle_budget_seconds:g}",
            f"debug_receive_probe_enabled={self.debug_receive_probe}",
            f"sdk_config_dir={self.sdk_config_dir}",
            f"runtime_config_file={self.runtime_config_file}",
            f"reboot_ack_timeout_seconds={self.reboot_ack_timeout_seconds:g}",
        ]


def load_settings(environment: Mapping[str, str] | None = None) -> AppSettings:
    """Parse the environment once and reject ambiguous timeout behavior."""
    env = os.environ if environment is None else environment
    parsed = {spec.name: _parse_number(env, spec) for spec in ENVIRONMENT_SPECS}
    config_dir = env.get("OMT_CONFIG_DIR", "/etc/omt")
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
        password_file=env.get("OMT_PASSWORD_FILE", os.path.join(config_dir, "web_password")),
        session_lifetime_seconds=int(parsed["OMT_SESSION_LIFETIME_SECONDS"]),
        control_command=env.get("OMT_CONTROL_COMMAND", "/usr/local/bin/control-omt.sh"),
        receiver_command=env.get("OMT_RECEIVER_COMMAND", "/usr/local/bin/omt-receiver"),
        control_timeout_seconds=float(parsed["OMT_CONTROL_TIMEOUT_SECONDS"]),
        source_cache_ttl_seconds=float(parsed["OMT_SOURCE_CACHE_TTL_SECONDS"]),
        source_target_file=env.get(
            "OMT_SOURCE_TARGET_FILE", os.path.join(config_dir, "source_target.json")
        ),
        playback_status_file=env.get(
            "OMT_PLAYBACK_STATUS_FILE", os.path.join(config_dir, "run", "playback-status.json")
        ),
        sdk_config_dir=sdk_dir,
        runtime_config_file=runtime_file,
        pipeline_status_stale_seconds=int(parsed["PIPELINE_STATUS_STALE_SECONDS"]),
        host_debug_file=env.get("OMT_HOST_DEBUG_FILE", "/host-debug/host-debug.txt"),
        host_debug_request_file=env.get("OMT_HOST_DEBUG_REQUEST_FILE", "/host-debug/request"),
        host_debug_pcap_file=env.get("OMT_HOST_DEBUG_PCAP_FILE", "/host-debug/host-network.pcap"),
        host_debug_pcap_metadata_file=env.get(
            "OMT_HOST_DEBUG_PCAP_METADATA_FILE", "/host-debug/host-network-pcap.txt"
        ),
        host_debug_timeout_seconds=float(parsed["OMT_HOST_DEBUG_TIMEOUT_SECONDS"]),
        host_debug_budget_seconds=int(parsed["OMT_HOST_DEBUG_BUDGET_SECONDS"]),
        debug_bundle_budget_seconds=float(parsed["OMT_DEBUG_BUNDLE_BUDGET_SECONDS"]),
        debug_receive_probe=_enabled(env.get("OMT_DEBUG_RECEIVE_PROBE", "1")),
        debug_download_limit=env.get("OMT_DEBUG_DOWNLOAD_LIMIT", "10 per hour"),
        debug_action_limit=env.get("OMT_DEBUG_ACTION_LIMIT", "30 per hour"),
        version_file=env.get("RPI_OMT_CLIENT_VERSION_FILE", "/app/RPI_OMT_CLIENT_VERSION"),
        runtime_integrity_manifest=env.get(
            "OMT_RUNTIME_INTEGRITY_MANIFEST", "/app/runtime-sha256.manifest"
        ),
        project_license_file=env.get("OMT_PROJECT_LICENSE_FILE", "/app/legal/LICENSE"),
        third_party_notices_file=env.get(
            "OMT_THIRD_PARTY_NOTICES_FILE", "/app/legal/THIRD_PARTY_NOTICES.txt"
        ),
        reboot_request_file=env.get(
            "OMT_REBOOT_REQUEST_FILE", "/host-actions/reboot.request"
        ),
        reboot_result_file=env.get(
            "OMT_REBOOT_RESULT_FILE", "/host-actions/reboot.result"
        ),
        reboot_ack_timeout_seconds=float(parsed["OMT_REBOOT_ACK_TIMEOUT_SECONDS"]),
        reboot_action_limit=env.get("OMT_REBOOT_ACTION_LIMIT", "3 per hour"),
    )
