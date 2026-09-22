# External provider credential availability

Checked: 2026-09-22. This audit records presence and usability only; no credential values were copied, printed, or added to the new repository.

| Provider | Legacy repository finding | Current test status |
|---|---|---|
| SIDARMA | The old scraper contains a legacy key literal. | A read-only probe using it returned HTTP 403; no current test credential is configured. Skip credentialed live acceptance. See [`id_sidarma.json`](../migration/sources/id_sidarma.json). |
| PAGASA | The old scraper contains a legacy timeline-token literal. | Its bounded provider smoke yielded no verified raw frame. An earlier Chromium/CSRF replay passed against localhost with generated image bytes only; the latest host retry could not start Chromium and the local Act browser job timed out downloading Chromium before running the replay. The credential-free CI job is configured, but hosted execution is unverified. No current test credential is configured; skip credentialed live acceptance. See [`ph.json`](../migration/sources/ph.json) and [`live.json`](live.json). |
| UK DataPoint | The old scraper contains a legacy API-key literal. | The service was retired on 2025-12-01 with no like-for-like Radar Composite replacement; the key was not sent. See [`uk.json`](../migration/sources/uk.json). |
| Weather Underground | No API-key literal was found in the old tile implementation. | No current test credential is configured; the explicit `RADIUST_TEST_WUNDERGROUND_API_KEY` smoke is wired but skips before network access. See [`wunderground.json`](../migration/sources/wunderground.json). |
| BMKG legacy `id` scraper | The old scraper contains a legacy token literal. | The literal was not copied or used; no authorized current token is configured. The new `id` adapter requires runtime configuration and has only synthetic offline replay evidence. See [`id.json`](../migration/sources/id.json). |

AWS S3, S3-compatible, and Aliyun OSS live tests remain deferred at the user's direction. Their credentials and endpoints were not exercised by this audit.

## Legacy checkout presence recheck — 2026-09-22

- Rechecked the checkout pinned by the migration inventory (`8d251601ca551fbd5c05451f1fb337fc4b75362c`) and a second local checkout. Only the pinned checkout contained the target SIDARMA, PAGASA, UK, and tile-source files.
- The pinned checkout has a root `.env`, but no SIDARMA, PAGASA, UK/DataPoint, or Weather Underground credential variable name was present. Credential values were not read into output. The legacy literals above remain the only findings; none is treated as a current usable test credential.
- No new authenticated provider request was made. Existing dated probe results remain authoritative: SIDARMA 403, PAGASA no verified raw frame, UK service retired, and WU credential absent.
