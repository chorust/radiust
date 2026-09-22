# Live provider tests

Live provider checks are opt-in. The default test suite uses replay fixtures and
does not send credentialed requests. Keep values in the process environment or
the CI secret store; do not put them in a config file, command history, fixture,
or issue comment.

| Provider | Runtime configuration | Live test secret | Status in this checkout |
| --- | --- | --- | --- |
| SIDARMA | `RADIUST_SOURCES__ID_SIDARMA__API_KEY` | `RADIUST_TEST_ID_SIDARMA_API_KEY` | No usable current credential; the legacy value returned HTTP 403. |
| PAGASA | `RADIUST_SOURCES__PH__TIMELINE_TOKEN` | `RADIUST_TEST_PH_TIMELINE_TOKEN` | No verified current token/raw frame. |
| Weather Underground | `RADIUST_SOURCES__WUNDERGROUND__API_KEY` | `RADIUST_TEST_WUNDERGROUND_API_KEY` | No legacy key or current test credential was found. |
| UK DataPoint | None | None | Retired on 2025-12-01; the adapter fails before network access. |
| BMKG legacy `id` | `RADIUST_SOURCES__ID__TOKEN` | None | No authorized current token was found. |

For a directly configured local CLI run, enable the network policy and inject
the source credential through the environment. The shell below only names the
variable; its value must be supplied by the local secret manager:

```bash
export RADIUST_RUNTIME__ALLOW_NETWORK=true
export RADIUST_SOURCES__ID_SIDARMA__API_KEY='<value from local secret manager>'
uv run radiust download id_sidarma --latest --raw-only --output ./data --json
```

For the bounded provider smoke, set the test gate and the matching test
variable, then run only the provider-marked cases:

```bash
RADIUST_TEST_ALLOW_LIVE=1 \
RADIUST_TEST_ALLOW_PROVIDER=1 \
uv run pytest tests/live/test_representative_sources.py -m 'live and provider' -vv
```

Without a matching test variable each credentialed case skips before any
provider request. The same names are wired into the manual `provider` job in
`.github/workflows/live.yml` as `RADIUST_TEST_ID_SIDARMA_API_KEY`,
`RADIUST_TEST_PH_TIMELINE_TOKEN`, and
`RADIUST_TEST_WUNDERGROUND_API_KEY` GitHub Actions secrets. The workflow is
manual so public and credentialed traffic cannot start on an ordinary push.

Local output is the current acceptance path. AWS S3, S3-compatible, and
Aliyun OSS provider tests remain deferred; their matrix is not implied by a
local filesystem pass.
