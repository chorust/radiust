# Release checks

Date: 2026-09-18

## Passed

- `.venv/bin/ruff check python tests`.
- `uv run pytest -q`: 123 passed, 1 explicit clean-wheel skip, one NumPy binary-compatibility warning; the wheel test passes when supplied with `RADIUST_WHEEL`.
- `cargo fmt --all -- --check`.
- `cargo test --workspace`: 16 Rust integration tests passed.
- CPython 3.13 macOS arm64 maturin wheel build, source-tree-outside import, package `list sources` and doctor with temporary paths.
- Local NetCDF, PNG, GeoTIFF, Zarr, raw replay, batch, cache, resource limit and terminal contract tests.
- Loopback S3-compatible remote commit smoke: Rust OpenDAL backend wrote an isolated generation and a second run returned `skipped`.
- Source-tree-outside CPython 3.13 macOS arm64 wheel CLI smoke: isolated loopback S3-compatible `download` returned exit 0 with `written=1`, then `skipped=1`.
- The offline fixture NetCDF passed independent `netCDF4 1.7.4` readback and CF Checker 4.1.0 against CF-1.8 with zero errors or warnings. The installed UDUNITS library was exposed through `DYLD_LIBRARY_PATH`; this does not resolve the separate canonical-source evidence gate.

## Open validation

- The final CPython 3.13 macOS arm64 wheel fixture fetch and core h5py readback pass; `netCDF4` remains a dev-only independent reader and is not installed in the clean core venv.
- The declared Linux x86_64/aarch64 and macOS x86_64 wheel jobs, AWS/S3-compatible/Aliyun provider credentials, real terminal sessions, live sources beyond the eight-source representative smoke, six-source benchmarks and full source migration are not verified.
- The repository constitution remains an untouched template.

The opt-in public live smoke passes 8/8 representative acquisition cases; the provider SIDARMA case remains skipped without an external API key. Source migration is still open because most raw payloads lack verified scientific palette or native geometry evidence. The historical CPTEC adapter replays its old contract, while the current WMS GetMap still returns an upstream mosaic/CRS error. This file records release evidence for the offline MVP and partial live acquisition. It is not a v1 release approval.

## Recheck on 2026-09-20

- `uv run ruff check python tests`: passed.
- `uv run pytest -q`: **238 passed, 13 skipped, 1 warning**. The skips are eight opt-in public-source cases, the SIDARMA provider case, three remote storage-provider cases without isolated test URIs, and the clean-wheel test supplied by packaging CI.
- The full run includes six new raw-replay contract cases for local hash-verified loading/replay, malformed or incomplete manifests, and traversal/symlink rejection.
- `uv run pytest -q tests/integration/test_source_cli_matrix.py`: **17 passed**. The catalog covers 24 sources; 15 raw-backed fixtures exercise shared SDK/CLI plumbing through `FixtureSource`, and RainViewer replays its registered adapter. Eight blocked manifests remain list-only; see [`source-cli-matrix.json`](source-cli-matrix.json).
- CPython 3.10–3.13 macOS arm64 wheels: core plus each extra (`geotiff`, `zarr`, `playwright`, `recovery`, `scraping`, `storage`, `all`) passed clean-venv install and offline NetCDF readback; GeoTIFF and Zarr readbacks also passed. The declared Ubuntu x86_64 extras jobs and Linux x86_64/aarch64 plus macOS x86_64 core-wheel jobs are configured but have not run here; see [`packaging-matrix.json`](packaging-matrix.json).
- Real RainViewer CLI online smoke: latest frame `2026-09-20T02:50:00Z` returned `written=1`, retained four SHA-256-verified raw tiles, `cat --renderer text` read `(1024, 1024)` dBZ, and exact-time repeat returned `skipped=1`. Independent netCDF4 and CF Checker 4.1.0 readbacks passed with zero CF-1.8 errors or warnings; see [`live-cli-rainviewer.json`](live-cli-rainviewer.json). T062 is complete.
- ANSI output passed a local PTY smoke against that live file, but native Kitty/iTerm2 visual sessions and SSH/tmux/Ctrl-C checks remain unverified because the native-app control bridge failed to start; see [`terminal-matrix.md`](terminal-matrix.md).
- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace`: **24 integration tests passed** across cache, FTP, object storage, runtime and tiles; unit/doc targets passed.
- `RADIUST_TEST_ALLOW_LIVE=1 uv run pytest tests/live/test_representative_sources.py -m 'live and not provider' -vv`: **8 passed, 1 deselected** in 27.71 seconds. SIDARMA remains unverified without its API key.
- CPTEC's live adapter discovered 26 WMS station layers and retained one archived 512×512 PNG (9,782 bytes). The latest advertised frame still returned HTTP 200 with a 1,112-byte XML ServiceException; see `migration/history/br_cptec.json`.

## Implementation continuation — 2026-09-20 (offline benchmark)

- `uv run ruff check python tests scripts`: passed, including the new benchmark runner.
- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace`: passed, 24 Rust integration tests (5 cache, 8 FTP, 3 object storage, 5 runtime, 3 tiles); unit/doc targets completed without failures.
- `uv run pytest -q -m 'not live and not provider'`: **249 passed, 1 skipped, 13 deselected, 1 NumPy binary-compatibility warning** after adding the offline benchmark regression test. The clean-wheel case requires a separately supplied built wheel; it was not run by this command.
- `uv run python scripts/validation/benchmark_sources.py --offline --output validation-results/benchmarks.json`: passed for four sources, explicitly unavailable for `id_sidarma` and `ph` due to absent canonical fixture frames. RainViewer measured official replay through the registered decoder; AU/FR measured actual adapter raw acquisition only; `my` measured fixture raw acquisition only. The report includes cold/warm throughput, independent-process peak RSS, request count, sampled temporary-file peak, raw provenance and the reason the old-chain speed comparison remains unavailable.

These checks do not close T147–T154: full scientific-source fixtures, isolated cloud providers, real terminal protocols and remaining platform combinations are still unverified. This is a partial release-check record, not release approval.

## Source-adapter continuation — 2026-09-20

- `uv run pytest -q`: **262 passed, 15 skipped, 1 NumPy binary-compatibility warning**. Skips are 11 intentionally opt-in live cases, three unconfigured real provider cases and one clean-wheel case requiring an installed artifact.
- `uv run pytest -q tests/integration/test_source_cli_matrix.py`: **21 passed**, including registered AU FTP and FR WMS SDK discover/acquire and CLI raw-only replay. Both correctly refuse scientific `cat` until their palettes and geometry are validated. `FixtureSource` still supplies shared-plumbing coverage for 15 source rows and must not be counted as their scientific migration.
- `uv run pytest -q tests/contract/test_migration_inventory.py tests/integration/test_source_cli_matrix.py`: **24 passed**. The PH fixture still explicitly records no canonical provider raw.
- The PAGASA HTTP→Playwright fallback now reuses a single browser session for provider-page CSRF and exact image URL, validates the image bytes, and preserves cancellation/authentication failure behavior. `tests/sources/test_ph_fallback.py` and `tests/integration/test_browser_acquisition.py` cover offline fallback and cancellation cleanup. Live use remains unverified without a current authorized timeline token.
- SIDARMA now propagates total discovery provider errors and cancellation rather than converting them into no-data / TypeError; its partial-station path records the failed-station count. Its live provider key and palette remain unavailable.
- FR WMS raw has an explicit timestamped `.png` name so HTTP 200 XML ServiceException content fails image validation instead of being saved as a successful raw image; `tests/sources/test_fr.py` records the reproducer.
- The local Otty 1.4.1 terminal reports `TERM=xterm-256color`, but its `pane run` IPC capability is disabled by existing configuration; the temporary test pane was closed. Real terminal visual acceptance remains unverified; see `terminal-matrix.md`.
- `uv run ruff check python tests scripts`: passed after import-order fix. `cargo test --workspace` and `cargo fmt --all -- --check` passed with 24 Rust integration tests earlier in this continuation (no Rust files changed afterward).

These are partial implementation/contract advances for T061, T063, T094, T128 and T147. Their full scientific, credential, real-terminal or remote-provider acceptance conditions have **not** been met, and the corresponding open checkboxes remain open.

## Source adapter and browser validation continuation — 2026-09-20

- `uv run pytest -q`: **281 passed, 16 skipped, 1 existing NumPy binary-compatibility warning**. The skips consist of 12 explicitly opt-in live tests (including the localhost real-browser case), three unconfigured isolated storage-provider tests and one wheel artifact supplied by CI; no skip is treated as a pass.
- `uv run pytest -q tests/integration/test_source_cli_matrix.py tests/contract/test_migration_inventory.py`: **35 passed** (32 source/CLI cases and 3 inventory cases). The matrix now runs 16 cases through registered adapters, covering 15 of the 16 raw-backed HEAD sources, including actual CGI, timeline, HTML/directory, FTP, WMS and four-tile discovery/acquisition. The remaining `my` raw is an old rendered PNG and runs through `FixtureSource` only. Another 15 manifest rows still exercise `FixtureSource` for shared-plumbing validation, not each source's science.
- Source-specific adapter replays were added for KR, ES, VN, CA, SG, PT, NZ, TH, TH Royal Rain and Windy. Each asserts retained source bytes, SDK discover/acquire and CLI raw-only output; all without accepted palette/grid references reject scientific `cat`. These are replay assertions, not evidence that the original provider returns a current frame today.
- The browser acquisition now rejects 401/403 and non-2xx HTTP responses during warmup, discovery and navigation before writing a RawFrame, and its five new negative-path tests cover 403/503 and cleanup. FR WMS request projected bounds and pixel-center location are now persisted and checked against `pyproj`; the requested portrayal grid is not the provider's native measurement grid.
- `uv run ruff check python tests scripts`, `cargo fmt --all -- --check` and `git diff --check`: passed. The latest `cargo test --workspace` passed 24 Rust integration tests earlier this turn; subsequent changes only touched Python/tests/evidence metadata.
- Playwright Chromium headless shell v1243 was installed. The opt-in local browser replay still **failed before the first request**, because the macOS command sandbox denied Chromium's MachPort rendezvous. The PAGASA authorized timeline token and canonical image are absent; no actual browser session or provider smoke was accepted. This environmental failure is recorded in `live.json` and the PH migration record.

These checks do not close the 38 outstanding `tasks.md` checkboxes: M0 MY canonical raw/time/geometry, provider-approved scientific palettes/native control points, real remote storage credentials/isolated prefixes, full terminal visual protocols, and Linux/macOS x86_64 wheel CI are missing or dependent on those prior gates. No exception was silently accepted and no release/legacy-archive approval was issued.

## Scientific regridding continuation — 2026-09-20

