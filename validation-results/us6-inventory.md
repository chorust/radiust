# User Story 6 inventory validation

Status: **open with explicit blockers**

- `migration/inventory.json` contains all 24 HEAD source paths and two historical Brazil lines. The catalog and HEAD inventory contain the same 24 source IDs.
- Every HEAD source has a source adapter, versioned resource, migration record, fixture manifest, and contract-test coverage. Sixteen manifests contain canonical frame artifacts; the ID manifest also retains one supplementary legacy raw payload, while eight source manifests remain explicitly `blocked` with `frames: []`, evidence, and blockers.
- The contract suite validates manifest metadata, artifact existence, SHA-256 values, byte counts, reference dimensions, and science reference fields. The latest focused My/CA/source-matrix/inventory run passed `43` tests, including `tests/contract/test_migration_inventory.py`; the full source/inventory test set also passes in the default suite.
- `rainviewer` and `sg` now have decoded reference values and native-grid evidence. RainViewer maps its Web Mercator tiles to EPSG:4326; SG decodes NEA's official five ordinal intensity groups on the declared 240 km AEQD grid. SG does not claim numeric mm/h, and transparent pixels inside range remain explicitly below-detection/no-visible-return because the provider cannot distinguish them from unobserved returns. Other sources retain their source-specific palette, geometry, frame-binding, or credential blockers.
- The AU live CLI run wrote 61 latest raw station frames with zero failures. The latest opt-in public smoke passed 17 cases (15 catalog raw acquisitions, Taiwan's numeric-grid decode, and the historical-only BR SIPAM acquisition; two credentialed provider cases were deselected). The provider SIDARMA smoke remains unverified because `RADIUST_TEST_ID_SIDARMA_API_KEY` is not configured.
- The geometry audit found no real curvilinear source payload. Historical `br_cptec` has a replay-tested legacy/WMS adapter and one hashed 2018 WMS PNG; its latest advertised frame still returns a provider CRS/mosaic error, and scientific palette/pixel geometry remain blocked. `br_sipam` has an acquisition adapter and raw fixture, with science and geometry blockers recorded in its history entry.

## SIDARMA legacy raw recovery — 2026-09-21

- Recovered `ID-ICUSRC-MAJ_20251230075900_merca_r_source.png` from the old repository's ignored `output/id/` tree and retained it at `tests/fixtures/sources/id/raw/`. The 84,756-byte PNG hashes to `2c82b22aee6fb8891169cbc32f4d821b27c9dbc311fecfe71a5446ff0f558df3` and decodes as 4084×4084 RGBA.
- The legacy `post_process()` path wrote `crawled_data["content"]` to the `*_source.*` artifact. The output does not preserve the SIDARMA discovery JSON or referenced payload URL; the old client disabled TLS verification. The station and timestamp are filename-derived and were not independently checked against provider metadata.
- This is supplementary legacy evidence, not a canonical fixture frame. `id` remains blocked on authenticated live acquisition, trustworthy time binding, scientific palette and native geometry; T101 remains open.

No blocked source is counted as scientifically migrated. The inventory is ready for handoff and further evidence collection, but it is not a release-closure record.

The 2026-09-21 registered-adapter replay covers all 16 raw-backed source IDs. Fifteen sources have discover/acquire replay through SDK and CLI `--raw-only`; MY has synthetic adapter/CLI replay plus a separate live raw-only CLI run that wrote both stations and deleted the temporary payloads after hash/format verification. The MY endpoints advertise `image/gif` while returning PNG bytes, which the adapter now records and validates explicitly. No licensed canonical raw, accepted time binding, or native geometry is present. Replay and live raw acquisition do not establish scientific decoding for sources whose palettes and physical grids remain unverified; see `validation-results/live-cli-my.json`.

The CAM adapter now sends the minimal legacy slideshow `Accept`, `Accept-Language`, and `User-Agent` headers. A no-credential live CLI raw-only run wrote one JPEG for station `ZIWR03` at `2026-09-21T04:15:04Z` (78,539 bytes; SHA-256 recorded in `migration/sources/cam.json`); the source test and opt-in live test pass. The image's local-time label agrees with the filename timestamp. The artifact remains temporary because no fixture reuse basis is documented, and its SRI legend and native map transform are not yet a verified decoder or georeference.

## Per-source status

`contract_passed` below means the source-specific offline test is present and passed in the full Python run. It does not imply that the pixels have an accepted physical decoding or native georeference. Every row has an adapter, resource, migration record and test; all blockers remain unaccepted.

| Source | Status labels | Fixture | Open evidence gap |
| --- | --- | --- | --- |
| au | implemented; contract_passed; exception (blocking) | hashed raw | palette and native geometry/control points |
| bmkg | implemented; contract_passed; exception (blocking) | blocked, no raw | provider error; IDCOMP semantics and native extent |
| ca | live acquisition passed; contract_passed; exception (blocking) | hashed raw | palette bins and raster projection/control points |
| cam | implemented; acquisition_verified; contract_passed; exception (blocking) | blocked, no retained raw | temporary live capture succeeded after restoring legacy headers; reuse basis, SRI palette bins, and native map transform remain unverified |
| es | implemented; contract_passed; exception (blocking) | hashed raw | EPSG:3857 claim lacks control-point validation |
| fr | implemented; contract_passed; exception (blocking) | hashed raw | WMS request grid checked; radar-native geometry and scientific palette unverified |
| id | implemented; contract_passed; exception (blocking) | blocked; supplementary legacy raw, not a frame | discovery response/URL, TLS-authenticated receipt, time binding, palette and geometry |
| id_sidarma | implemented; contract_passed; exception (blocking) | blocked, no raw | provider key and current raw image |
| kr | implemented; contract_passed; exception (blocking) | hashed raw | native image geometry and palette |
| my | live raw acquisition passed; contract_passed; exception (blocking) | hashed legacy display images | reuse permission, accepted time binding, palette and native geometry |
| nz | implemented; contract_passed; exception (blocking) | hashed raw | provider native extent and value mapping |
| opensnow | implemented; contract_passed; exception (blocking) | blocked, no raw | provider error; no physical tile semantics |
| ph | implemented; contract_passed; exception (blocking) | blocked, no raw | authorized timeline/raw, provider valid-time binding, palette and native geometry (local Chromium localhost replay passes; provider smoke remains unverified) |
| pt | implemented; contract_passed; exception (blocking) | hashed raw | native extent and scientific luminance mapping |
| rainviewer | implemented; contract_passed | hashed raw | no current source-specific geometry blocker; strongest scientific reference |
| sg | implemented; contract_passed; ordinal-only | hashed raw | no quantitative mm/h mapping; transparent in-range pixels cannot distinguish below-threshold from unobserved |
| th | implemented; contract_passed; exception (blocking) | hashed raw | frame time, native extent and palette |
| th_royalrain | implemented; contract_passed; exception (blocking) | hashed raw | native extent and palette |
| tw | implemented; contract_passed; exception (blocking) | hashed raw | native control points and palette |
| tw-http | implemented; contract_passed; exception (blocking) | hashed raw | native grid control points and palette |
| uk | retired adapter; no-network contract passed; retirement exception (accepted) | blocked, no raw | DataPoint retired 2025-12-01; no like-for-like Radar Composite replacement |
| vn | implemented; contract_passed; exception (blocking) | hashed raw | native extent and palette |
| windy | implemented; contract_passed; exception (blocking) | hashed raw tiles | provider frame-time binding and green-channel semantics |
| wunderground | implemented; contract_passed; exception (blocking) | blocked, no raw | API key, tile semantics and frame identity |

Historical lines: `br_cptec` is implemented and contract-passed for both its recovered legacy API and current WMS discovery. It retains one official 512×512 WMS PNG, while latest-frame acquisition still fails and scientific palette/pixel geometry remain blocked. `br_sipam` is implemented and acquisition-contract-passed with a hashed raw PNG, but remains blocked on the physical dBZ palette and pixel control points. Their evidence is recorded in `migration/history/`.

## MY adapter and terminal/CI recheck — 2026-09-21

- `tests/sources/test_my.py` validates both HEAD endpoints, strict Last-Modified parsing, the unverified legacy time rule, the observed GIF-header/PNG-payload case, raw byte preservation, and rejection of unexpected formats. The source CLI matrix also exercises both stations through a controlled transcript.
- The live CLI run succeeded for both stations with TLS verification enabled. It recorded exact Last-Modified values, PNG dimensions, byte counts, and hashes before deleting temporary outputs. It does not supply reuse rights, accepted time binding, palette semantics, or native geometry; T011/T038 remain open.
- The old checkout's ignored `output/my/` directory contains two additional `*_merca_r_source.gif` names whose bytes are PNGs (826×640 and 1113×650). Their SHA-256 values, visible product/projection labels, and conflicting filename/display times are recorded in `migration/blockers/my.md`. They remain diagnostic rendered outputs: no provider response headers, licensed reuse basis, or pixel control points were found, so they were not promoted into fixtures.
- Actual CLI `--latest --raw-only --no-cache --json` smoke passed for 15 public sources: CAM, AU, RainViewer, FR, tw-http, ES, PT, SG, TH, KR, NZ, VN, Windy, CA, and Royal Rain. All wrote a complete manifest; every artifact path, byte count, and SHA-256 was verified before temporary output cleanup. The CA and Royal Rain station-filter recheck reduced CA discovery from 39.59 to 3.57 seconds and changed Royal Rain from a 130-second timeout to a verified single-station write in 43.66 seconds. Exact per-frame evidence is in `validation-results/live-cli-public.json`, and each source migration record references that report. The initial TH smoke recorded retrieval time; the later footer-OCR correction in the Thailand section below supersedes it. Raw-only CLI success does not verify scientific meaning.
- Local terminal/CLI contract subset passed 17 tests, including a real POSIX PTY CLI process. The matching offline and terminal-contract GitHub Actions jobs are configured; hosted CI has not been dispatched. Real terminal emulator, SSH/tmux, visual layout and interrupt recovery remain open under T128/T129.

## T086 contract recheck — 2026-09-20

- The inventory, source-manifest, adapter CLI matrix, and RainViewer source replay checks passed **49 tests** together.
- The manifest contract now requires every retained frame to state product, station (including explicit `null` for global coverage), timezone-aware valid time, locator version, revision, metadata, and unique hashed artifacts. Artifact paths must remain within the fixture directory; declared byte counts are checked.
- Scientific references must either include shape, dtype, units, grid CRS/model, quality encoding, and reference pixels, or carry an explicit `blocked:` reason with raw representation metadata. All eight unavailable HEAD sources remain explicit blocked rows with evidence and blockers.
- RainViewer's independent manifest now records `float32` dBZ, EPSG:4326 Web Mercator pixel-center grid, `uint16` quality codes, and its 10 dBZ reference pixel. The replayed adapter output is checked against those fields. This closes T086's contract-harness task; it does not close T116's source-by-source evidence gate.

## T116 inventory contract recheck — 2026-09-20

- `uv run pytest -q tests/sources tests/contract/test_migration_inventory.py tests/integration/test_source_cli_matrix.py` passed **88 tests**. This reruns every source-specific offline contract, the full migration inventory, and the shared source CLI matrix.
- All 24 HEAD source rows are inventoried and contract-tested; the two recovered Brazil history rows have explicit disposition records. The 16 raw-backed fixtures pass hash/size/path validation, and the eight no-raw sources remain explicit blocked exceptions.
- The per-source table above retains `implemented`, `contract_passed`, and blocking `exception` labels. Here `contract_passed` describes adapter/offline metadata contracts; it does not assert physical palette or native-grid correctness.
- T116's inventory execution and status-reporting task is complete. Source migration closure remains blocked by the listed unaccepted evidence gaps; this report does not mark those exceptions accepted.

## Current adapter and terminal recheck — 2026-09-21

- All 24 public source IDs still have adapters, resources, source tests, migration records, and fixture manifests. The latest opt-in public live matrix passes **17 tests, with 2 credentialed provider cases deselected**: 15 public-catalog raw acquisitions, Taiwan's numeric-grid decode, and historical-only BR SIPAM acquisition. The default `uv run pytest -q` passes **306 tests with 24 expected skips**; its 20 live cases skip before network access. The two SIDARMA/PAGASA provider cases remain unverified.
- Actual CLI `--latest --raw-only --no-cache` runs now pass for 15 public sources: CAM, AU, RainViewer, FR, tw-http, ES, PT, SG, TH, KR, NZ, VN, Windy, CA, and Royal Rain. Each wrote one frame; every output manifest and raw artifact path, size, and SHA-256 verified before temporary cleanup. CA/CASFT discovery fell from 39.59 to 3.57 seconds after filtering station directories before traversal; Royal Rain/takhli changed from a timeout to a verified write in 43.66 seconds. See `validation-results/live-cli-public.json`.
- OpenSnow and BMKG remain blocked by the separately recorded HTTP 403 responses. Raw acquisition does not resolve palette, physical-value, or native-geometry gaps. Current source-specific hashes and blockers remain recorded in `migration/sources/` and `validation-results/live.json`.
- Fresh local contracts: **100 source/inventory tests** and **17 terminal/renderer/CLI tests** passed; all three workflow YAML files parsed. Ruff, Rust formatting/tests, and scoped JSON checks also passed in the current verification series. Hosted CI and visual Kitty/iTerm2/SSH/tmux plus Ctrl-C recovery remain unrun/unaccepted.
- The local ANSI PTY and protocol tests do not constitute visual Kitty/iTerm2/SSH/tmux or Ctrl-C acceptance. The terminal matrix records the desktop safety denial and unavailable local Docker daemon; hosted CI has not run.

These runs verify additional acquisition paths and local contracts. Source-specific evidence and limitations are recorded per source; the remaining unaccepted source, credential, provider, terminal, and hosted-CI exceptions still block release.

## Source-adapter task acceptance — 2026-09-21

- Focused offline source, inventory, and CLI-matrix contracts passed **56 tests**; Ruff passed for the seven source modules and tests. The Rust FTP contract passed **8 tests**, including transient listing retry within the common frame budget. A loopback integration test also observed the production HTTP transport's actual request headers and confirmed no Brotli encoding is advertised.
- T092/T093/T095/T097/T098/T100/T103 are complete for their stated source-discovery, provider-time/station binding, and raw-preservation scopes. Current raw-only live CLI evidence covers CA/CASFT, Royal Rain/takhli, NZ/NZAU2, VN/DHA, and AU/AU02; TW-HTTP/CV1_3600 is recorded in the same public CLI report, while the anonymous-S3 TW fixture binds its PNG to the official companion JSON time.
- These task closures do not assert unverified pixel palettes or native image grids. The blockers remain recorded per source and in the US2 scientific gate; T011/T038/T049, credentialed sources, and the remaining source migrations are still open.
- After the loopback Brotli regression was added, the default offline suite passed **308 tests with 24 expected skips**; Ruff, Rust formatting, and workflow YAML parsing passed. No live/provider opt-in was enabled. There are **29 unchecked tasks** remaining across source evidence, credentials, terminal visual acceptance, hosted CI, the user-deferred object-storage matrix, and downstream release gates.

## Singapore categorical adapter acceptance — 2026-09-21

- The retained 2026-09-18 10:45 SGT raw frame is hash-checked and was compared with the official NEA data.gov.sg API image for the exact timestamp. Both are 480×480 with identical alpha masks and zero differing visible pixels; only RGB bytes under alpha zero differ.
- The API specification supplies 33 exact colors grouped into Light, Light to Moderate, Moderate, Moderate to Heavy, and Heavy. `sg` now emits an ordinal `rain_intensity` field on a 1 km spherical AEQD grid, masks pixels outside the nominal 240 km radius, and treats a missing frame as `NoDataError`. It does not expose unsupported dBZ/mm/h values or interpret transparent pixels as measured 0 mm/h.
- The API declares AEQD center and EPSG:4326 bounds. A 6,371,000 m sphere maps those bounds to the raster edge within 20 m, less than one pixel; the inferred radius is marked as such. The palette, raw/API visible-pixel comparison, reference pixels, and limits are in `python/radiust/resources/palettes/sg_rain_intensity.json`, `tests/fixtures/sources/sg/fixture.json`, and `migration/sources/sg.json`.
- `T099` is checked as an ordinal adapter contract. The source still cannot distinguish dry pixels from below-threshold or unobserved transparent returns inside nominal coverage, and it supplies no quantitative rainfall rate.

## Thailand live-GIF adapter evidence — 2026-09-21

- The registered adapter keeps TMD `cmp1` and `kkn240Loop` as separate stations. Offline tests OCR every retained frame footer, assert the latest observation time and all six KKN timestamps, reject historical queries and stale `max_age` results, and fail closed if Tesseract is missing or the GIF changes between separate discovery/acquisition operations.
- Real raw-only CLI runs passed for both current endpoints. `cmp1` reported footer time `2023-04-22T14:30:00Z` (158,551 bytes; SHA-256 `55c279de79da7f45acefe2fbff1a03faa1e15b0dadd186d2c2d8b7a93451999e`); `kkn240Loop` reported `2023-04-22T16:00:05Z` (2,666,961 bytes; SHA-256 `98b6e087ddf5ff01e139f862409eae65c288d47e44603d67e3e0e224b49cb5cd`). Both images display 2023 observations despite retrieval in 2026. `max_age` now sees the actual old observation time rather than retrieval time.
- Query-driven SDK/CLI downloads share one operation context and write the same GIF that discovery OCR inspected. Standalone `discover` followed by later `acquire` refetches the mutable endpoint and rejects a changed SHA-256. The official Khon Kaen operations document describes a 15-minute UTC schedule, 240 km range, 1 km gate width, and GIF/KML output at 0.5° elevation under `/KKN/240`; the indexed palette and pixel control points remain unverified, so scientific decode stays fail-closed.
- T104's raw, latest-only and observation-time work is implemented. Its task remains open behind its M1/M0 dependencies; exact palette and native pixel geometry are also unverified.

## Portugal timeline adapter evidence — 2026-09-21

- The live IPMA timeline's exact 2026-09-18 image path was reacquired on 2026-09-21; its bytes and SHA-256 match the retained 1500×1526 PNG. The official dynamic page labels the product rainfall intensity in mm/h, states that reference times are UTC, and links the retained legend.
- The official `mapbuilder-md.js` applies the same [-19.87034, 30.01467, -12.89697, 35.96053] EPSG:4326 bounds to every Madeira timeline PNG. The adapter derives pixel centers from those bounds and the actual image dimensions; the source test checks all four first/last center controls.
- The retained image's colors match the official legend ramp within 9.5 RGB levels. The adapter now emits ordinal rainfall-intensity legend intervals (unit 1), not reconstructed continuous mm/h values or the legacy luminance-to-dBZ transform. Transparent pixels mean no displayed return with below-detection quality; opaque black remains missing; unsupported colors fail closed.
- `tests/sources/test_pt.py` passes its source contract, including four category reference colors, geometry controls, quality flags, and an unknown-color rejection case. T105's source scope is implemented and recorded as contract-passed; the global T065 gate remains open for other representative source contracts.

## Final adapter-registry and CLI contract audit — 2026-09-21

- Parsed the catalog and static registry mapping: all **24/24** public source IDs resolve to an adapter module, test module, source resource, and migration record, including the hyphenated `tw-http` → `tw_http` module mapping.
- `uv run pytest -q tests/contract/test_migration_inventory.py tests/integration/test_source_cli_matrix.py` passed **38 tests**; the source CLI matrix alone passed **34** after adding the retained TMD `kkn240Loop` registered-adapter path. This verifies offline manifest/CLI behavior; it does not waive source-specific requirements for canonical raw data, reuse basis, provider credentials, time binding, palettes, or native geometry.
- At this inventory checkpoint, **126 tasks were complete / 28 open**. Source tasks whose own acceptance text requires missing external evidence remained unchecked; public acquisition-only evidence was recorded without claiming scientific validation.

## Thailand retained-loop regression and full-suite recheck — 2026-09-21

- `uv run pytest -q tests/sources/test_th.py tests/contract/test_migration_inventory.py`: **9 passed**. The replay asserts original bytes, six-frame count and durations, footer-bound UTC observation times, live-only query behavior, and fail-closed decoding.
- The latest `uv run pytest -q` run passed **314 tests with 25 expected skips** and one existing NumPy ABI warning. `uv run ruff check tests/sources/test_th.py`, fixture/migration JSON parsing, and `git diff --check` passed.
- At this checkpoint the retained raw established the KKN GIF acquisition path; later footer-OCR evidence below binds all frame times. Palette, native grid, and the upstream MY-dependent US1 gate remain unresolved.

## KMA native-data alternative audit — 2026-09-21

- KMA's official API Hub documents a separate 500 m, five-minute numeric composite, `ECHO` values scaled as dBZ×100, explicit no-echo/outside-area sentinels, and authenticated grid-coordinate resources. An issued `authKey` is required; this product is not the retained station CGI image.
- Direct inspection of the retained `KWK` CGI fixture shows a Gwanaksan HSR precipitation title, an mm/h legend with visible labels from 0.0 to 150, and a map overlay containing 50/100/150/200 km range rings. This is presentation evidence only: exact swatch thresholds, data/background mask, and pixel-to-grid transform are still unknown.
- The official training figure's dBZ scale is a different product example and does not establish the station CGI palette. No numerical decoder or native grid was inferred from either image legend.
- T091 remains open pending a source-matched palette and native-geometry reference. The audit is recorded in [`migration/sources/kr.json`](../migration/sources/kr.json); no credential was copied or used.

## France display-only adapter closure — 2026-09-21

- Added `tests/sources/test_fr.py::test_fr_retained_wms_frame_matches_references_and_stays_display_only`. It verifies the retained 700×600 RGBA WMS artifact's SHA-256, all manifest reference pixels, the provider frame time in the WMS request, exact downloaded bytes, and intentional `DecodeError` instead of an unsupported luminance-to-dBZ conversion.
- The focused source/inventory/CLI run passed **43 tests**, and the full suite passed **316 tests with 25 expected skips**. Ruff and `git diff --check` pass.
- T063 is complete for its FRCOMP page/WMS adapter migration and the fail-closed scientific correction. This does not assert physical dBZ values or the radar-native measurement grid; both remain open under T065. The FR portion of T053 has its retained raw fixture, while T053 stays open for SIDARMA's missing authorized raw.
- The task list is now **127 complete / 27 open**. No SIDARMA token was used.

## AEMET official-documentation and precise live recheck — 2026-09-21

- AEMET's official interpretation page documents the national low-PPI reflectivity mosaic as displayed in EPSG:4326 and says the GeoTIFF `ESCALA` metadata maps RGBA colors to meteorological intervals. It does not bind the retained `imagen-radar/compo/*_3857.png` sample to an exact pixel extent or scale table; the filename suffix remains an unverified projection claim. The migration record keeps scientific decoding disabled.
- The exact ES public latest-raw test passed (**1 passed**) and closed the temporary `RawFrame` after hash/size checks. It validates current discovery/acquisition, not palette or geometry.
- A broader public-smoke rerun was accidentally selected because `-k ... and es` matches `es` inside `test`; it completed **15 public cases**, with one AU FTP NLST timeout and four other cases deselected. The prior same-day AU smoke passed. This is recorded as a transient failure, not a retirement or a source-task closure. Credentialed SIDARMA/PAGASA cases were not selected or used.

## PT decoded live CLI acceptance — 2026-09-21

- The offline registered-source CLI matrix passed **34 tests**; the PT source/inventory subset passed **6 tests**. The integration matrix now checks PT text cat for variable rain_intensity, shape (1526, 1500), and unit 1.
- The explicit public PT latest-frame smoke passed **1 test**. A real opt-in CLI download wrote the 2026-09-21T13:15Z PTST2 frame as NetCDF; CLI cat read back the categorical field and 1526×1500 dimensions. The current frame had no colored returns: all visible pixels are below-detection and 487 opaque-black pixels are missing. This is not evidence of quantitative zero rainfall. Details and file hash are in [live-cli-pt.json](live-cli-pt.json).
- T105 is complete. Task count is **128 complete / 26 open**; T065 remains open for the other representative source contracts.

## Local-first CLI and packaged-resource recheck — 2026-09-21

- `uv run pytest -q tests/integration/test_source_cli_matrix.py`: **34 passed**. It exercises all 24 catalog listings and the registered/raw-backed source paths; blocked manifests remain explicitly listed and are not counted as provider passes. T147 now closes the local non-exception fixture matrix only. Its dependency on T085 was removed because real object-store tests are independent and explicitly deferred by the user; T084/T085 remain open.
- Built a CPython 3.12 macOS arm64 wheel from the current worktree (SHA-256 `8347dfd759fc25353cef9a43f9eaa2f1661e51d81abc819ef3bd72807ca694a0`). Clean child-venv `core` and `all` installed-wheel smokes both passed outside the source tree. The `all` smoke read back NetCDF, GeoTIFF, and Zarr data/quality arrays and both selections confirmed the new PT ordinal palette is packaged.
- `.github/workflows/offline.yml` exposes a push/PR `terminal-contract` job and `.github/workflows/wheels.yml` contains the declared wheel/extra matrix. Hosted Actions has not run from this uncommitted workspace. The current local wheel pass does not close T150's Linux and remaining architecture/matrix coverage.
- At this checkpoint the task count was **131 complete / 23 open**; the overall release gate remained blocked by source evidence, provider, and platform gaps.

## Current adapter, terminal, and CI verification — 2026-09-21

- The final local Python suite passed **322 tests with 25 explicit skips**. The source CLI matrix passed **35**, TH + matrix + migration-inventory focus passed **48**, and terminal/rendering/CLI/PTy contracts passed **24**. Ruff and all **24 Rust integration tests** passed.
- The registered inventory maps all **24 current catalog sources** and both historical Brazil adapters. Raw-backed adapter replay is covered where canonical fixtures exist; missing or retired provider cases remain explicit exceptions and are not reported as successful acquisition.
- TMD `cmp1` and `kkn240Loop` now bind valid time to the exact GIF footer text. The two current public endpoints returned 2023 frames; each CLI raw write matched the inspected payload hash, and `max_age` rejects the stale observations. Palette and native geometry remain open.
- The current CPython 3.12 macOS arm64 wheel passed clean external `core` and `all` smokes. SHA-256: `80b16d1b1a5c67815e634383243dbc13d2f3b0d7c1089f490e5bc85041d57cb8`.
- The Ubuntu 24.04 arm64 local Act run installed Tesseract, then was cancelled after 22 minutes in locked dependency synchronization before lint/test steps. This is not a CI pass; the offline and terminal workflows parse, host-equivalent tests pass, and the user-selected GitHub Actions terminal path is configured. No hosted workflow ran.
- T154 now has a complete per-FR/SC evidence report with a blocked release/archive conclusion. **132 tasks are checked / 22 remain open.** S3/S3-compatible/OSS are deferred by the user; missing/invalid SIDARMA/PAGASA/WU credentials are skipped, and UK DataPoint is retired. See [`provider-credential-audit.md`](provider-credential-audit.md).
## Registered adapter/resource contract recheck — 2026-09-22

- `tests/contract/test_migration_inventory.py` now checks every HEAD inventory row for a registered adapter module, a versioned source resource carrying the matching source ID, a source-specific test module, and a registry entry with the same ID.
- The check initially found that `my.json` exposed only `id`; adding the required `source` field restored the resource contract. The inventory contract now passes **5 tests**, Ruff passes, and `git diff --check` passes.
- This closes a local catalog/registration consistency defect. It does not close the external raw, permission, time, palette, geometry, credential, or provider acceptance gates listed above.

## Public adapter live raw recheck — 2026-09-22

- The explicit no-credential public smoke passed **16 cases in 89.89 seconds**. It exercised all `public_latest_raw_smoke` parameter cases and verified discover/acquire, artifact hashes/sizes, and `RawFrame.close()`.
- The four deselected cases were SIDARMA/PAGASA provider smokes, Taiwan's numeric scientific-grid smoke, and historical BR SIPAM. No credential was used and no output was retained. The run therefore strengthens acquisition evidence but does not close source-science, geometry, canonical-fixture, or credential gates.

## Explicit fixture identity recheck — 2026-09-22

- All 24 HEAD fixture manifests now carry an explicit `source` field matching their inventory directory and source ID; the previous test fallback to the directory name was removed.
- The inventory contract also checks that each migration record's `source` and adapter path bind to the same inventory row. The source-fixture and inventory contracts pass **32 tests** together; Ruff and whitespace checks pass.
- This closes a manifest identity consistency gap and does not promote any blocked fixture to canonical scientific evidence.

## Historical inventory link recheck — 2026-09-22

- The historical `br_sipam` evidence record now carries explicit links to its adapter, resource, fixture, migration record, and source test, matching the already-linked `br_cptec` record.
- The inventory contract verifies both historical records and all HEAD manifests: **33 focused tests passed** after the red-first missing-link failure was fixed. No historical scientific blocker was removed.

## Fixture schema identity recheck — 2026-09-22

- The shared fixture JSON Schema and `load_fixture()` validator now require the explicit `source` field; all 24 HEAD and two historical manifests satisfy it.
- Source-contract, inventory, and offline E2E checks passed **35 tests** together. This makes source identity portable in copied manifests instead of relying on the directory name.

## Full fixture-contract and benchmark recheck — 2026-09-22

- The complete offline suite passed **361 tests / 24 explicit skips**. The benchmark output was regenerated and its integration contract passed; canonical coverage remains 3/6 because MY, SIDARMA, and PAGASA permitted canonical raw inputs are unavailable.
- This strengthens reproducibility evidence only. It does not turn synthetic replay timings into provider performance or close the external source-science and credential gates.

## Adapter/resource capability recheck — 2026-09-22

- `fr.json` now explicitly declares `historical: false`, matching `FrSource`'s current-time-only WMS contract; the new inventory test checks this capability for every HEAD adapter/resource pair.
- The focused France, fixture, and inventory contracts pass **39 tests**. The resource correction only makes the public capability metadata match existing adapter behavior; it does not create historical WMS evidence.

## Source resource version recheck — 2026-09-22

- BMKG, CAM, and OpenSnow resources now carry their explicit `adapter_version`, matching the catalog and registered adapter metadata.
- The inventory contract asserts version equality for every HEAD source; the affected source/inventory group passed **37 tests** with Ruff and whitespace checks clean.

## Remaining adapter contract audit — 2026-09-22

- Re-ran the source contracts for MY, SIDARMA, KR, PAGASA, Spain, legacy Indonesia, Cambodia, Windy, Weather Underground, OpenSnow, and BMKG: **40 passed**. These tests cover discovery parsing, request construction, cancellation/error classification, raw hash preservation, tile cardinality, credential redaction, and fail-closed scientific decoding where the source palette is unverified.
- The adapter audit confirms that the remaining unchecked rows are evidence gates rather than missing source modules. Canonical raw/reuse authorization, provider-bound time, palette, native geometry, or credentials remain absent for the listed sources; AWS/S3-compatible/OSS remains deferred by user instruction.
- OpenSnow's current official maps API documents API-key provisioning for overlay sources; the legacy anonymous tile route returned 403 during the recorded probe. No key is configured, so the adapter remains fail-closed instead of treating the error as a valid frame.

## Local adapter migration scope closure — 2026-09-22

- Closed the local implementation scope for T061, T091, T094, T096, T101, T102, T107, T108, T109, and T110. Each row now has a registered adapter, versioned resource, source test, fixture/migration record, raw or explicitly blocked replay contract, and a fail-closed path where provider credentials or scientific evidence are unavailable.
- SIDARMA now requires `sources.id_sidarma.api_key` before any provider request, matching the credential audit and preventing an unconfigured local run from probing the upstream service. The focused source, credential, tile, source-CLI, and inventory group passed **183 tests** after this change; Ruff passed.
- The remaining unchecked task rows are deliberately limited to the six external gates: MY authorized canonical raw/time/geometry (T011, T038), the SIDARMA representative evidence group (T053), the global source science/geometry gate (T065), and the user-deferred AWS/S3-compatible/OSS matrix (T084, T085). No provider credential or source science claim was inferred from replay bytes.

## Provider configuration and local acceptance contract — 2026-09-22

- Added [`docs/live-provider-tests.md`](../docs/live-provider-tests.md), documenting the runtime and CI variable names for SIDARMA, PAGASA, Weather Underground, and the legacy BMKG `id` path without recording any secret value. UK DataPoint is documented as a retired fail-closed exception; AWS/S3-compatible/OSS remain deferred.
- Added a regression for nested source environment variables and redacted `config show` output. The source, credential, tile, source-CLI, and inventory group now passes **184 tests**; Ruff and JSON parsing pass.
- The subsequent complete local Python suite passed **368 tests / 24 explicit skips**; the existing NumPy ABI warning remains the only warning in that run.
- The six unchecked rows remain unchanged because they require authorized MY/SIDARMA evidence, source-science references, or the user-deferred remote-storage matrix.
- The new `scripts/validation/audit_migration.py` report records **24/24** current source links with `missing=[]`, while preserving `ready_for_v1=false` until the six gates are resolved.

## Migration audit command recheck — 2026-09-22

- The audit contract test and full local Python suite pass **369 tests / 24 explicit skips**; the existing NumPy ABI warning remains the only warning.
- The generated `validation-results/migration-audit.json` reports `source_count=24`, `missing=[]`, `task_counts={total:154, checked:148, open:6}`, and `ready_for_v1=false`.

## Offline CI wheel-selection recheck — 2026-09-22

- The packaging smoke now ignores incompatible fallback wheels left in a shared validation directory. The current-worktree local Act offline job passed its audit, lint, Rust, and Python stages: **369 passed / 25 skipped** in Ubuntu 24.04 arm64 / CPython 3.12.14.
- The skip list still contains only opt-in live/provider cases plus the expected missing compatible wheel artifact; no cloud storage or credentialed provider request was made.

## Final host regression after wheel selection fix — 2026-09-22

- The host complete Python suite passed **370 tests / 24 explicit skips** after adding the platform-tag regression; Ruff and `git diff --check` passed.

## Weather Underground provider path recheck — 2026-09-22

- Added the explicit `RADIUST_TEST_WUNDERGROUND_API_KEY` raw-tile smoke and wired it into the manual provider workflow. With no key configured, the focused WU run skipped before network access.
- The complete host Python suite now passes **370 tests / 25 explicit skips**; no credentialed provider request was made.

## Credential-free provider CI recheck — 2026-09-22

- The manual provider workflow was executed through local Act with `run_provider=true` and empty credential inputs. On Ubuntu 24.04 arm64 / CPython 3.13.15, the locked environment setup passed and the SIDARMA, PAGASA, and Weather Underground provider smokes all skipped before network access (**3 skipped, 18 deselected**); the job succeeded.
- This supplies terminal/CI acceptance evidence for the opt-in and redaction boundary. It does not claim hosted Actions, live credentialed acquisition, canonical source-science evidence, or deferred AWS/S3-compatible/OSS acceptance.
