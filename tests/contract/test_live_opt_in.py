from tests.support.pytest_policy import opt_in_skip_reasons


def test_live_tests_are_skipped_without_explicit_opt_in() -> None:
    reasons = opt_in_skip_reasons({"live"}, allow_live=False, allow_provider=False)

    assert reasons == ["set RADIUST_TEST_ALLOW_LIVE=1 to run live-source tests"]


def test_provider_tests_require_live_and_provider_opt_ins() -> None:
    markers = {"live", "provider"}

    assert opt_in_skip_reasons(markers, allow_live=False, allow_provider=False) == [
        "set RADIUST_TEST_ALLOW_LIVE=1 to run live-source tests",
        "set RADIUST_TEST_ALLOW_PROVIDER=1 to run credentialed provider tests",
    ]
    assert opt_in_skip_reasons(markers, allow_live=True, allow_provider=False) == [
        "set RADIUST_TEST_ALLOW_PROVIDER=1 to run credentialed provider tests"
    ]
    assert opt_in_skip_reasons(markers, allow_live=True, allow_provider=True) == []