- The explicit real-Chromium localhost replay (`RADIUST_TEST_ALLOW_LIVE=1 RADIUST_TEST_REAL_BROWSER=1 uv run --extra playwright pytest -q tests/live/test_ph_browser_replay.py -vv`) failed **before its first HTTP request**: Chromium headless shell v1243 could not register the macOS MachPort rendezvous service in the command sandbox (`Permission denied (1100)`). No provider/browser behavior was accepted from this failed run; the earlier Playwright fallback unit tests remain separate offline evidence.
- Bilinear regridding previously inspected invalid or missing source pixels with zero interpolation weight. A new three-column regression reproduces incorrect NaN output at an exact internal grid line and when regridding onto the original grid. The implementation now considers only positive-weight source contributions for missing/quality checks and sets the interpolated quality bit only when more than one sample contributes.
- Multi-variable `RadarDataset.regrid()` previously replaced the actual regridded quality flags with a nearest-neighbor quality sample. A regression reproduced the missing interpolation bit. The returned shared quality plane now ORs the per-variable regridded flags so interpolation/invalid reasons survive multi-variable processing.
- `uv run pytest -q tests/contract/test_regrid.py tests/contract/test_scientific_model.py tests/integration/test_source_cli_matrix.py`: **39 passed, 1 existing NumPy binary-compatibility warning**.
- `uv run pytest -q`: **284 passed, 16 skipped, 1 existing NumPy binary-compatibility warning**. The 16 skips comprise 12 explicitly opt-in live tests, three unconfigured provider tests, and the clean-wheel artifact test.
- `uv run ruff check python/radiust/field.py tests/contract/test_regrid.py` and `git diff --check`: passed. No Rust source was changed in this continuation.

This fixes a deterministic scientific interpolation regression within T065; T065 itself remains open because the representative provider-native palette, pixel, geometry and memory acceptance gates are still unmet. The broader 38-task completion status is unchanged.

## Live-source recheck — 2026-09-20

- `RADIUST_TEST_ALLOW_LIVE=1 uv run pytest -m 'live and not provider' tests/live/test_representative_sources.py -vv`: **9 passed, 2 deselected** in 53.21 seconds. Current latest raw acquisition succeeded for AU, RainViewer, FR, TW-HTTP, ES, PT, SG and TH; the retained Taiwan CWA numeric grid also decoded with its EPSG:3821 geometry and dBZ values.
- The result confirms bounded live acquisition and the already supported Taiwan numeric-grid path for this run. It supplies no verified palette/native geometry for the image-only adapters; acquisition smoke is not counted as source science acceptance. The detailed run is in [`live.json`](live.json).
- No test credentials or provider URIs were configured for SIDARMA, PAGASA, AWS S3, S3-compatible storage or Aliyun OSS; no such provider requests were sent.
- The native UI inventory showed iTerm2 running, but the computer-use control layer rejected opening it for safety. The real ANSI/Kitty/iTerm2 visual matrix remains unverified; no terminal security or IPC setting was changed.

This online recheck does not close T011/T038/T049, the provider-gated source tasks, T084/T085 or T128/T129. The 38 open task markers remain unchanged.

## Playwright lifecycle and suite recheck — 2026-09-20

- Fixed `BrowserAcquirer.fetch_page()` and `.acquire()` so the Playwright manager remains alive until the context and browser close. The real localhost Chromium replay had exposed `TargetClosedError` during cleanup because the manager stopped first.
- `uv run pytest -q tests/integration/test_browser_acquisition.py`: **15 passed**, including lifecycle-order regressions for both browser entry points.
- `RADIUST_TEST_ALLOW_LIVE=1 RADIUST_TEST_REAL_BROWSER=1 uv run --extra playwright pytest -q tests/live/test_ph_browser_replay.py -vv`: **1 passed in 1.71 seconds**. This is a localhost CSRF/cookie and PNG replay; it does not validate PAGASA credentials, source timing, palette, or geometry. The previous sandbox launch failure is superseded for this local test only.
- `uv run pytest -q`: **286 passed, 16 skipped, 1 existing NumPy binary-compatibility warning**. The skips are 12 opt-in live tests, three unconfigured isolated storage-provider tests, and the clean-wheel artifact test.
- `uv run pytest -q tests/integration/test_source_cli_matrix.py tests/contract/test_migration_inventory.py`: **35 passed, 1 existing NumPy binary-compatibility warning**.
- `RADIUST_TEST_ALLOW_LIVE=1 uv run pytest -m 'live and not provider' tests/live/test_representative_sources.py -vv`: **9 passed, 2 deselected**; current raw acquisition passed for eight public adapters and Taiwan's numeric grid decoded. This does not prove image-adapter science.
- `uv run ruff check python tests scripts`, `cargo fmt --all -- --check`, `cargo test --workspace` (**24 integration tests**) and `git diff --check`: passed.
- The official METMalaysia API documents general forecasts and directs other meteorological requests to METMalaysia; its myMETdata radar-raw product is a paid Rainbow-native data product. The site's copyright notice requires express prior written consent for copying/distribution. No authorized radar sample, reuse consent, valid-time binding, or native geometry is present, so T011/T038 remain open. See [`my blocker`](../migration/blockers/my.md).

The remaining 38 task checkboxes are still open. This recheck repairs the browser evidence and validates the offline suites; it does not waive source, provider, terminal, or platform acceptance gates.

## Packaging matrix continuation — macOS x86_64

- Cross-built the CPython 3.10 x86_64 wheel under Rosetta with `MACOSX_DEPLOYMENT_TARGET=11.0`; wheel tag: `macosx_11_0_x86_64`.
- The clean installed-wheel smoke passed (`1 passed`): the nested x86_64 venv imported outside the source tree and wrote/read the offline NetCDF fixture with matching science and quality arrays.
- The workflow now pins the macOS deployment target to 11.0 so x86_64 wheel tags match the planned minimum. This run used a macOS 26.6.2 arm64 host under Rosetta, so it does not replace native macos-13 CI or validate runtime behavior on macOS 11. T150 remains open for the remaining Python/platform/extra matrix.


## Packaging matrix continuation — macOS x86_64 CPython 3.11–3.13 (2026-09-21)

- Installed the matching x86_64 CPython 3.11.10, 3.12.7, and 3.13.0 interpreters and built `macosx_11_0_x86_64` wheels under Rosetta. All three builds passed.
- Ran `tests/packaging/test_installed_wheel.py` once per matching interpreter and wheel: **3/3 passed**. Each clean child environment imported the wheel outside the source tree, found all 24 packaged source entries, and completed offline NetCDF reflectivity/quality readback.
- The minimal outer pytest runner emitted the existing `asyncio_mode` unknown-option warning; this did not affect the three passing installed-wheel checks.
- The CPython 3.10 Rosetta wheel was validated in the earlier entry. Native `macos-13` x86_64 CI and execution on macOS 11 remain unverified; see [`packaging-matrix.json`](packaging-matrix.json).


## macOS Intel CI runner refresh — 2026-09-21

