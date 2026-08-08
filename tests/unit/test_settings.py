import math
from typing import cast

import pytest

from omt_client.settings import (
    ENVIRONMENT_SPECS,
    RATE_LIMIT_SPECS,
    SettingsError,
    load_settings,
)


def test_settings_defaults_and_diagnostic_output_are_stable():
    settings = load_settings({"HOME": "/state"})
    assert settings.session_lifetime_seconds == 43200
    assert settings.sdk_config_dir == "/etc/omt/omt"
    assert settings.diagnostics_host_budget_seconds == 25
    assert "control_timeout_seconds=8" in settings.diagnostic_lines()


@pytest.mark.parametrize("spec", ENVIRONMENT_SPECS)
@pytest.mark.parametrize("value", ["not-a-number", "nan", "inf", "-inf"])
def test_settings_reject_malformed_and_nonfinite_values(spec, value):
    if spec.value_type == "integer" and value in {"nan", "inf", "-inf"}:
        expected = "integer"
    else:
        expected = "finite" if value in {"nan", "inf", "-inf"} else spec.value_type
    with pytest.raises(SettingsError, match=expected):
        load_settings({spec.name: value})


@pytest.mark.parametrize("spec", ENVIRONMENT_SPECS)
def test_settings_enforce_declared_lower_bound(spec):
    invalid = spec.minimum - 1 if spec.minimum_inclusive else spec.minimum
    with pytest.raises(SettingsError, match=spec.name):
        load_settings({spec.name: str(invalid)})


def test_cache_ttl_allows_zero_and_all_other_defaults_are_finite():
    settings = load_settings({"OMT_SOURCE_CACHE_TTL_SECONDS": "0"})
    assert settings.source_cache_ttl_seconds == 0
    assert all(
        math.isfinite(float(value.split("=", 1)[1]))
        for value in settings.diagnostic_lines()
        if value.split("=", 1)[1].replace(".", "", 1).isdigit()
    )


def test_request_and_login_limits_are_settings_driven():
    """These were hardcoded in factory.py while every other limit came from
    AppSettings, so they could not be tuned or reported in a support bundle."""
    defaults = load_settings({})
    assert defaults.max_request_bytes == 16384
    assert defaults.login_rate_limit == "5 per minute"
    assert "max_request_bytes=16384" in defaults.diagnostic_lines()
    assert "login_rate_limit=5 per minute" in defaults.diagnostic_lines()

    overridden = load_settings(
        {"OMT_MAX_REQUEST_BYTES": "2048", "OMT_LOGIN_RATE_LIMIT": "2 per minute"}
    )
    assert overridden.max_request_bytes == 2048
    assert overridden.login_rate_limit == "2 per minute"


@pytest.mark.parametrize("value", ["1", "true", "YES", " on "])
def test_receive_probe_accepts_documented_true_values(value):
    assert load_settings({"OMT_DIAGNOSTICS_RECEIVE_PROBE": value}).diagnostics_receive_probe


@pytest.mark.parametrize("value", ["0", "false", "NO", " off "])
def test_receive_probe_accepts_documented_false_values(value):
    assert not load_settings({"OMT_DIAGNOSTICS_RECEIVE_PROBE": value}).diagnostics_receive_probe


@pytest.mark.parametrize("value", ["", "enabled", "flase", "2"])
def test_receive_probe_rejects_ambiguous_values(value):
    with pytest.raises(SettingsError, match="OMT_DIAGNOSTICS_RECEIVE_PROBE"):
        load_settings({"OMT_DIAGNOSTICS_RECEIVE_PROBE": value})


def test_receive_probe_rejects_non_text_from_custom_environment_maps():
    malformed = cast(dict[str, str], {"OMT_DIAGNOSTICS_RECEIVE_PROBE": None})
    with pytest.raises(SettingsError, match="must be a boolean"):
        load_settings(malformed)


@pytest.mark.parametrize("spec", RATE_LIMIT_SPECS)
@pytest.mark.parametrize("value", ["", "nonsense", "5 per fortnight", "per minute", "5"])
def test_unparseable_rate_limits_fail_startup_instead_of_serving_unthrottled(spec, value):
    """Flask-Limiter skips a limit string it cannot parse and serves the request,
    so an unvalidated typo here silently removes the throttle -- on the login
    form, that is brute-force protection gone with nothing to notice."""
    with pytest.raises(SettingsError, match=spec.name):
        load_settings({spec.name: value})


