"""Untrusted provider strings cannot become terminal commands or credentials."""

from radiust.safety import safe_text, safe_value


def test_nested_value_preserves_types_and_removes_credentials():
    payload = {"frames": [{"source": "th", "count": 2, "token": "secret", "message": "authorization: Bearer abcdef\x1b]0;attack\x07"}], "valid": True}
    result = safe_value(payload)
    assert result["frames"][0]["count"] == 2
    assert result["valid"] is True
    assert "secret" not in repr(result)
    assert "abcdef" not in repr(result)
    assert "\x1b" not in repr(result)
    assert "\x07" not in repr(result)
    assert "th" in repr(result)


def test_url_userinfo_and_query_auth_are_redacted_without_losing_identity():
    result = safe_text("https://bob:pwd@example.org/radar.png?token=abcdef&station=th\x1b[2J")
    assert "example.org/radar.png" in result
    assert "station=th" in result
    assert "bob" not in result and "pwd" not in result and "abcdef" not in result
    assert "\x1b" not in result


def test_discovery_status_fields_remain_numeric():
    value = safe_value({"counts": {"missing_credentials": 2, "network_restricted": 3}, "secret_key": "private"})
    assert value["counts"] == {"missing_credentials": 2, "network_restricted": 3}
    assert value["secret_key"] == "[REDACTED]"


def test_safe_text_sanitizes_control_ranges_and_assignment_credentials():
    message = "token=hidden api_key:opaque \x00\x1f\x7f\x80\x9f\x1b]2;poison\x07"
    clean = safe_text(message)
    assert all(char not in clean for char in ("\x00", "\x1f", "\x7f", "\x80", "\x9f", "\x1b", "\x07"))
    assert "hidden" not in clean and "opaque" not in clean


def test_provider_supplied_line_breaks_and_tabs_cannot_spoof_report_rows():
    clean = safe_text("station=TH\nstatus=success\r\tcredential=hidden")
    assert not any(char in clean for char in ("\r", "\n", "\t"))
    assert "station=TH" in clean and "hidden" not in clean


def test_common_provider_secret_variants_and_query_keys_are_redacted():
    message = (
        "access_token=one refresh_token=two secret_key=three "
        "X-Api-Key: four https://user:pass@example.org/radar?access_token=five&station=TH "
        "credential=six"
    )
    sanitized = safe_text(message)
    for secret in ("one", "two", "three", "four", "five", "six", "user", "pass"):
        assert secret not in sanitized.split("example.org")[0], secret
    assert "example.org/radar" in sanitized
    assert "station=TH" in sanitized
    assert "[REDACTED]" in sanitized


def test_nested_secret_aliases_do_not_change_nonsecret_json_types():
    original = {
        "source": "TH", "count": 5, "enabled": False, "age": None,
        "metadata": {"access_token": "one", "x-api-key": "two", "refresh_token": "three", "fields": [1, None, True]},
    }
    clean = safe_value(original)
    assert clean["metadata"]["fields"] == [1, None, True]
    assert (clean["source"], clean["count"], clean["enabled"], clean["age"]) == ("TH", 5, False, None)
    assert all(clean["metadata"][key] == "[REDACTED]" for key in ("access_token", "x-api-key", "refresh_token"))


def test_public_discovery_item_sanitizes_without_legacy_cli_module():
    from radiust.models import DiscoveryItem, DiscoveryTarget

    item = DiscoveryItem(
        DiscoveryTarget("ph", "composite", None),
        "missing_credentials",
        error={"message": "token=private-value"},
    )
    assert item.as_dict()["error"]["message"] == "token=[REDACTED]"
