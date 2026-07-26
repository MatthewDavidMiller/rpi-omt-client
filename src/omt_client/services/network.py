"""OMT discovery-network configuration service."""

from __future__ import annotations

from typing import Any

from ..models import ActionResult
from ..network_config import (
    OmtNetworkConfigurationError,
    empty_settings_xml,
    network_configuration_from_xml,
    normalize_discovery_server,
    update_network_configuration_xml,
)
from ..safe_io import ReadStatus, atomic_replace, read_bytes
from ..settings import AppSettings
from .protocols import SourcePlaybackService

SETTINGS_XML_LIMIT = 64 * 1024


class RuntimeNetwork:
    def __init__(self, settings: AppSettings, source: SourcePlaybackService) -> None:
        self._settings = settings
        self._source = source

    def read(self) -> dict[str, Any]:
        result = read_bytes(self._settings.runtime_config_file, SETTINGS_XML_LIMIT)
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
            result = read_bytes(self._settings.runtime_config_file, SETTINGS_XML_LIMIT)
            current = empty_settings_xml() if result.status is ReadStatus.MISSING else result.data
            if result.status is not ReadStatus.MISSING and not result.ok:
                raise OmtNetworkConfigurationError(result.detail or result.status.value)
            if result.ok:
                existing = network_configuration_from_xml(current)
                if existing["discovery_server"] == normalized:
                    return ActionResult(
                        True,
                        message="OMT discovery settings are already up to date.",
                    )
            updated = update_network_configuration_xml(current, normalized)
            atomic_replace(self._settings.runtime_config_file, updated, SETTINGS_XML_LIMIT)
        except (OmtNetworkConfigurationError, OSError) as exc:
            return ActionResult(False, error=str(exc))
        self._source.refresh()
        configured = bool(self._source.configuration()[0])
        if configured:
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
            + (" and playback restarted." if configured else "."),
        )
