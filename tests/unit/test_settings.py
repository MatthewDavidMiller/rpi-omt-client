import math

import pytest
from settings import ENVIRONMENT_SPECS, SettingsError, load_settings


def test_settings_defaults_and_debug_output_are_stable():
    settings = load_settings({"HOME": "/state"})
    assert settings.session_lifetime_seconds == 43200
    assert settings.sdk_config_dir == "/etc/omt/omt"
    assert settings.host_debug_budget_seconds == 25
    assert "control_timeout_seconds=8" in settings.debug_lines()


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
        for value in settings.debug_lines()
        if value.split("=", 1)[1].replace(".", "", 1).isdigit()
    )


def test_runtime_override_derives_sdk_directory_unless_explicit():
    derived = load_settings({"OMT_RUNTIME_CONFIG_FILE": "/tmp/sdk/settings.xml"})
    explicit = load_settings({
        "OMT_RUNTIME_CONFIG_FILE": "/tmp/sdk/settings.xml",
        "OMT_STORAGE_PATH": "/tmp/canonical",
    })
    assert derived.sdk_config_dir == "/tmp/sdk"
    assert explicit.sdk_config_dir == "/tmp/canonical"