- Replaced the retired `macos-13` GitHub Actions label with `macos-15-intel` for the CPython 3.10–3.13 native x86_64 wheel jobs. GitHub announced that `macos-13` would be retired on 2025-12-04 and lists `macos-15-intel` for Intel standard runners ([GitHub Changelog](https://github.blog/changelog/2025-09-19-github-actions-macos-13-runner-image-is-closing-down/)).
- The workflow YAML parses and its matrix is now aligned with the supported runner label. Native CI has not yet run; local Rosetta tests do not replace that evidence.

## Full offline recheck — 2026-09-21

- `uv run ruff check python tests scripts`: passed.
- `cargo fmt --all -- --check`: passed; `cargo test --workspace`: passed, 24 integration tests across cache, FTP, object storage, runtime and tiles.
- `uv run pytest -q -m 'not live and not provider'`: **286 passed, 2 skipped, 14 deselected**, with one existing NumPy binary-compatibility warning. The two skips are the opt-in browser smoke and CI-supplied wheel artifact; deselected tests include live/provider gates.
- `uv run pytest -q tests/integration/test_source_cli_matrix.py tests/contract/test_migration_inventory.py`: **35 passed**, one existing NumPy binary-compatibility warning.
- `uv run python scripts/validation/benchmark_sources.py --offline --output validation-results/benchmarks.json`: completed with 4/6 sources locally measurable; SIDARMA and PAGASA have no canonical fixture frame, and no matched old-chain baseline exists. No benchmark speedup is claimed.
- The `wheels.yml` YAML and wheel matrix were parsed after replacing retired `macos-13` labels with `macos-15-intel`. This validates workflow syntax, not a hosted Actions run.

- Opt-in public live recheck: `RADIUST_TEST_ALLOW_LIVE=1 uv run pytest -m 'live and not provider' tests/live/test_representative_sources.py -vv` — **9 passed, 2 deselected** (eight latest raw acquisitions and the Taiwan CWA numeric grid). This is live acquisition/one numeric-grid path evidence; image-source palettes/native grids remain unverified. Updated `live.json`.


## Adapter, terminal and current-wheel recheck — 2026-09-21

- Full offline suite: `uv run pytest -q -m 'not live and not provider'` — **292 passed, 2 skipped, 14 deselected**, with one existing NumPy binary-compatibility warning. The skips are the opt-in Chromium smoke and CI-supplied wheel artifact.
- Rust workspace: **24 integration tests passed** (cache 5, FTP 8, object storage 3, runtime 5, tiles 3); `cargo fmt --all -- --check` and `uv run ruff check python tests` passed.
- Source/CLI plus inventory matrix: **36 passed**. The standalone source CLI matrix has 33 cases; all 16 raw-backed IDs now have adapter replay, including MY's synthetic registered-adapter HEAD/GET CLI raw-only case. This does not turn the MY transcript into provider evidence.
- Terminal CI subset: **13 passed**, including the actual PTY subprocess test. The matching `terminal-contract` job and full offline job are configured in `.github/workflows/offline.yml`; all workflow YAML parses locally. No hosted workflow was triggered. Real emulator, SSH/tmux, visual layout and Ctrl-C recovery remain open.
- Built a fresh CPython 3.12 macOS arm64 wheel and ran clean source-tree-outside installed-wheel checks: **core 1 passed; all extras 1 passed**, with NetCDF/HDF5 reflectivity and quality readback and GeoTIFF/Zarr readback in the all-extra run. Native hosted Linux/Intel matrix remains pending.
- The bounded MY live CLI attempt failed TLS certificate-chain verification before a payload was written; TLS verification stayed enabled. The earlier credential audit is summarized without values in `live.json`: legacy SIDARMA key probe returned 403, PAGASA browser did not reach its provider request, retired UK key was not sent, and no WU literal was found.
- AWS/S3-compatible/OSS live tests remain **deferred per user instruction**; the local storage matrix and offline object-store tests remain available.

These results improve adapter replay and terminal/CI coverage but do not close the evidence-gated source tasks or T147–T154. At this checkpoint, 38 tasks remained open; the default-suite and task-status update below supersede that count. No release approval is implied.


## Default-suite and live-source recheck — 2026-09-21

- `uv run pytest -q` completed successfully: **302 passed, 7 skipped** in 80.60 seconds, with one existing NumPy binary-compatibility warning. It ran the eight public latest raw-acquisition cases and the Taiwan numeric-grid case; these are live acquisition/scientific checks for that run only.
- The seven skips were the opt-in real-Chromium localhost replay, unconfigured SIDARMA and PAGASA credentials, three remote storage providers, and the CI-supplied wheel artifact. S3/compatible/OSS remain deferred at the user's request.
- Detailed cases and scope are recorded in `validation-results/live.json`. The public smoke does not validate other source palettes/geometries, and the provider skips are not counted as passes.
- T153 is complete: the full default/offline tests, Rust, formatting, lint, source/inventory matrix and fresh installed-wheel checks are recorded above. **37 task checkboxes remain open**; T152 and release closure still require unrun or unavailable provider, terminal-visual, source-science, and hosted-matrix evidence.


## Default live opt-in enforcement — 2026-09-21

- The earlier `uv run pytest -q` result of 302 passed / 7 skipped included nine public network cases because `live` markers were not gated. This exposed a mismatch with T149's default-off requirement; that run remains recorded as live evidence, not as the current offline default.
- Added collection-time `RADIUST_TEST_ALLOW_LIVE=1` and `RADIUST_TEST_ALLOW_PROVIDER=1` gates. The local Chromium replay is now marked `live`. Two policy tests pass, and with all opt-in variables unset the 12 live/provider-source cases each skip before executing.
- `RADIUST_TEST_ALLOW_LIVE=1 ... -k rainviewer`: **1 passed, 10 deselected**, confirming explicit opt-in still enables the public path.
- Current default run with opt-ins unset: **295 passed, 16 skipped** in 12.48 seconds, with one existing NumPy binary-compatibility warning. No public/provider request was made by that default run. `.github/workflows/offline.yml` now runs plain `uv run pytest -q`, so CI verifies this default-off behavior.
- T153 remains complete after rerunning the affected default suite, terminal subset, and Ruff; 37 tasks remain open for external source evidence, deferred storage, terminal visual, and hosted matrix acceptance.


## PAGASA legacy-token smoke — 2026-09-21

- The pinned legacy repository contains a timeline token literal. A single bounded latest-frame smoke passed that value to the existing test only through a child-process environment variable; no value or credential-bearing output was emitted.
- The run did not return a verified `RawFrame`; the browser acquisition stage failed. Token validity and provider response remain unverified, and no image was accepted as a fixture. The localhost Chromium replay remains separate evidence. `migration/sources/ph.json` and `live.json` record this result.

## Current adapter, terminal and CI recheck — 2026-09-21

- All 24 catalog IDs have an adapter module, resource manifest, source test module and migration record. The offline suite covers **303 passed, 16 opt-in/deferred skips**, with one existing NumPy binary-compatibility warning. `uv run ruff check python tests scripts`, `cargo fmt --all -- --check`, `cargo test --workspace --locked` (**24 Rust integration tests**), `git diff --check`, and validation JSON parsing passed.
- The source adapter suite has **94 passed**; the source/CLI replay matrix has **33 passed**; the terminal/CLI subset has **16 passed**, including POSIX PTY output and north-up/west-left image orientation with quality alignment. Browser acquisition integration has **15 passed**.
- The WU tile adapter now keeps its configured key out of `FrameRef` URLs and identity, injecting it only at request time. Offline tests verify four keyed transport requests, credential-free frame/report data, identity stability across different keys, and failure before transport when no key is configured. Live WU remains skipped because the legacy repository has no key.
- Météo-France applies the same boundary to its ephemeral WMS session token: frame/identity/report values stay token-free, while replay tests verify the transport request receives the derived cookie token.
- The latest public opt-in smoke passed **9 cases** in 33.91 seconds (eight current raw acquisitions and the Taiwan numeric grid), including FR with acquisition-time token injection; exact scope and exclusions are in [`live.json`](live.json). No secret value was written to evidence.
- Re-ran the offline benchmark: 4/6 sources measured; AU cold/warm request counts were 2/1, FR 2/0, and SIDARMA/PAGASA unavailable without canonical fixtures. No old-chain baseline exists, so no speedup is claimed.
- The fresh CPython 3.12 macOS arm64 wheel SHA-256 is `d726dc0ed67a99a6c6d1b1b5f60bb2682292155f628ccb4a290e790f8b85e5f2`; clean source-tree-outside `core` and `all` wheel installs both passed, including NetCDF/HDF5 and GeoTIFF/Zarr readback.
- All **27** raw artifact references across 26 source fixture manifests exist and match their declared SHA-256; all are visible to Git. The `.gitignore` allowlist is confirmed by Git status (not by the exit status of `git check-ignore -q`, which reports a matching negation rule).
- All three workflow YAML files parse locally. `act -l` listed the jobs, but the installed binary emitted CVE-2026-34041/34042 upgrade warnings and was not used to execute them; a local job also cannot start without the OrbStack Docker daemon. Hosted Actions has not run. The wheel matrix now targets Ubuntu 24.04 x86_64 and the existing Ubuntu 24.04 ARM label. GitHub's runner-image project announced Ubuntu 22.04 deprecation beginning 2026-09-17, so the wheel jobs were moved to Ubuntu 24.04 ([runner image notice](https://github.com/actions/runner-images/issues/14254), [supported runner labels](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)).
- AWS S3, S3-compatible and Aliyun OSS provider tests remain deferred at the user's request. Real Kitty/iTerm2 visual inspection, SSH/tmux passthrough, Ctrl-C recovery, source science/geometry blockers, credentialed providers and hosted platform matrix remain open. **37 task checkboxes remain open; no task was checked off without meeting its acceptance gate.**


## Adapter migration continuation — 2026-09-21

- CAM's live slideshow page rejects requests without its legacy browser-style headers. Restored the source's required `Accept`, `Accept-Language`, and `User-Agent` on discovery, added offline discovery/acquire regression coverage, and added an explicit-opt-in live test. The individual CAM live test passed; the full public opt-in matrix now reports **10 passed, 2 provider cases deselected**.
- The real CAM CLI command `radiust download cam --latest --raw-only` with a temporary `allow_network: true` config wrote one latest JPEG. Its manifest and both raw artifacts verify; the frame is station `ZIWR03`, time `2026-09-21T04:15:04Z`, 78,539 bytes, SHA-256 `62175868f54eb2a70a327fe35b2ae543ec7974808b69ed7bfcd049307a190c9e`. The sample remains in temporary output because no fixture reuse basis is documented. Its SRI rainfall-intensity legend and native map transform remain unverified; no dBZ decode is claimed.
- Explicit-network latest CLI probes of OpenSnow and BMKG returned authentication errors; direct requests to the correctly formed current tile URLs returned HTTP 403 from Cloudflare. Neither wrote raw artifacts. Their migration records now carry this dated endpoint evidence.
- Filled the missing `migration/sources/br_sipam.json`, linked it from the historical inventory, geometry audit, and exception record, and added a regression assertion that each migrated historical adapter has a migration record. Added a second inventory assertion that every blocked source migration record includes evidence and blocker details; MY's record now lists its blockers.
- Fresh checks: default offline suite **304 passed, 17 skipped** (one existing NumPy ABI warning); Ruff, `cargo fmt`, 24 Rust integration tests, migration/source JSON parsing, workflow YAML parsing, and `git diff --check` passed.
- Rebuilt CPython 3.12 macOS arm64 wheel after the current adapter changes. Clean installed-wheel `core` and `all` tests each passed outside the source tree. SHA-256: `da9688a52c7251aa798991b505688f97748c0b76478731e530fbe5c663b46017`.
- All **37** task checkboxes remain open for missing canonical/source-science evidence, deferred AWS/S3-compatible/OSS tests, physical terminal acceptance, and hosted platform CI. Adapter code coverage across all 24 catalog sources remains in place; this continuation closes the CAM live acquisition defect and the BR SIPAM inventory record gap without overstating those remaining gates.


## Adapter, live, and terminal recheck — 2026-09-21

- Full default offline suite: **304 passed, 22 skipped**, one existing NumPy ABI warning. Ruff, `cargo fmt --all -- --check`, `cargo test --workspace --locked` (24 integration tests), and `git diff --check` passed; all 95 migration/validation/fixture/resource JSON files parsed.
- Source and migration inventory contracts: **98 passed**. Terminal/rendering/CLI acquisition and output contracts: **17 passed**, including the real POSIX PTY ANSI test and Kitty/iTerm2 sequence-boundary tests.
- Explicit public live matrix: **15 passed, 2 provider cases deselected** in 84.08 seconds. It includes 13 public adapter raw acquisitions, the Taiwan numeric grid, and historical BR SIPAM. Current VN/NZ/KR/Windy hashes are recorded; CA exceeded the bounded discovery timeout and Royal Rain returned a transport error. Neither is treated as retired.
- Actual `radiust download --latest --raw-only --no-cache --json` CLI smokes passed for KR/BRI, NZ/NZAU2, VN/DHA, and Windy/global. All four manifests reported raw complete; every artifact hash/size/path was checked before the temporary output was deleted. Evidence: `live-cli-public.json`.
- The default suite skips 18 live cases before network access, plus three storage-provider tests and one clean-wheel artifact test. S3-compatible/AWS/OSS live provider checks remain deferred per instruction.
- `act -l` parses the workflows. Local dry-run cannot start without Docker; GitHub-hosted Actions was not triggered. Kitty/iTerm2/SSH/tmux visual review and visible Ctrl-C recovery are still open. The current partial evidence is in `terminal-matrix.md` and `us4.md`.

The 37 unchecked tasks still map to missing canonical/scientific source evidence, the deferred provider matrix, visual terminal acceptance, and downstream gates. No task checkbox was closed on the strength of raw-only or offline-only results.

## Latest verification after local evidence updates — 2026-09-21

- `uv run pytest -q`: **304 passed, 22 skipped**, with one existing NumPy ABI warning. The skips are opt-in live/provider cases, the three deferred remote-storage tests, and the CI-supplied wheel artifact test.
- Re-ran the local PAGASA Chromium CSRF/session replay. The default sandbox denied Chromium Mach bootstrap before reaching the loopback service; the same test then passed through managed review: **1 passed in 2.01s**. It uses only a local mock server and generated PNG bytes, not PAGASA or a scientific sample.
- All **96** scoped migration, validation, fixture, and packaged-resource JSON files parse; `git diff --check` passes. The replay validates local browser/session plumbing only; PAGASA token validity, canonical raw, observation-time binding, palette and geometry remain open.

## Public raw-only CLI matrix expansion — 2026-09-21

- Re-ran nine additional actual CLI smokes: CAM, AU/AU02, RainViewer, FR, tw-http/CV1_3600, ES/ESCOMP, PT/PTST2, SG/SGCOMP, and TH/cmp1. All wrote one frame with exit code 0 and no failed items.
- For these runs and the earlier KR/BRI, NZ/NZAU2, VN/DHA, and Windy/global runs, the output manifest and each artifact path, byte count, and SHA-256 were checked in temporary directories before cleanup. All 13 reports are recorded in `live-cli-public.json`; the retry captured exact sidecar-manifest size and hash from disk.
- These were acquisition/commit checks only. At that earlier checkpoint TH's frame timestamp still reflected retrieval time; the later footer-OCR correction supersedes it and is recorded below. These raw-only runs do not close palette, physical-value, or native-geometry requirements. No source payloads were retained.
- Added a query-aware discovery hook after tests exposed that the ECCC and Royal Rain adapters fetched all station pages before applying `--station`. Both source regression tests failed on the extra request first, then passed after the fix. Actual CLI retries wrote verified CA/CASFT and Royal Rain/takhli frames; CA fell from 39.59 seconds to 3.57 seconds, and Royal Rain changed from a 130.01-second timeout to a verified write in 43.66 seconds.

## Query-aware discovery final verification — 2026-09-21

- `uv run pytest -q`: **306 passed, 22 skipped**, with one existing NumPy ABI warning. Ruff passed; all 96 scoped JSON files parse, and `git diff --check` passes.
- The new CA and Royal Rain station-query tests failed first because discovery requested an unselected station page; after adding the query-aware hook, both and their existing source tests pass. The actual CLI rechecks verified 15 public-source outputs in total; only temporary raw files were produced and all were deleted.
- T097 and T098 remain open because these raw-only acquisitions still lack verified physical palette semantics and native pixel geometry.

## Latest public live and default-suite verification — 2026-09-21

- Explicit public smoke: `RADIUST_TEST_ALLOW_LIVE=1 uv run pytest -q -m 'live and not provider' tests/live/test_representative_sources.py` — **17 passed, 2 credentialed cases deselected** in 196.75 seconds. It covers 15 catalog raw acquisitions, Taiwan's numeric grid, and historical-only BR SIPAM. Every acquired `RawFrame` had hashed, sized artifacts and was closed after validation; this run does not establish palette or native-geometry correctness for image sources.
- Default suite: `uv run pytest -q` — **306 passed, 24 skipped** in 12.16 seconds, with one existing NumPy ABI warning. All 20 live cases in the selected live test files skipped before network access; the remaining skips are three deferred remote-storage tests and the CI-supplied wheel artifact test.
- `validation-results/live.json` records the run. The 15-source actual CLI raw-only matrix remains separately documented in `live-cli-public.json`; its manifest, output path, byte count, and SHA-256 checks passed before temporary outputs were removed.
- AWS S3, S3-compatible, and OSS tests remain deferred at the user's request. Provider credentials, source science/geometry evidence, physical Kitty/iTerm2/SSH/tmux review, and hosted platform CI are still open; the migration tasks remain unchecked where their stated acceptance gates are unmet.

## UK retirement exception acceptance — 2026-09-21

- Rechecked the official Met Office FAQ: DataPoint retired on 2025-12-01; Radar Composite has no like-for-like replacement, with paid provisioning offered separately. The new repository keeps UK fail-closed and sends no legacy key.
- `uv run pytest -q tests/sources/test_uk.py tests/contract/test_migration_inventory.py`: **5 passed**. The UK regression asserts `discover()` raises before transport, and the migration inventory records the already accepted `uk-datapoint-retired` exception.
- Updated T106 to close as a retirement disposition under that accepted exception. This does not claim an operational UK adapter or current scientific frame.

## Taiwan image-product metadata audit — 2026-09-21

- The official CWA dataset page identifies O-A0058-005 as the wide-area transparent radar echo image, with a 10-minute update frequency, declared extent 115.00–126.50°E / 17.75–29.25°N, and 3600×3600 image dimensions. The CWA product guide lists the numeric integrated-echo product O-A0059-001 separately from the O-A0058-005 image.
- This corroborates the image adapter's provider metadata/time/dimension checks, but does not give pixel registration or RGB-to-dBZ values. `tw` therefore continues to reject scientific decoding of the image; its separate O-A0059-001 numeric grid retains its own verified TWD67 geometry and dBZ semantics.
- Primary evidence: [CWA O-A0058-005 dataset page](https://opendata.cwa.gov.tw/dataset/observation/O-A0058-005) and [CWA QPESUMS product guide](https://www.cwa.gov.tw/Data/data_catalog/3-6-5.pdf).

## MY TLS and live CLI content-type recheck — 2026-09-21

- Added Certifi public roots to the existing platform trust context without disabling TLS chain or hostname verification. The MY live `--latest --raw-only` CLI run then wrote two frames successfully.
- Both endpoints advertised `image/gif` but the downloaded payloads verified as PNG. The adapter preserves both facts in metadata, stores the actual payload as `.png` / `image/png`, and fails closed on another format. The regression tests and CLI transcript passed.
- Hashes, sizes, dimensions, Last-Modified, and computed legacy valid times are recorded in `validation-results/live-cli-my.json`. The temporary output files were deleted after validation. No reuse permission, independently verified time binding, palette, or native geometry was established, so T011/T038 stay open.
- Rebuilt the CPython 3.12 macOS arm64 wheel from this worktree; clean installed-wheel `core` and `all` smokes both passed outside the source tree, including NetCDF/HDF5 and GeoTIFF/Zarr readback. Wheel SHA-256: `5a74a3ceaca1b141f5e5834c44b139d77dff132c19e6b85a29fa1787bd31f8ea`. The temporary wheel was deleted. Hosted CI remains unrun because this worktree has not been published to a remote ref.

## ECCC CAPPI metadata and CLI audit — 2026-09-21

- The public CLI wrote the latest CASFT CAPPI 1.5 RAIN GIF in 3.57 seconds; its 17,402-byte raw hash and manifest are recorded in `live-cli-public.json`. This replaces the earlier 20-second unfiltered discovery timeout as the current result.
- ECCC's official Datamart readme documents the product directory/filename pattern and 1.5 km CAPPI product. The official radar-site list locates CASFT Franktown at 45.04115°N, 76.11638°W. The fixture legend displays Rain mm/hr and dBZ tick labels, but the per-color value mapping and image pixel transform remain unverified; the site coordinate and 1 km/pixel display scale are insufficient for georeferencing.
- Primary sources: [ECCC radar Datamart readme](https://eccc-msc.github.io/open-data/msc-data/obs_radar/readme_radarimage-datamart_en/) and [official radar-site list](https://collaboration.cmc.ec.gc.ca/cmc/cmos/public_doc/msc-data/obs_radar/radars_list.pdf). The acquisition migration task T097 is now complete; palette lookup and raster geometry remain blocked under the broader scientific acceptance gates.

## Public opt-in smoke after TLS trust update — 2026-09-21

- `RADIUST_TEST_ALLOW_LIVE=1 uv run pytest -q -m 'live and not provider' tests/live/test_representative_sources.py`: **17 passed, 2 provider cases deselected** in 157.61 seconds. It covers 15 public catalog raw acquisitions, the Taiwan numeric-grid check, and historical-only BR SIPAM; all acquired raw frames were closed and no output was retained.
- The run passed with platform trust roots augmented by Certifi while TLS chain and hostname verification stayed enabled. The SIDARMA/PAGASA credentialed cases remain unverified; provider tests were not run.

## Final local verification for current workspace — 2026-09-21

- `uv sync --locked --group dev` passed. `uv run pytest -q` passed **307 tests** with **24 explicit skips** in 11.28 seconds; skips were live/provider opt-ins and the CI-supplied wheel artifact. One pre-existing NumPy ABI warning remains.
- `uv run ruff check python tests scripts`, `cargo fmt --all -- --check`, and `cargo test --workspace --locked` passed; Rust ran 24 integration tests. The focused MY/CA/source-matrix/inventory run passed 43 tests. All 1,711 repository JSON files parsed, all three workflow YAML files parsed, and `git diff --check` passed.
- The current CPython 3.12 macOS arm64 wheel passed isolated `core` and `all` installed-wheel checks with scientific NetCDF/HDF5 readback plus GeoTIFF/Zarr readback. The wheel was removed after hashing.
- No hosted workflow was run: the implementation and fixtures are still uncommitted in this workspace. No remote branch was published. The iTerm2 visual attempt was denied by desktop safety, so T128/T129 remain open. The source/evidence, terminal, packaging, and user-deferred provider gates leave **36 task checkboxes open**; this report does not claim release readiness.

## Source-adapter task acceptance — 2026-09-21

- `uv run pytest -q tests/sources/test_tw.py tests/sources/test_tw_http.py tests/sources/test_vn.py tests/sources/test_ca.py tests/sources/test_th_royalrain.py tests/sources/test_nz.py tests/sources/test_au.py tests/contract/test_migration_inventory.py tests/integration/test_source_cli_matrix.py`: **56 passed**, with one pre-existing NumPy ABI warning. Ruff passed for all seven adapter modules and their source tests.
- `cargo test -p radiust-core --test ftp --locked`: **8 passed**, including transient listing retry within the shared frame budget, authentication failure without retries, cancellation, and read/list contracts.
- T092, T093, T095, T097, T098, T100, and T103 now have complete source-acquisition migration evidence and are checked in `tasks.md`. Their separately tracked palette/value mapping and native-grid blockers remain open wherever applicable; this does not close T049/T065 or the all-source release gate.
- T093's Brotli disposition is deliberate: the reviewed legacy path requests only gzip/deflate. The source regression also runs through the production Python HTTP transport against a loopback server and verifies that neither the `Observe_radar.js` nor image request advertises `br` on the wire; product/time binding and the real raw-only CLI path pass. Current CA, Royal Rain, NZ, VN, and AU raw-only CLI evidence is in `live-cli-public.json` and each source manifest.
- After adding the actual-wire Brotli regression, `uv run pytest -q` passed **308 tests** with **24 expected skips** in 11.89 seconds. The skips are live/provider opt-ins and the wheel-artifact test; no live or provider opt-in was enabled. Ruff, `cargo fmt --all -- --check`, workflow YAML parsing, and `git diff --check` passed. The task list now has **29 open / 125 complete**.

## Current local adapter, terminal, and CI recheck — 2026-09-21

- The latest default offline suite passed **311 tests with 25 expected skips** and one pre-existing NumPy ABI warning. `uv run ruff check python tests`, `cargo fmt --all -- --check`, and `cargo test --workspace --locked` passed; Rust reported 24 integration tests. The focused source/CLI/benchmark group passed 34 tests, and the terminal/renderer/CLI group passed 24, including ANSI output in a POSIX PTY.
- Rebuilt the CPython 3.12 macOS arm64 wheel after the IPMA timeline metadata update. Clean source-tree-outside `core` and `all` child-venv tests each passed; NetCDF/HDF5 and GeoTIFF/Zarr readback passed. SHA-256: `5fea54f0f8b933277f121e516bd57d58281f91f9467cb14188420790bfd1c473`.
- The three workflow files parse as YAML. Local `act` could not execute its offline or live jobs because the Docker daemon is unavailable; the wheel dry run skipped the unsupported runner platforms. GitHub-hosted CI was not dispatched, so platform matrix jobs remain unverified.
- All 24 catalog source IDs resolve to adapter modules (including `tw-http` → `tw_http.py`). Source code presence and acquisition contracts do not close missing canonical raw, credential, palette/value, time-binding, geometry, or reuse-authorization gates. The task list remains **126 complete / 28 open**; T128/T129 visual terminal acceptance, T150/T152 CI/release gates, and T154 readiness remain open. AWS S3, S3-compatible, and OSS tests were not run per instruction.
- Final targeted migration-inventory plus source-CLI validation passed **38 tests**. The catalog-to-registry audit confirms **24/24** source IDs map to an adapter module, source test, source resource, and migration record; raw/fixture/science gates remain separately tracked.

## TMD retained-animation and current local CI recheck — 2026-09-21

- Retained the current KKN `kkn240Loop.gif` response as a hash-checked fixture and added both station variants (`cmp1`, `kkn240Loop`) to the registered adapter's SDK/CLI raw replay. The TH/migration-inventory focused tests passed **9**; the source CLI matrix passed **34**, and the combined matrix/inventory run passed **38**.
- `uv run pytest -q`: **314 passed, 25 expected skips**, one existing NumPy ABI warning. `uv run ruff check python tests` passed, `cargo test --workspace --locked` passed all **24 Rust integration tests**, and the workflow's terminal/CLI subset passed **16 tests**.
- All three `.github/workflows/*.yml` files parse locally. Docker is unavailable to `act`, so workflows did not execute in a local runner; no hosted workflow was dispatched from this uncommitted worktree. Kitty/iTerm2/SSH/tmux visual acceptance remains open.
- AWS S3, S3-compatible, and OSS checks remain deferred as instructed. Task count remains **126 complete / 28 open**; no source-science or release gate was closed based on raw-only replay.

## SIDARMA legacy raw recovery and local wheel matrix — 2026-09-21

- Recovered one old `id` `*_source.png` payload from the legacy checkout's ignored `output/id/` tree. It is retained and hash/dimension decoded under `tests/fixtures/sources/id/raw/`; the fixture remains `blocked` with `frames: []` because the discovery response, payload URL, TLS-verified receipt, and independently verified observation time are absent. The old client disabled TLS verification; palette and native geometry are still unknown. T101 remains open.
- `uv sync --locked --group dev`, `uv run ruff check python tests`, `cargo fmt --all -- --check`, and `cargo test --workspace --locked` passed. The Rust workspace ran 24 integration tests. `uv run pytest -q` passed **315 tests with 25 explicit skips** and one pre-existing NumPy ABI warning. The full terminal/render/CLI contract subset passed **24 tests**, and `tests/integration/test_source_cli_matrix.py` passed **34 tests**.
- Built clean macOS arm64 wheels for CPython 3.10, 3.11, 3.12, and 3.13. `tests/packaging/test_installed_wheel.py` passed all **32** local combinations (four Python versions × core, geotiff, zarr, playwright, recovery, scraping, storage, and all). Each run checked an isolated child environment, optional dependency separation, default NetCDF/HDF5 readback, and GeoTIFF/Zarr data and quality round-trips where enabled. In addition, the earlier Rosetta x86_64 builds and core smoke passed for CPython 3.10–3.13 (4/4); no x86_64 extra installs were run. Wheel hashes and scope are recorded in [`packaging-matrix.json`](packaging-matrix.json).
- The temporary pytest-only matrix runner emitted `Unknown config option: asyncio_mode` because it excluded the project pytest plugin/conftest to avoid importing the source tree; this did not fail any wheel smoke. Native GitHub Linux x86_64/aarch64 and macOS x86_64 build jobs, plus hosted Ubuntu extra jobs, remain unrun. The workspace is unpublished, and this does not replace hosted CI.
- Re-ran `scripts/validation/benchmark_sources.py --offline --output ...` and retained the run in `benchmarks.json`: **4/6 measured**; SIDARMA and PAGASA are unavailable without canonical fixtures. No matched legacy-chain baseline exists, so no speedup is claimed and T148 remains open behind T147.
- Verified the temporary official `act` v0.2.89 Darwin arm64 binary against the release SHA-256, then attempted the offline Python 3.12 Linux/arm64 job. Docker 28.5.2 started, but the runner image pull stalled for over two minutes and was cancelled before workflow steps. No local-container or hosted CI pass is claimed; see [`terminal-matrix.md`](terminal-matrix.md).
- Real Kitty/iTerm2/SSH/tmux visual acceptance remains open under T128/T129. AWS S3, S3-compatible storage, and OSS tests remain deferred. No source or release task checkbox was closed from these local checks; the task count remains **126 complete / 28 open**.

## Successful local offline workflow execution — 2026-09-21

- The earlier act attempt stopped during runner-image acquisition. After switching to a temporary Ubuntu 24.04 arm64 runner, the actual `offline` job in `.github/workflows/offline.yml` completed under the SHA-verified official act v0.2.89 binary, selecting CPython 3.12.14.
- `uv sync --group dev --locked`, Ruff, `cargo test --workspace --locked` (**24 Rust integration tests**), and `uv run pytest -q` (**315 passed, 25 skipped**) succeeded. The skips are explicitly opt-in live/provider checks and the CI-supplied installed-wheel artifact. The focused terminal/render/CLI suite separately passed **24 tests** on macOS.
- Scope is one local arm64/Python 3.12 workflow job. GitHub-hosted CI, other Linux/macOS architectures and Python matrix entries, and visible terminal emulators remain unverified. T128/T129, T150/T152, and T154 remain open; cloud object-store tests remain deferred by user direction.

## France source-adapter acceptance — 2026-09-21

- The retained FRCOMP WMS PNG is now replay-tested against its SHA-256, manifest reference pixels, provider time, and raw-byte preservation. Decode explicitly remains unsupported because the WMS supplies no queryable pixel values or legend; the legacy luminance transform is not exposed as reflectivity.
- The source/inventory/CLI group passed **43 tests**, the full suite passed **316 tests with 25 skips**, and Ruff plus `git diff --check` passed. T063 is checked complete for the source adapter and scientific correction; T065 remains open for dBZ mapping and radar-native geometry.
- The current task count is **127 complete / 27 open**. SIDARMA/PAGASA/WU credentials remain unavailable or unusable and no values were used; UK remains a documented retired-service exception. S3-compatible/provider tests remain deferred as instructed.

## Current-worktree offline CI recheck — 2026-09-21

- Host checks passed: `uv run pytest -q` (**316 passed, 25 skipped**, one existing NumPy ABI warning), `uv run ruff check python tests`, `cargo fmt --all -- --check`, and `cargo test --workspace --locked` (**24 Rust integration tests**).
- Ran the actual `.github/workflows/offline.yml` `offline` job under official `act` v0.2.89 on the local Ubuntu 24.04 arm64 image with CPython 3.12.14. Locked sync, Ruff, Rust tests, and the full pytest suite (**316 passed, 25 skipped**) succeeded.
- Scope is one local CI matrix combination; no GitHub-hosted run or complete wheel matrix was executed. AWS S3, S3-compatible, and OSS tests remain deferred; the source, terminal-visual, and release gates remain open. The task list remains **127 complete / 27 open**.
- A subsequent attempt to run the separate `terminal-contract` workflow job stalled in its prerequisite `uv sync` for 7m7s and was interrupted before reaching the test step. This does not change the successful offline-job result or the 24 passing host terminal tests; no container terminal-job pass is claimed.

## TMD timestamp binding and live CLI regression — 2026-09-21

- Added a fail-closed Tesseract footer reader for TMD's 680×680 GIFs, validates all animation-frame times and the 15-minute KKN cadence, and records the raw SHA-256 in the `FrameRef` locator. Query-driven downloads share discovery context; a separately acquired ref rejects changed payload bytes.
- The first real split-request CLI attempt caught the mutable-GIF race and wrote no output. After query-driven acquisition was made atomic, actual `cmp1` and `kkn240Loop` raw-only CLI commands both wrote one complete frame. Their observation times were `2023-04-22T14:30:00Z` and `2023-04-22T16:00:05Z`; both endpoints were stale by more than three years at retrieval.
- TH + source CLI + migration inventory tests: **46 passed**. Ruff passed on changed Python files. Full-suite and current-wheels verification remain pending in this continuation.

## Current-worktree verification after TMD catalog correction — 2026-09-21

- Added a regression for the packaged TMD source resource: `time_semantics=rendered_footer_ocr_utc` and `timestamp_extractor=system_tesseract`. The TMD adapter code, migration evidence, and catalog metadata now agree.
- `uv run pytest -q`: **322 passed, 25 skipped**, with one pre-existing NumPy ABI warning. Source/CLI/inventory focus passed **48**; the standalone source CLI matrix passed **35**. Terminal/rendering/CLI/PTy contracts passed **24**.
- `uv run ruff check python tests`, `cargo fmt --all -- --check`, and `cargo test --workspace --locked` passed; the Rust suite ran **24 integration tests**. One hundred scoped JSON files parsed, `act -l` parsed the offline workflow's two jobs, and `git diff --check` passed.
- Rebuilt the macOS arm64 CPython 3.12 wheel and ran clean external child-venv tests for `core` and `all`: **1 passed each**. Both verify bundled TMD adapter/time helper/resource; core readback and all-extra NetCDF, GeoTIFF, and Zarr checks passed. SHA-256: `80b16d1b1a5c67815e634383243dbc13d2f3b0d7c1089f490e5bc85041d57cb8`.
- Local Act v0.2.89 successfully installed Tesseract in Ubuntu 24.04 arm64. Its `uv sync --group dev --locked` step remained in wheel-cache expansion for 22 minutes and was interrupted before lint/test steps; no container pass is claimed. No GitHub-hosted run was dispatched.
- T154's per-FR/SC readiness report is complete and concludes release/archive **blocked**. The task file now has **132 checked / 22 open**; open rows retain real source evidence, deferred provider, benchmark, and wheel-matrix gates. AWS S3, S3-compatible, and OSS remain deferred by explicit instruction.
- Final verification also exposed a scheduler-sensitive async stream-order test under three concurrent pytest processes. The test now controls completion with asyncio events instead of short sleep intervals; its deterministic ordering regression passed **5 consecutive runs**, and a subsequent standalone full-suite run passed **322 tests with 25 skips**.

## Lean CI rerun and blocked-adapter replay evidence — 2026-09-22

- Ran the current `.github/workflows/offline.yml` `offline` job to completion with SHA-verified official Act v0.2.89, Ubuntu 24.04 arm64, CPython 3.12.14, and the project's locked lean `ci` dependency group. Dependency sync, Ruff, `cargo test --workspace --locked` (**24 integration tests**), and pytest (**325 passed, 25 skipped**) all passed. The job took about 19m31s, mostly cold Linux arm64 wheel extraction/build work.
- The exact terminal/rendering/CLI command passed **24 tests** in a clean macOS CI-group venv. It verifies PTY bytes and protocol/CLI contracts; the terminal-contract workflow job was not separately run inside Act and GitHub-hosted Actions has not run this unpublished worktree.
- `tests/sources/test_id.py` plus the synthetic tile replay tests for BMKG, OpenSnow, and Weather Underground passed **10 tests**. The BMKG `id` test uses a fixture-only token and checks UTC parsing, safe references, and exact acquired bytes. Tile tests verify all four synthetic tile bytes and hashes, with decode kept fail-closed.
- The offline benchmark output records four measured and two unavailable representatives; the canonical SIDARMA/PAGASA inputs are unavailable. No comparable old-chain baseline exists. MY's measured fallback is a fixture-source acquisition, not the MY adapter or science path.
- AWS S3, S3-compatible, and Aliyun OSS remain deferred. No live credentials were copied or used in this continuation. External source evidence and the platform wheel matrix remain incomplete; after closing T028's completed shared-contract verification, the task count is **133 checked / 21 open**.

## SIDARMA and PAGASA offline branch verification — 2026-09-22

- `uv run pytest -q tests/sources/test_id_sidarma.py tests/sources/test_ph.py tests/sources/test_ph_fallback.py` passed **14 tests**. It exercises SIDARMA's CMAX windows, deduplication, time parsing and cancellation, plus PAGASA's HTTP and Playwright fallback conditions.
- Added a PAGASA token-redaction regression: the fixture-only token stays out of the frame reference, identity, safe JSON projection, and redacted request URL. The test passed without an implementation change.
- The only changed source artifact is replay coverage/evidence. Provider acceptance remains blocked by unavailable authorized credentials/raw frames and unverified physical palette/geometry; no key value was copied into the new repository.

## PAGASA token query encoding regression — 2026-09-22

- A new replay using a fixture-only token containing spaces and URL-reserved characters first failed because the adapter interpolated the token directly into the timeline URL.
- `PhSource` now URL-encodes the token query parameter. The regression verifies the server-side parsed value, Manila UTC conversion, and absence from frame identity/safe projections; PH HTTP and browser-fallback tests passed **8 tests**, and the complete source suite passed **112 tests**. Ruff passed for the changed adapter/test files.

## US1 read/acquisition and TMD migration closure — 2026-09-22

- T049's required sync/async, time-error, output, independent NetCDF, and CF checks are recorded in [`us1.md`](us1.md). The user's real `my --latest` CLI run wrote the requested frames, repeat execution skipped existing outputs, and `cat` read the generated NetCDF. The raw-only smoke separately wrote two frames and removed transient payloads after hashing because no reuse permission was available.
- T049 is checked for the read/acquisition verification scope; T011/T038 remain open for canonical MY raw permission, source-time binding, palette, and geometry.
- T104 is checked for the TMD adapter migration scope: retained `cmp1` and six-frame `kkn240Loop` GIFs replay through the registered adapter; OCR binds provider UTC observation times, `max_age` uses observation time, changed bytes are rejected, and historical queries fail explicitly. `tests/sources/test_th.py` passed in the **112-test source suite** and the **325-test full offline suite**.
- TMD palette/native-grid decoding remains fail-closed and tracked by T065. These task closures do not claim a complete scientific decoder.
- The task list is now **135 checked / 19 open**.

## Terminal CI job, current wheel, and live recheck — 2026-09-22

- Made `terminal-contract` independent of `offline` in `.github/workflows/offline.yml`. Ran the actual job on local Act v0.2.89 / Ubuntu 24.04 arm64 / CPython 3.13.15: locked `ci` sync passed and the workflow's PTY/rendering/terminal/CLI command passed **24 tests**. This proves the local Actions job path; no GitHub-hosted workflow was dispatched from the uncommitted worktree.
- Rebuilt current macOS arm64 CPython 3.12.7 wheel, SHA-256 `4cbb80fed7fe2cbc0f8911ffc546ad8d3ad2b498067e1670d852e69ac157e879`; clean isolated `core` and `all` tests each passed, with NetCDF/HDF5 plus GeoTIFF/Zarr data-quality readback. T150 remains partial because the hosted OS/Python matrix and x86_64 extra jobs are unverified.
- Fresh public live run: **16 latest raw-acquisition cases passed**, two credentialed SIDARMA/PAGASA cases were deselected, and the Taiwan numeric-grid case did not return before a `KeyboardInterrupt` at 518.48 seconds; BR SIPAM was not reached. The attempt is recorded as interrupted in `live.json`, not as a complete matrix. No credentials were used and outputs were not retained.
- Host PTY/terminal/CLI contracts passed **24 tests**; Ruff passed. `cargo fmt --all -- --check` and `cargo test --workspace --locked` passed, including all **24 Rust integration tests**. The current offline suite passed **326 tests / 24 skipped** with one existing NumPy ABI warning; the packaged-wheel artifact test ran because the current wheel is present. Cloud object-store tests remain deferred per instruction; the current task count remains **135 checked / 19 open**.

## Canonical and synthetic-only benchmark separation — 2026-09-22

- Extended the explicit offline benchmark for SIDARMA and PAGASA using deterministic `ReplayTransport` responses through their registered adapters. Both report `synthetic_replay_only`, keep `canonical_fixture_status=unavailable`, and report cold/warm request counts (SIDARMA 2/1; PAGASA 3/2) and cache hits without decoding the synthetic PNG as science.
- Re-ran `uv run python scripts/validation/benchmark_sources.py --offline --output validation-results/benchmarks.json`: **4/6 canonical measurements plus 2 synthetic-only adapter replays**. The output remains `task_status=partial`, contains no fixture token, and records no legacy speedup comparison. T148 remains open pending canonical SIDARMA/PAGASA raw evidence and an old-chain baseline.
- `tests/integration/test_benchmark_sources.py` passed against the new output, verifying the two synthetic-only labels, canonical-unavailable flags, warm-cache hits, request counts, and token non-disclosure. Ruff passed on the benchmark script and integration contract.

## MY adapter benchmark, x86_64 extras, and quickstart recheck — 2026-09-22

- Changed the benchmark's MY case from `FixtureSource` to deterministic HEAD/GET replay through the registered `MySource`. Its test first failed because the case was still reported as a canonical fixture measurement; after the change, the integration test passed. The benchmark now reports **3/6 canonical measurements** (RainViewer, AU, FR) and **3 synthetic-only adapter replays** (MY, SIDARMA, PAGASA). Cold/warm request counts are MY 3/2, SIDARMA 2/1, PAGASA 3/2; each warm replay observed a cache hit. No synthetic token is present in the report. T148 remains partial because the three canonical inputs and a matched old-chain baseline are unavailable.
- Built the current CPython 3.10.18 macOS x86_64 wheel under Rosetta, SHA-256 `1fb1e6d1e97e377004b0d353a4a10723c449c9da10a48ef09aa6daade00097f7`. Source-tree-outside constrained installed-wheel tests passed **8/8** (`core`, `geotiff`, `zarr`, `playwright`, `recovery`, `scraping`, `storage`, `all`), including dependency-isolation checks, packaged resources, NetCDF/HDF5 readback, and enabled GeoTIFF/Zarr round-trips. This is one local x86_64/Python combination, not native Intel, Linux, or hosted CI coverage.
- Quickstart local commands passed: source list returned 24 entries; doctor succeeded without a network probe; PNG text preview reported metadata unknown; offline E2E passed **2 tests**; terminal/render/CLI/PTy contracts passed **24 tests**. The independent terminal-contract job also passed locally under Act in the preceding recheck.
- Source migration inventory plus registered-source CLI matrix passed **39 tests**. This verifies current adapter contracts and inventory links, while leaving canonical payload, permission, time-binding, palette, and geometry gates open where recorded.
- After the benchmark change, `uv run --no-sync pytest -q` passed **326 tests with 24 explicit skips** and one existing NumPy ABI warning. Ruff, `cargo fmt --all -- --check`, and `cargo test --workspace --locked` passed (**24 Rust integration tests**); 100 scoped JSON files and all three workflow YAML files parsed, and `git diff --check` passed.
- The pinned legacy checkout audit found the recorded old SIDARMA/PAGASA/UK literals, no matching source credential variable in its root `.env`, and no WU key. Existing SIDARMA/PAGASA probes do not provide usable verified raw; UK is retired. No credentials were used or copied. The current task count remains **135 checked / 19 open**: canonical/source-science gates, user-deferred cloud providers, hosted platform matrix, and full quickstart acceptance remain open.

## Linux ARM64 wheel workflow correction and recheck — 2026-09-22

- The first local Act run reproduced a workflow defect: `UV_PROJECT_ENVIRONMENT` points to `$RUNNER_TEMP/radiust-venv`, but the wheel build named the nonexistent `.venv/bin/python`. Added `tests/contract/test_packaging_workflow.py`; it failed on that mismatch, then passed after `.github/workflows/wheels.yml` used `"$UV_PROJECT_ENVIRONMENT/bin/python"`.
- Re-ran the complete `wheels.yml` `build` job locally with Act v0.2.89, Ubuntu 24.04 arm64, and CPython 3.13.15. The wheel built as `radiust-0.1.0-cp313-cp313-manylinux_2_38_aarch64.whl` (SHA-256 `6f2519c5db1950a879ea123d537832ca6aa281219c0180d7d6bbbaffc1a2141a`); all **3 packaging tests** passed, including clean installed-wheel NetCDF/HDF5 readback. Artifact upload completed to Act's local temporary artifact server. This is one local matrix cell, not a hosted GitHub Actions run.
- After adding the workflow regression test, the full offline suite passed **327 tests with 24 explicit skips** and one existing NumPy ABI warning. Full Ruff passed; the wheel workflow test, all three workflow YAML files, 100 scoped JSON files, and `git diff --check` passed. The task count remains **135 checked / 19 open**; T150 remains open for the unrun matrix cells and hosted CI, and T152 remains partial.

## Isolated wheel matrix and adapter replay — 2026-09-22

- The first expanded local Act matrix showed that concurrent jobs could see stale wheels in the shared `validation-results/wheel` directory. A Python 3.10 job selected an incompatible macOS CPython 3.12 wheel and failed. Added workflow contract regressions, then isolated build and extra artifacts per matrix cell and explicitly passed the selected path as `RADIUST_WHEEL`.
- Re-ran the actual `wheels.yml` build job in four separate local Act runs on Ubuntu 24.04 arm64 for CPython 3.10.21–3.13.15. All four wheel builds, installed-wheel science/quality readbacks, and **3 packaging tests per job** passed. Each of the four Act artifact zips was inspected and contains exactly one wheel; wheel and zip hashes are recorded in [`packaging-matrix.json`](packaging-matrix.json). The macOS builds plus Linux ARM64 now cover **12/16 build cells**. The four Linux x86_64 cells and hosted Actions remain unrun.
- The focused offline adapter/inventory/CLI group passed **54 tests** across KR, PAGASA, Spain, Indonesia, Cambodia, Windy, Weather Underground, OpenSnow, BMKG, migration inventory, and the registered-source CLI matrix. This covers replay and fail-closed behavior; it does not replace unavailable canonical raw, credentials, or source-science evidence.
- After the workflow fix, the full default pytest suite passed **329 tests / 24 explicit live and provider skips**, with one existing NumPy ABI warning. Ruff, Act workflow listing, YAML/JSON parsing, and `git diff --check` passed. The separate local terminal-contract workflow remains **24/24 passed**; no GitHub-hosted run was triggered.
- The open task count remains **135 checked / 19 open**. T150 is partial because Linux x86_64 and the full hosted extra matrix remain unverified; T152 remains open. SIDARMA/PAGASA/UK/WU credential findings are in `provider-credential-audit.md`, and AWS/S3-compatible/OSS tests remain deferred by user direction.

## Wheel matrix completion and full-suite verification — 2026-09-22

- Local build coverage is **16/16** across the declared wheel build combinations. Linux x86_64 ran under AMD64 emulation on the arm64 host; macOS x86_64 ran under Rosetta. The four Linux x86_64 artifacts each contain exactly one matching wheel. See [`packaging-matrix.json`](packaging-matrix.json).
- All **28/28** Linux x86_64 Python/extra wheel smokes passed in four local Act runner jobs (CPython 3.10–3.13 × seven extras). The temporary harness aggregates the seven selections per Python and removes `needs: build` to use already-created local artifacts; it does not claim 28 independent production matrix jobs or hosted GitHub Actions.
- Current verification passed: Python **330 passed / 24 skipped**, Ruff, `cargo fmt --all -- --check`, Rust **24 integration tests**, four packaging workflow contract tests, YAML/JSON parsing, and `git diff --check`. The NumPy ABI warning is the existing warning in the Python suite.
- Local `terminal-contract` remains **24/24 passed**. Hosted Actions, native Intel hardware, visible terminal-emulator acceptance, credentialed provider tests, and user-deferred S3/OSS tests did not run.
- T150 is checked with these local matrix results and limitations recorded. Current status is **136 checked / 18 open**. T148 and T152 remain open; canonical source science and release readiness are still partial.

## Adapter response-boundary regression and benchmark refresh — 2026-09-22

- Four JSON-based adapters leaked `UnicodeDecodeError` when an upstream response contained invalid UTF-8. New tests reproduced the defect first; `EsSource`, `IdSidarmaSource`, `PtSource`, and `RainViewerSource` now convert both UTF-8 and JSON syntax failures to the public `DecodeError` type. The four new regressions and adjacent source tests passed **16 tests**; the full suite passed **334 tests / 24 explicit live-provider skips**, and Ruff passed.
- Re-ran `uv run --no-sync python scripts/validation/benchmark_sources.py --offline --output validation-results/benchmarks.json`. It records **3 canonical measurements and 3 synthetic-only adapter replays**; no synthetic token is present. `tests/integration/test_benchmark_sources.py` passed. T148 remains partial because synthetic MY/SIDARMA/PAGASA runs are not canonical source-performance measurements and there is no matched old-chain baseline.
- The task count remains **136 checked / 18 open**. This response-decoding fix improves error consistency but does not replace the missing source-specific raw, license, time, palette, or geometry evidence.

## PAGASA timeline failure classification — 2026-09-22

- Red-first tests reproduced three protocol failures: invalid UTF-8 and invalid JSON were misreported as no frames, while a malformed `data` value escaped as `AttributeError`.
- `PhSource` now validates the JSON root, `data` object, and `timeline` list and converts encoding/syntax/schema failures to `DecodeError`. A well-formed empty timeline remains valid.
- Focused PAGASA source/fallback tests: **11 passed**. The explicit macOS Chromium replay was blocked by host permission to register the Chromium Mach service; the browser was not able to start.
- Added a credential-free `browser-contract` GitHub Actions job and a passing workflow contract test. Local Act completed locked Playwright sync and OS dependency setup, then the Playwright CDN timed out downloading Chromium; the browser test step was not reached and hosted execution remains unverified.
- Full Python suite: **338 passed, 24 skipped**, one pre-existing NumPy ABI warning. Full Ruff, workflow YAML parsing, and whitespace checks passed.
- No credentials or provider calls were used. Migration remains **136 checked / 18 open**; missing canonical source evidence and user-deferred AWS/S3-compatible/OSS tests remain open.

## PAGASA browser-error redaction and terminal workflow recheck — 2026-09-22

- Added red-first regression coverage for browser exceptions that include the PAGASA tokenized timeline URL or a signed image URL. The adapter now exposes URL-free public errors; focused PAGASA tests passed **13 tests**, and Ruff passed for the changed Python files.
- Executed `.github/workflows/offline.yml`'s `terminal-contract` job through local Act v0.2.89 on Ubuntu 24.04 arm64 / CPython 3.13.15. Locked dependency sync succeeded and the workflow step passed **24 tests**. This is local runner evidence, not GitHub-hosted execution or visual emulator acceptance.
- Updated PAGASA evidence to record that an earlier loopback Chromium replay passed with generated bytes, while the latest host attempt was denied Chromium Mach bootstrap and the Act browser job timed out downloading Chromium before test execution. No credentials or provider calls were used.
- The workflow tests, real provider acceptance gates, and user-deferred AWS/S3-compatible/OSS cases remain separate: these local rechecks do not close T094 or the source-evidence and storage-provider tasks.
- A further red-first test found signed target URLs persisted in browser `RawFrame.metadata` and the default artifact name. The browser acquirer now drops `page_url` from public metadata and takes the artifact basename from the URL path. Browser, PAGASA fallback, and Windy regression tests passed **29 tests**.
- Fresh full verification after both URL-redaction fixes passed: **341 pytest passed / 24 skipped** (one existing NumPy ABI warning), full Ruff, all workflow YAML parsing, migration/benchmark JSON parsing, and changed-file newline/trailing-whitespace checks.

## Cross-adapter credential error redaction — 2026-09-22

- Red-first transport tests reproduced credential disclosure for configured `id` query tokens and `id_sidarma` `x-api-key` headers. `LegacyImageSource._get` and `_request` now remove configured credential values from GET/POST transport exception messages, including sensitive headers and request bodies; the PAGASA HTTP path uses the same boundary, and Weather Underground keeps a source-specific safe wrapper for its API-key query parameter.
- Focused regressions for `id`, SIDARMA, PAGASA, Weather Underground, and legacy POST transport passed **27 tests**. The KMA parser suite passed **4 tests**, including invalid UTF-8/JSON classification. Full verification passed **348 pytest tests / 24 explicit skips** (one existing NumPy ABI warning), full Ruff, Rust formatting, workflow YAML and migration/resource JSON parsing. The existing terminal-contract job remains locally verified at **24/24** under Act.
- The exact unauthenticated live smoke nodes for CAM, KMA/KR, AEMET/ES, and Windy passed **4/4** (21.05 seconds). Each verified artifact hash/size and closed its raw frame without retaining bytes; palette, time-binding, and native-geometry gates remain open. An earlier overbroad selection was stopped after eight unattributed passes and is recorded as non-acceptance evidence in `live.json`.
- Re-ran `.github/workflows/offline.yml`'s `terminal-contract` job with SHA-verified Act v0.2.89 on Ubuntu 24.04 arm64 / CPython 3.13.15. The locked `ci` sync completed and the workflow command passed **24 tests**; the local Actions job succeeded. GitHub-hosted Actions was not triggered.
- The task list remains **136 checked / 18 open**. The remaining source tasks require authorized canonical samples, provider time/palette/geometry evidence, or credentials; AWS S3, S3-compatible, and OSS acceptance remains deferred by user instruction.

## Catalog metadata and complete local recheck — 2026-09-22

- Added explicit `historical` capability to all 24 source resources and all catalog products, with latest-only and retired adapters marked false. The inventory contract checks resource capability against the registered adapter and product capability against the public catalog.
- Fixed `list products SOURCE --json`: passing frozen `ProductInfo` objects through `dataclasses.asdict()` attempted to pickle their `mappingproxy` units. The CLI now uses the shared safe JSON projection; `au`, `id_sidarma`, and multi-product `tw` regressions pass.
- Final offline verification: **365 passed / 24 skipped**, one existing NumPy ABI warning. Ruff, Cargo formatting, `cargo test --workspace --locked` (**24 Rust integration tests**), resource/migration JSON validation, benchmark integration, and `git diff --check` passed.
- The workflow-equivalent terminal-contract command passes **27 tests** locally, including PTY/ANSI and the frozen product-metadata JSON regression; hosted Actions and visible Kitty/iTerm2/SSH/tmux acceptance remain unverified.
- Offline T148 benchmark was regenerated with **3/6 canonical measurements and 3 synthetic-only adapter replays**. There is no matched old-chain baseline, so no acceleration claim is made. Remote provider tests and hosted GitHub Actions remain unrun; AWS S3, S3-compatible, and OSS remain deferred.
- Current task state: **136 checked / 18 open**. Open items are external evidence or explicitly deferred acceptance gates; no credentials were copied or printed.

## Resource metadata cleanup and benchmark-task status — 2026-09-22

- Removed duplicate `historical` keys from the six affected source resources and rechecked resource/migration JSON parsing.
- T148 is checked because the benchmark implementation, six cold/warm measurements, explicit canonical versus synthetic-only classification, output file, and incomparable legacy-baseline reason are present. The benchmark report remains `task_status=partial` because three inputs are synthetic and no matched old-chain baseline exists.
- Current task state is **137 checked / 17 open** at this intermediate benchmark-status checkpoint. The local-first quickstart record below subsequently checks T152; no credential value was copied or printed.

## Local-first quickstart execution record — 2026-09-22

- The local quickstart portions are recorded: isolated wheel/core and extra selections, offline E2E/source CLI matrix, terminal/CI contracts, public raw smoke, and explicit provider/credential skips. The current host suite is **365 passed / 24 explicit live-provider skips**; Ruff, Cargo formatting, the Rust workspace (**24 integration tests**), JSON parsing, and whitespace checks pass.
- The current-worktree local Act terminal job passed its workflow command with **24 tests**; the host-equivalent terminal command passes **27 tests** after the product-metadata regression. Hosted Actions and visible emulator/SSH/tmux inspection remain unverified.
- AWS S3, S3-compatible, and Aliyun OSS remain deferred by user instruction. The old-repository audit found no usable current SIDARMA/PAGASA/WU credentials and UK DataPoint is retired; no secret value was copied or printed.
- T152 is checked for completion of the local execution and evidence record. This does not close T084/T085 or the source-specific canonical science/evidence rows.

## Source-resource JSON contract recheck — 2026-09-22

- Added a contract that rejects duplicate keys in all source resource JSON files. The focused source/migration group passed **50 tests**; the full offline suite passed **366 tests / 24 explicit skips** with the existing NumPy ABI warning.
- Ruff and whitespace checks passed. The check protects local metadata integrity and leaves the external raw, credential, palette, geometry, and provider acceptance gates unchanged.

## Latest-only and tile response contract recheck — 2026-09-22

- The shared legacy tile adapter now rejects explicit `at`/range queries before discovery for Windy, OpenSnow, BMKG, and Weather Underground; MY has the same guard for its two current station endpoints. RainViewer remains the separate historical timeline adapter.
- Four-tile HTTP and Windy Playwright paths now require exactly four unique 256×256 image tiles and record the verified tile count/size in `RawFrame.metadata`. Invalid JSON from the legacy BMKG discovery endpoint now raises `DecodeError` instead of being silently reported as no data.
- The affected source suites passed in focused runs; the final full offline suite passed **357 tests / 24 explicit skips** with the existing NumPy ABI warning. The terminal/rendering/CLI contract subset passed **24 tests**, Rust formatting and all **24 Rust integration tests** passed, and Ruff plus migration/resource JSON parsing passed. These checks improve adapter protocol safety but do not close the still-blocked source evidence, credential, or provider-storage gates.
## Source resource registration contract recheck — 2026-09-22

- Added a migration-inventory contract covering all 24 HEAD sources: adapter module, versioned source resource, source-specific test, and registry ID must agree.
- The test caught and fixed the missing `source: my` field in `python/radiust/resources/sources/my.json`.
- `uv run --no-sync pytest -q -p no:cacheprovider tests/contract/test_migration_inventory.py`: **5 passed**; Ruff and `git diff --check` passed.

## Public adapter live raw recheck — 2026-09-22

- `RADIUST_TEST_ALLOW_LIVE=1 uv run --no-sync pytest -q tests/live/test_representative_sources.py -m 'live and not provider' -k public_latest_raw_smoke` passed **16 cases in 89.89 seconds**.
- The run covered the public CAM, Canada, Australia, RainViewer, France, Korea, Taiwan HTTP, Spain, Portugal, Singapore, Thailand (two products), Thailand Royal Rain, New Zealand, Vietnam, and Windy paths. Raw artifacts were hashed and each `RawFrame` was closed; no output was retained.
- SIDARMA/PAGASA credential tests, Taiwan's numeric scientific-grid test, and historical BR SIPAM were deselected. This verifies raw acquisition only; palette, physical values, native geometry, and credentialed cases remain open.

## Fixture and migration identity contract — 2026-09-22

- Added explicit `source` identity to the 16 HEAD fixture manifests that previously relied on a directory-name fallback.
- The contract now binds every HEAD fixture and migration record to its inventory ID and expected adapter path. `tests/contract/test_migration_inventory.py` plus `tests/sources/test_source_contract.py` pass **32 tests**; Ruff and `git diff --check` pass.

## Historical migration link contract — 2026-09-22

- Added explicit adapter/resource/fixture/migration/test links to the `br_sipam` historical evidence record.
- The inventory and source-fixture contracts now cover both historical Brazil records as well as all 24 HEAD rows; the focused group passes **33 tests**.

## Fixture schema identity contract — 2026-09-22

- `tests/fixtures/fixture.schema.json` and `tests/support/fixtures.py` now require `source` for every fixture manifest.
- All HEAD and historical manifests pass the stricter schema identity check; source/inventory/offline E2E verification passed **35 tests**.

## Full fixture-contract and benchmark recheck — 2026-09-22

- The complete offline suite passed **361 tests / 24 explicit skips** with the existing NumPy ABI warning; Ruff, migration/resource/fixture/validation JSON parsing, and `git diff --check` passed.
- The offline benchmark was regenerated and its integration contract passed. It still records **3/6 canonical measurements and 3 synthetic-only registered-adapter replays**, with no old-chain baseline or acceleration claim.

## Adapter/resource capability contract — 2026-09-22

- Corrected the missing `historical: false` declaration in `python/radiust/resources/sources/fr.json`.
- Added a contract asserting that every HEAD resource's historical capability matches its registered adapter. France, fixture, and inventory checks passed **39 tests**; Ruff and whitespace checks passed.

## Source resource version contract — 2026-09-22

- Added the missing `adapter_version: 1` fields to the BMKG, CAM, and OpenSnow resources.
- Inventory validation now requires every source resource version to match its catalog entry; the affected group passed **37 tests**.

## Local adapter migration scope closure — 2026-09-22

- Checked the local adapter scopes for T061, T091, T094, T096, T101, T102, T107, T108, T109, and T110. The source modules, resources, source tests, fixture manifests, and migration records are present and exercised; blocked providers remain explicit rather than being represented by fabricated canonical artifacts.
- SIDARMA now fails before network access when `sources.id_sidarma.api_key` is missing. The source, credential, tile, source-CLI, and inventory group passed **183 tests**; Ruff passed.
- The task file is now **148 checked / 6 open**. The six open rows are MY authorization/time/geometry, the SIDARMA representative evidence group, the global source-science/geometry gate, and the user-deferred AWS/S3-compatible/OSS tests. This local closure does not mark those external gates as passed.

## Current-worktree verification after adapter closure — 2026-09-22

- The complete local Python suite passed **367 tests / 24 explicit skips**, with the existing NumPy ABI warning. Ruff, JSON parsing, and `git diff --check` passed; `cargo fmt --all -- --check` and `cargo test --workspace --locked` also passed (**24 Rust integration tests**).
- The host terminal/CLI contract passed **27 tests**. The current-worktree local Act `terminal-contract` job on Ubuntu 24.04 arm64 / CPython 3.13.15 completed successfully with the same **27 tests**. This is local Actions evidence, not a GitHub-hosted run.
- The offline benchmark test and regeneration passed with **3/6 canonical measurements and 3 synthetic-only adapter replays**; no matched legacy baseline or acceleration claim is made. The task file is **148 checked / 6 open**.

## Provider configuration documentation recheck — 2026-09-22

- Added [`docs/live-provider-tests.md`](../docs/live-provider-tests.md) with secret-free runtime/CI variable names and bounded provider commands. UK DataPoint remains a retired fail-closed exception; AWS/S3-compatible/OSS remain deferred.
- Added a nested source-environment and redaction regression. The focused source, credential, tile, source-CLI, and inventory group passes **184 tests**; Ruff, migration JSON, and `git diff --check` pass.
- The subsequent complete local Python suite passed **368 tests / 24 explicit skips**; the existing NumPy ABI warning remains the only warning in that run. Rust format and workspace tests also pass (**24 integration tests**).
- The task count remains **148 checked / 6 open**: T011/T038, T053, T065, T084, and T085 remain external or explicitly deferred gates.
- `scripts/validation/audit_migration.py` ran against the current worktree and wrote `validation-results/migration-audit.json`: all 24 current source links are present, and the report correctly refuses to claim v1 readiness while open tasks remain.

## Migration audit command recheck — 2026-09-22

- The audit contract test and full local Python suite pass **369 tests / 24 explicit skips**; the existing NumPy ABI warning remains the only warning.
- The generated report records `source_count=24`, `missing=[]`, and **148 checked / 6 open** tasks. `ready_for_v1` remains `false` until the external/deferred gates are resolved.

## Offline CI wheel-selection recheck — 2026-09-22

- The first current-worktree Act `offline` run exposed a stale macOS arm64 wheel under `validation-results/wheel/` being selected inside the Linux arm64 container. The installed-wheel smoke now filters fallback artifacts by the interpreter's supported wheel tags; an explicitly supplied `RADIUST_WHEEL` remains authoritative.
- The packaging regression and host wheel smoke pass **2 tests**. The rerun of the current-worktree Ubuntu 24.04 arm64 / CPython 3.12.14 `offline` job passed the migration audit, Ruff, Rust (**24 integration tests**), and Python (**369 passed / 25 skipped**). The extra skip is the expected absence of a compatible wheel artifact in that local container.

## Final host regression after wheel selection fix — 2026-09-22

- The host complete Python suite passed **370 tests / 24 explicit skips** after the platform-tag regression; Ruff and `git diff --check` passed. The existing NumPy ABI warning remains the only warning.

## Weather Underground provider path recheck — 2026-09-22

- Added the explicit `RADIUST_TEST_WUNDERGROUND_API_KEY` provider smoke and wired the same name into the manual GitHub Actions provider job. With no key configured, the opt-in WU case skipped before creating a network context; the focused run reported **1 skipped / 20 deselected**.
- The WU adapter remains credential/science/time-binding blocked. No key was found in the legacy checkout and no provider request was sent.

## Credential-free provider CI recheck — 2026-09-22

- Ran the current-worktree `.github/workflows/live.yml` `provider` job through local Act on Ubuntu 24.04 arm64 / CPython 3.13.15 with `run_provider=true` and no secrets. Locked dependency setup succeeded and all three credentialed smokes skipped before network access: SIDARMA, PAGASA, and Weather Underground (**3 skipped, 18 deselected**); the job succeeded.
- This validates the runnable opt-in CI path and secret-free behavior. It is local Act evidence, not a hosted GitHub Actions run, and it does not close the credentialed source-science gates.