@pytest.mark.parametrize("spec", RATE_LIMIT_SPECS)
def test_every_rate_limit_is_overridable_and_reported_in_a_support_bundle(spec):
    """A throttle an operator cannot see is a throttle they cannot diagnose, and
    all four are attached to real endpoints in `factory.create_app`."""
    reported = spec.name.removeprefix("OMT_").lower()
    assert f"{reported}={spec.default}" in load_settings({}).diagnostic_lines()
    assert f"{reported}=7 per hour" in load_settings({spec.name: "7 per hour"}).diagnostic_lines()


def test_the_request_ceiling_cannot_be_set_below_a_usable_form_post():
    with pytest.raises(SettingsError, match="OMT_MAX_REQUEST_BYTES"):
        load_settings({"OMT_MAX_REQUEST_BYTES": "1023"})


def test_playback_status_follows_the_runtime_directory_off_the_config_volume():
    """The shipped image puts per-boot state on a tmpfs, because the receiver
    rewrites the status document continuously and the config volume is
    SD-card-backed flash. The status path has to follow OMT_RUNTIME_DIR there;
    deriving it from OMT_CONFIG_DIR would leave the web worker reading a stale
    file on the volume while the receiver published to tmpfs."""
    default = load_settings({"OMT_CONFIG_DIR": "/etc/omt"})
    assert default.runtime_dir == "/etc/omt/run"
    assert default.playback_status_file == "/etc/omt/run/playback-status.json"

    tmpfs = load_settings({"OMT_CONFIG_DIR": "/etc/omt", "OMT_RUNTIME_DIR": "/run/omt/state"})
    assert tmpfs.runtime_dir == "/run/omt/state"
    assert tmpfs.playback_status_file == "/run/omt/state/playback-status.json"

    # An explicit status path still wins over the directory it would sit in.
    explicit = load_settings(
        {
            "OMT_RUNTIME_DIR": "/run/omt/state",
            "OMT_PLAYBACK_STATUS_FILE": "/elsewhere/status.json",
        }
    )
    assert explicit.playback_status_file == "/elsewhere/status.json"


def test_runtime_override_derives_sdk_directory_unless_explicit():
    derived = load_settings({"OMT_RUNTIME_CONFIG_FILE": "/tmp/sdk/settings.xml"})
    explicit = load_settings(
        {
            "OMT_RUNTIME_CONFIG_FILE": "/tmp/sdk/settings.xml",
            "OMT_STORAGE_PATH": "/tmp/canonical",
        }
    )
    assert derived.sdk_config_dir == "/tmp/sdk"
    assert explicit.sdk_config_dir == "/tmp/canonical"


@pytest.mark.parametrize(
    "name",
    [
        "OMT_DEBUG_DOWNLOAD_LIMIT",
        "OMT_HOST_DEBUG_FILE",
        "PIPELINE_STATUS_STALE_SECONDS",
    ],
)
def test_obsolete_diagnostic_settings_fail_with_migration_guidance(name):
    with pytest.raises(SettingsError, match="Migrate to OMT_DIAGNOSTICS_"):
        load_settings({name: "1"})


def test_bundle_budget_cannot_outlive_the_gunicorn_worker():
    with pytest.raises(SettingsError, match="Gunicorn"):
        load_settings({"OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS": "86"})


def test_host_timeout_cannot_exhaust_the_bundle_budget_alone():
    with pytest.raises(SettingsError, match="must not exceed"):
        load_settings(
            {
                "OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS": "40",
                "OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS": "41",
            }
        )


def test_board_identity_comes_from_the_installer_environment():
    """`deploy/host/install.sh` writes both from the detected board into the
    compose env file. The defaults are the Pi 5 tier so that a missing variable
    never silently degrades an installed appliance."""
    default = load_settings({})
    assert default.board_label == "Raspberry Pi"
    assert default.board_video_ceiling == "1920x1080@60"

    pi4 = load_settings(
        {
            "OMT_BOARD_LABEL": "Raspberry Pi 4 Model B",
            "OMT_VIDEO_CEILING": "1920x1080@30,1280x720@60",
        }
    )
    assert pi4.board_label == "Raspberry Pi 4 Model B"
    assert pi4.board_video_ceiling == "1920x1080@30,1280x720@60"
    # Support bundles must record which board the appliance believed it was.
    assert "board_label=Raspberry Pi 4 Model B" in pi4.diagnostic_lines()
    assert "board_video_ceiling=1920x1080@30,1280x720@60" in pi4.diagnostic_lines()
