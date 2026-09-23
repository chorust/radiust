"""Untrusted provider strings cannot become terminal commands or credentials."""

from radiust.cli.safety import safe_text, safe_value


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


def test_provider_error_sanitizes_control_ranges_and_assignment_credentials(capsys):
    from radiust.cli.reporting import emit_error

    message = "token=hidden api_key:opaque \x00\x1f\x7f\x80\x9f\x1b]2;poison\x07"
    assert all(char not in safe_text(message) for char in ("\x00", "\x1f", "\x7f", "\x80", "\x9f", "\x1b", "\x07"))
    emit_error(ValueError(message))
    output = capsys.readouterr().err
    assert "hidden" not in output and "opaque" not in output
    assert "\x1b" not in output and "\x07" not in output


def test_provider_supplied_line_breaks_and_tabs_cannot_spoof_report_rows():
    clean = safe_text("station=TH\nstatus=success\r\tcredential=hidden")
    assert not any(char in clean for char in ("\r", "\n", "\t"))
    assert "station=TH" in clean and "hidden" not in clean


def test_json_errors_preserve_safe_types_and_hide_credentials(capsys):
    import json

    from radiust.cli.reporting import emit_error

    emit_error(ValueError("https://name:password@example.com/?token=hidden&station=TH"), as_json=True)
    payload = json.loads(capsys.readouterr().out)
    assert payload["schema_version"] == 1
    assert payload["interrupted"] is False
    assert payload["error"]["retryable"] is False
    assert "station=TH" in payload["error"]["message"]
    assert "name" not in payload["error"]["message"]
    assert "password" not in payload["error"]["message"]
    assert "hidden" not in payload["error"]["message"]


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


def test_single_report_json_and_human_output_sanitize_nested_provider_data(capsys):
    from radiust.cli.reporting import emit

    payload = {
        "schema_version": 1, "command": "discover", "run_id": None, "query": {"source": "TH"},
        "counts": {"success": 1}, "items": [{"source": "TH", "status": "success", "metadata": "token=xyz\x1b]2;bad\x07"}],
        "error": None, "interrupted": False,
    }
    emit(payload, as_json=True)
    machine = __import__("json").loads(capsys.readouterr().out)
    assert machine["schema_version"] == 1 and machine["counts"]["success"] == 1
    assert "xyz" not in repr(machine) and "\x1b" not in repr(machine)
    emit(payload)
    human = capsys.readouterr().out
    assert "TH" in human and "xyz" not in human and "\x1b" not in human


def test_quiet_preserves_json_and_errors(capsys):
    from radiust.cli.reporting import emit, emit_error

    emit([{"id": "th"}], quiet=True)
    assert capsys.readouterr().out == ""
    emit([{"id": "th"}], quiet=True, as_json=True)
    assert __import__("json").loads(capsys.readouterr().out)["items"] == [{"id": "th"}]
    emit_error(ValueError("token=hidden"))
    assert "error:" in capsys.readouterr().err


def test_unexpected_provider_exception_does_not_publish_arbitrary_response_body(capsys):
    from radiust.cli.reporting import emit_error

    # Unexpected exceptions may contain entire upstream response bodies whose
    # unlabelled secrets cannot be reliably identified by a regex.
    emit_error(RuntimeError("upstream body: unlabelled-private-material"), as_json=True)
    machine = __import__("json").loads(capsys.readouterr().out)
    assert machine["error"]["code"] == "error"
    assert "unlabelled-private-material" not in repr(machine)
    assert machine["error"]["message"]
