# Quickstart execution record

Date: 2026-09-17

The CPython 3.13 local run completed the offline `my` list/discover/fetch/decode/download/readback/repeat-skip path. Local output, raw replay, optional GeoTIFF/Zarr, batch/cache and terminal contract tests passed. A clean macOS arm64 wheel was built and imported outside the source tree; list and doctor worked with temporary cache/output paths.

The full quickstart remains open because it requires canonical source raw evidence, every migrated source, AWS/S3-compatible/OSS provider runs, real Kitty/iTerm2/ANSI records, all declared wheel combinations and the benchmark matrix. Those omissions are recorded in the corresponding validation files rather than treated as skips that pass.

## Recheck on 2026-09-20

- `uv run pytest -q`: **238 passed, 13 skipped, 1 warning**; eight opt-in public-source cases, SIDARMA credentials, three unconfigured remote storage providers, and the CI-supplied clean-wheel case remain explicit skips.
- `uv run ruff check python tests`: passed. `cargo fmt --all -- --check` and `cargo test --workspace`: passed, including **24 Rust integration tests**.
- `uv run pytest -q tests/integration/test_source_cli_matrix.py`: **17 passed**. This covers shared CLI/SDK plumbing with `FixtureSource` for raw-backed manifests and one recorded RainViewer adapter replay; it does not claim all sources are live adapters or scientifically validated.
- The focused local output/raw-replay suite passed **25 tests**. Independent `netCDF4 1.7.4` readback and CF Checker 4.1.0 on the offline NetCDF fixture passed with zero errors and warnings.
- The explicit public live smoke remains **8/8 passed** as recorded in `validation-results/live.json`; provider credentials, physical source validation, real terminal visual checks, the remaining Linux/macOS x86_64 packaging CI jobs, and the benchmark matrix remain open.
- A separate RainViewer live CLI run wrote one latest frame with four raw tiles; text cat, exact-time repeat-skip, independent netCDF4 readback, and CF Checker all passed. Details are in `live-cli-rainviewer.json`.

The current status of the source files and licensing/time/geometry evidence is summarized in `validation-results/us6-inventory.md` and `migration/blockers/`. Passing shared CLI plumbing or NetCDF conformance does not close those source-level requirements.

## Implementation continuation — 2026-09-20

- `uv run pytest -q`: **286 passed, 16 skipped, 1 existing NumPy binary-compatibility warning**. The live/provider and clean-wheel skips remain explicit; they are not acceptance evidence.
- `uv run pytest -q tests/integration/test_source_cli_matrix.py tests/contract/test_migration_inventory.py`: **35 passed**.
- The opt-in public raw smoke passed **9 cases, 2 deselected**. The separate real Chromium localhost replay passed **1 case** after fixing browser cleanup order. Neither run supplies MY, SIDARMA, PAGASA-provider, or image-palette acceptance evidence.
- `validation-results/packaging-matrix.json` still records the macOS arm64 combinations as passed and the declared Linux/macOS x86_64 CI jobs as pending. `validation-results/storage-providers.json` still records AWS, S3-compatible, and Aliyun OSS as not run. The real terminal visual matrix remains partial.

T152 remains open because the complete provider, terminal, platform, and source-science steps are not yet satisfied.

## Continuation — 2026-09-21

- Built CPython 3.11.10, 3.12.7 and 3.13.0 `macosx_11_0_x86_64` wheels under Rosetta and ran the matching clean installed-wheel smoke for each: **3/3 passed**. Along with the separately recorded 3.10 x86_64 run, local Rosetta packaging smoke now covers CPython 3.10–3.13. Native GitHub Intel CI and macOS 11 runtime remain unverified; details and wheel hashes are in `packaging-matrix.json`.
- Updated `.github/workflows/wheels.yml` to `macos-15-intel`, because GitHub retired the `macos-13` hosted runner label in December 2025. The workflow YAML and four Intel matrix rows parse locally; no workflow was dispatched and no push was made.
- Re-ran the offline test set: **286 passed, 2 skipped, 14 deselected**; Rust workspace: **24 integration tests passed**; lint and formatting checks passed. The explicit source inventory/CLI matrix passed **35 tests**.
- Re-ran the offline six-source benchmark: 4/6 sources measured; SIDARMA and PAGASA have no canonical fixture frame. No old-chain matched baseline is available, so no acceleration conclusion is drawn.
- Full quickstart remains open: external credentials/sample authorization, isolated AWS/S3-compatible/OSS prefixes, native Kitty/iTerm2/SSH/tmux visual sessions, native hosted CI, all source science/geometry references, and dependent full-inventory/benchmark gates remain unavailable. See `storage-providers.json`, `terminal-matrix.md`, `packaging-matrix.json`, `benchmarks.json`, and `us6-inventory.md`.

- Re-ran the no-credential public live matrix with `RADIUST_TEST_ALLOW_LIVE=1`: **9 passed, 2 deselected** (AU, RainViewer, FR, Taiwan HTTP, Spain, Portugal, Singapore, Thailand, and Taiwan native numeric grid). Image palette/native-grid claims are not inferred from raw acquisition alone; detailed scope is in `live.json`.


## Current implementation recheck — 2026-09-21

- Full offline suite: **292 passed, 2 skipped, 14 deselected**; Rust workspace: **24 integration tests passed**; Ruff and cargo formatting passed.
- Source/CLI plus inventory matrix: **36 passed**; terminal/CLI contract CI subset: **13 passed**, including a real POSIX PTY subprocess CLI smoke.
- A fresh CPython 3.12 macOS arm64 wheel passed clean source-tree-outside `core` and `all` installed-wheel checks, including NetCDF science/quality readback plus GeoTIFF/Zarr readback. GitHub Actions YAML parses, but hosted CI remains unrun.
- MY registered adapter has synthetic two-station HEAD/GET CLI replay and a successful live two-station raw-only CLI run with TLS verification enabled. The GIF-advertised endpoint bytes verified as PNG and were deleted after hash/size/dimension checks; canonical authorized raw, accepted valid-time semantics, palette and native geometry are still missing. See `validation-results/live-cli-my.json`.
- Per user instruction, AWS/S3-compatible/OSS tests remain deferred. Real Kitty/iTerm2, SSH/tmux, visual inspection and interrupt recovery remain open.

T152 remains open because these checks do not supply the required provider, terminal, hosted-CI or source-science evidence.


- `uv run pytest -q`: **302 passed, 7 skipped** in 80.60 seconds. The run included nine public online cases (eight latest raw adapters and the Taiwan numeric grid); missing provider credentials and the three deferred remote-storage configurations remained skipped. See `live.json` for scope.


## Default live-test opt-in correction — 2026-09-21

- The earlier full default run (302 passed, 7 skipped) unexpectedly included nine public online tests. A missing collection-time guard caused that; the report retains it as live evidence.
- Tests now require `RADIUST_TEST_ALLOW_LIVE=1`; credentialed provider tests additionally require `RADIUST_TEST_ALLOW_PROVIDER=1`. With the variables unset, all 12 live cases skipped before network access.
- The current default `uv run pytest -q` result is **295 passed, 16 skipped**. Setting the explicit live flag still passes the bounded RainViewer probe (1 passed). `.github/workflows/offline.yml` now runs plain default pytest to enforce this policy in CI.

## Current adapter, terminal and local evidence — 2026-09-21

- Rechecked the full offline default: **303 passed, 16 skipped** in 11.45 seconds. Skips are opt-in live/provider cases and the CI-supplied wheel artifact; AWS/S3-compatible/OSS live tests remain deferred per user instruction.
- All 24 catalog source IDs have an adapter module, source resource, test module and migration record. The standalone source/CLI matrix has **33 passed**. This proves local registered-adapter replay where raw fixtures exist, not science for every provider; missing upstream frames/credentials and unverified palettes/geometry remain listed in `us6-inventory.md` and `migration/blockers/`.
- A current CPython 3.12 macOS arm64 wheel passed clean installed-wheel smoke for `core` and `all`; NetCDF science/quality plus GeoTIFF/Zarr readback passed. Wheel SHA-256 is recorded in `packaging-matrix.json`.
- The terminal/CLI subset passed **16 tests**, including PTY ANSI output and regular-grid display orientation. Real Kitty/iTerm2, SSH/tmux and Ctrl-C visual acceptance are still open.
- The Ubuntu x86 wheel jobs now use the `ubuntu-24.04` runner label. `act -l` parses the jobs, but local execution cannot start without the OrbStack Docker daemon; hosted Actions remains unrun.

The full quickstart is still incomplete while source-specific science/geometry, authorization, terminal visual, hosted matrix and deferred storage-provider evidence remain open. T152 stays unchecked.


## Adapter and terminal recheck — 2026-09-21

- The opt-in command `RADIUST_TEST_ALLOW_LIVE=1 uv run pytest -q -m 'live and not provider' tests/live/test_representative_sources.py` passed **15 tests, 2 provider cases deselected** in 84.08 seconds. It covers 13 public-catalog raw acquisitions, the Taiwan numeric-grid decode, and a direct historical-only BR SIPAM acquisition. Raw acquisition alone does not verify image palettes or native geometry.
- The actual raw-only CLI path also passed for KR, NZ, VN, and Windy: each command wrote one frame, and the complete output manifest plus artifact hashes verified before temporary cleanup. Details are in `live-cli-public.json`; this confirms CLI acquisition/commit only, not scientific decode.
- Extra bounded probes confirmed current VN, NZ, KR, and Windy raw acquisition; a CA discovery exceeded its 20-second budget, and Royal Rain discovery returned a transport error. Hashes and statuses are in `live.json` and the corresponding migration records. OpenSnow/BMKG 403 and credential-blocked sources remain unaccepted.
- The default offline suite passed **304 tests, 22 expected skips, one existing NumPy ABI warning**. Source and migration inventory contracts passed **98 tests**; terminal, renderer, and CLI contracts passed **17 tests**, including the POSIX PTY ANSI process. Ruff, `cargo fmt`, `cargo test --workspace --locked` (24 Rust integration tests), 95 scoped JSON files, and `git diff --check` passed.
- `act -l` parsed the offline and live workflows. Local workflow execution stopped before job steps because the OrbStack Docker daemon is unavailable. The GitHub-hosted workflows were not dispatched. The terminal matrix remains partial for actual emulator, SSH/tmux, visual, and Ctrl-C recovery evidence.
- AWS S3, S3-compatible, and Aliyun OSS live tests remain deferred per instruction. T152 remains open with the source-science, credential, visual-terminal, provider, and hosted-platform gates.

## Adapter continuation — 2026-09-21

- Fixed Cambodia slideshow acquisition by restoring the legacy `Accept`, `Accept-Language`, and `User-Agent` headers. Its offline source test and explicit opt-in live test pass; the real latest raw-only CLI wrote and hash-verified one 78,539-byte image. The raw remains temporary because fixture reuse terms are not documented; SRI palette bins and native map geometry remain unverified.
- The full public opt-in smoke now passes **10 cases, 2 deselected**: nine raw acquisitions and the Taiwan numeric grid. OpenSnow and BMKG still return HTTP 403 from Cloudflare; no payload was written.
- Added the missing BR SIPAM migration record and made the inventory contract require migration records for historical adapters. The contract now also requires every blocked migration record to contain blocker details and evidence.
- Current default suite: **304 passed, 17 skipped**, with one existing NumPy ABI warning. Ruff, `cargo fmt`, all 24 Rust integration tests, JSON parsing, workflow YAML parsing, and `git diff --check` passed.
- Rebuilt the CPython 3.12 macOS arm64 wheel after the adapter change. Clean source-tree-outside `core` and `all` installed-wheel smoke each passed; the new wheel SHA-256 is recorded in `packaging-matrix.json`.

T152 remains open for the other source-science and authorization gates, deferred object-store providers, physical terminal acceptance, and hosted platform CI.

## Local PAGASA browser/session replay — 2026-09-21

- `RADIUST_TEST_ALLOW_LIVE=1 RADIUST_TEST_REAL_BROWSER=1 uv run --extra playwright pytest -q tests/live/test_ph_browser_replay.py`: **1 passed in 2.01s** after the default macOS sandbox denied Chromium Mach bootstrap before the test reached its loopback server; the replay passed through managed review.
- The test exercises a local CSRF/cookie-protected timeline and generated PNG. It makes no PAGASA request and provides no provider token, source-time, palette, or geometry evidence.
- The overall quickstart remains incomplete because the provider, scientific source, physical terminal, hosted CI matrix, and deferred object-store acceptance gates remain open.

## Public adapter CLI raw-only matrix — 2026-09-21

- Ran the actual CLI with `runtime.allow_network: true`, 15-second request timeout, 90-second frame deadline, and no cache: `uv run radiust --conf <temporary>/online.json download <source> --latest [--station <station>] --raw-only --no-cache --output <temporary>/<source> --json`.
- CAM, AU/AU02, RainViewer, FR, tw-http/CV1_3600, ES/ESCOMP, PT/PTST2, SG/SGCOMP, TH/cmp1, KR/BRI, NZ/NZAU2, VN/DHA, Windy/global, CA/CASFT, and Royal Rain/takhli all returned exit code 0 and wrote one frame. Across all 15, each manifest was complete and artifact paths, sizes, and SHA-256 values matched before temporary outputs were deleted; see `live-cli-public.json`.
- A station-query-aware discovery hook fixed unnecessary multi-station crawling: CA/CASFT went from a 39.59-second full directory scan to 3.57 seconds, and Royal Rain/takhli changed from an outer 130-second timeout to a verified write in 43.66 seconds. The source tests first reproduced the extra station requests, then passed after the fix.
- At this matrix checkpoint the TH CLI value still reflected retrieval time; the correction below supersedes that result with footer-OCR UTC binding. The matrix itself checks discovery/acquisition and CLI commit only, not scientific decode, palettes, or native geometry.

## Live, terminal, and default-CI recheck — 2026-09-21

- `RADIUST_TEST_ALLOW_LIVE=1 uv run pytest -q -m 'live and not provider' tests/live/test_representative_sources.py`: **17 passed, 2 credentialed cases deselected** in 157.61 seconds after the TLS trust-bundle update. The matrix covers 15 public catalog raw acquisitions, Taiwan's numeric grid, and historical-only BR SIPAM.
- `uv run pytest -q`: **307 passed, 24 expected skips**. All 20 live cases skipped before network access; the other skips are the three deferred remote-storage tests and the CI-supplied wheel artifact.
- The source/inventory suite passed **100 tests** and the terminal/renderer/CLI suite passed **17 tests**. All three workflow YAML files parsed locally. Hosted platform CI and actual Kitty/iTerm2/SSH/tmux visual acceptance were not run; local `act` cannot start without Docker.
- AWS S3, S3-compatible, and OSS live checks remain deferred as instructed. Full quickstart acceptance remains open for source science/geometry, available provider credentials, physical terminal checks, and hosted packaging CI.

## Latest source, terminal, and wheel recheck — 2026-09-21

- MY live raw-only CLI wrote both stations with TLS verification enabled; PNG payload signatures, sizes, dimensions, hashes, and Last-Modified metadata were checked in temporary outputs and then deleted. See `live-cli-my.json`; T011/T038 remain open for reuse permission and accepted time/geometry evidence.
- `uv sync --locked --group dev`, Ruff, `cargo fmt --all -- --check`, and `cargo test --workspace --locked` passed. The current CPython 3.12 macOS arm64 wheel passed clean `core` and `all` installed-wheel tests outside the source tree, including NetCDF/HDF5 and GeoTIFF/Zarr readback.
- The PTY/terminal contracts pass, but iTerm2 UI automation was denied by the desktop safety boundary. Hosted Actions was not run: this implementation remains only in the local worktree. AWS/S3-compatible/OSS and SIDARMA/PAGASA credentialed cases remain unrun by instruction or credential status.

## Local continuation — 2026-09-21

- Adapter/source/CLI contracts passed **120 tests**; focused identity, model, bridge, raw-replay, transport, and network-isolation contracts passed **22 tests**. `cargo test --workspace --locked` passed all **24 Rust integration tests**.
- Terminal, renderer, CLI output/acquisition/cat contracts passed **24 tests**, including ANSI output in a real POSIX PTY. Kitty/iTerm2 visual review and SSH/tmux/Ctrl-C acceptance remain unavailable.
- An additional real CLI latest raw-only request for Thailand `kkn240Loop` wrote one verified raw GIF and complete manifest. A later correction OCR-bound the footer time to the exact GIF SHA-256; the current checked result is 2,666,961 bytes, SHA-256 `98b6e087ddf5ff01e139f862409eae65c288d47e44603d67e3e0e224b49cb5cd`, with observation time `2023-04-22T16:00:05Z`. Native geometry and palette remain unverified.
- SIDARMA/PAGASA/UK/WU credential findings are summarized without values in [`provider-credential-audit.md`](provider-credential-audit.md). The user-deferred AWS/S3-compatible/OSS tests were not run.

## Final local implementation and CI recheck — 2026-09-21

- The exact default offline command passed **311 tests with 25 expected skips** and one existing NumPy ABI warning. Ruff, `cargo fmt --all -- --check`, and `cargo test --workspace --locked` passed; all 24 Rust integration tests passed.
- The source/CLI/benchmark focused group passed **34 tests**; terminal, renderer, and CLI contracts passed **24**, including a real POSIX PTY ANSI subprocess. This does not satisfy the visible Kitty/iTerm2/SSH/tmux and Ctrl-C acceptance.
- The latest CPython 3.12 macOS arm64 wheel passed clean isolated `core` and `all` installation/readback tests. SHA-256: `5fea54f0f8b933277f121e516bd57d58281f91f9467cb14188420790bfd1c473`.
- All 24 catalog IDs have adapter-module mappings. This establishes migration code presence, not completion of the source-specific sample, authorization, timestamp, palette, or geometry acceptance gates. SIDARMA/PAGASA/UK/WU credential availability is recorded without values in `provider-credential-audit.md`; unavailable or unusable provider credentials were not used.
- Workflow YAML parses locally, but `act` could not start jobs because Docker is unavailable and GitHub-hosted CI was not dispatched from this local worktree. AWS S3, S3-compatible, and OSS remain deferred as requested. The task list is **126 complete / 28 open**; quickstart/release readiness remains incomplete.
- The final migration-inventory plus source-CLI focused recheck passed **38 tests** (34 source CLI matrix, plus four migration-inventory checks); the static registry/resource audit verified all **24/24** catalog source mappings.

## TMD retained animation replay — 2026-09-21

- Retained the official `kkn240Loop.gif` response in the TH fixture and replayed it offline. The test checks exact bytes/hash, 680×680 dimensions, all six frames and their durations, OCR-bound UTC footer times, and rejection of unverified scientific decode.
- `uv run pytest -q` passed **314 tests with 25 expected skips** and one existing NumPy ABI warning; the TH/migration-inventory focused tests passed **9 tests**, the registered source CLI matrix passed **34**, and Ruff passed.
- Quickstart acceptance remains open: source-specific science/geometry, physical terminal review, hosted platform CI, and the user-deferred remote object-store matrix still require their own evidence.

## Current implementation continuation — 2026-09-21

- Recovered one legacy SIDARMA `*_source.png` from `旧项目/output/id/` and added hash/dimension verification. Its fixture remains blocked and `frames: []`: the old output omitted the discovery response and image URL, the old client disabled TLS verification, and the filename time is not independently bound to provider metadata. Palette and native grid remain unknown; no token value was copied.
- `uv sync --locked --group dev`, Ruff, Cargo formatting, and `cargo test --workspace --locked` passed; the Rust suite ran 24 integration tests. `uv run pytest -q` passed **315 tests with 25 explicit skips** and one existing NumPy ABI warning.
- The full terminal/rendering/CLI contract subset passed **24 tests**; the source CLI matrix passed **34 tests**. This verifies automated PTY/protocol contracts, not visible Kitty/iTerm2 output or SSH/tmux/Ctrl-C recovery.
- Built macOS arm64 wheels for CPython 3.10–3.13 and passed **32/32** clean installed-wheel selections across core, each extra, and all. Earlier local Rosetta x86_64 core wheel smokes passed for 3.10–3.13 (4/4). Linux hosted builds and Ubuntu extra jobs remain unrun; details are in `packaging-matrix.json`.
- Re-ran the offline six-source benchmark and refreshed `benchmarks.json`: **4/6 measured**. SIDARMA and PAGASA remain unavailable without canonical raw fixtures, and no matched legacy baseline exists; no acceleration claim is made.
- The exact local offline and terminal CI commands pass. No hosted workflow ran from this unpublished worktree. The old credential audit remains secret-safe in `provider-credential-audit.md`; no configured provider secret was pasted or used. AWS S3, S3-compatible, and OSS remain deferred by instruction.
- A temporary, release-hash-verified `act` v0.2.89 run reached local job setup, then stalled pulling the Linux/arm64 runner image and was cancelled before steps. This is recorded as unrun; host-equivalent local CI checks remain green. The previously installed `act` binary was not used for job execution because it predates the published security fixes.

The quickstart is still open for canonical/source-science evidence, authenticated SIDARMA/PAGASA/WU cases, visible terminal sessions, hosted Linux/Intel CI, remote storage tests, and dependent full-inventory gates. The task list was **126 complete / 28 open** at this checkpoint.

## Local Actions runner recheck — 2026-09-21

- Successfully executed `.github/workflows/offline.yml`'s `offline` job under local `act` v0.2.89 with a temporary Ubuntu 24.04 arm64 runner, selecting Python 3.12.14. Locked dependency sync, Ruff, the Rust workspace (24 integration tests), and pytest (**316 passed, 25 explicit skips**) all passed in the latest run.
- This is one local Linux/arm64/Python 3.12 job, not a hosted Actions run or the full packaging/platform matrix. The focused macOS terminal/rendering/CLI contracts passed **24 tests**; visual Kitty/iTerm2 and SSH/tmux acceptance is still open.
- No configured SIDARMA, PAGASA, UK, or WU credential was used; missing/invalid credential cases remain skipped. AWS S3, S3-compatible, and OSS tests remain deferred as requested. Adapter code maps all 24 catalog IDs, while source-specific raw provenance, time-binding, science, and geometry gates remain recorded separately.

## France adapter continuation — 2026-09-21

- Added a real retained WMS-frame replay contract for the FR adapter, including SHA-256, every manifest RGBA sample, provider time, exact raw preservation, and fail-closed scientific decode. The focused source/inventory/CLI group passed **43 tests**; the full suite passed **316 tests with 25 expected skips**; Ruff passed.
- T063 is now complete as the FRCOMP display-only migration and historical luminance correction. T065 still blocks a physical color-to-dBZ mapping and radar-native grid; T053 remains open because there is no authorized SIDARMA raw sample. Current count: **127 complete / 27 open**.

## TMD observation-time correction and real CLI recheck — 2026-09-21

- The current public `cmp1` and `kkn240Loop` GIFs were downloaded with `radiust download th --latest --station ... --raw-only --no-cache`. Both raw-only CLI runs wrote one complete manifest and the exact raw bytes inspected by the timestamp parser.
- The output now reports the footer observation times: `cmp1` `2023-04-22T14:30:00Z` (158,551 bytes, SHA-256 `55c279de79da7f45acefe2fbff1a03faa1e15b0dadd186d2c2d8b7a93451999e`); `kkn240Loop` `2023-04-22T16:00:05Z` (2,666,961 bytes, SHA-256 `98b6e087ddf5ff01e139f862409eae65c288d47e44603d67e3e0e224b49cb5cd`, six 15-minute footer times).
- A split discovery/acquire against the mutable endpoint changed bytes and was rejected before output. Query-driven downloads now preserve one operation context and hash-bind acquisition to discovery. The current endpoints still serve 2023 observations; `max_age` rejects them as stale. TMD palette and native geometry remain blocked.
- The TH source test, migration-inventory test, and source CLI matrix passed **46 tests**; Ruff passed on all changed Python files. GitHub Actions installs Tesseract for the full source suite. See [`tmd-source.md`](../docs/tmd-source.md) and [`live-cli-public.json`](live-cli-public.json).

## Current-worktree TMD, adapter, terminal, and packaging recheck — 2026-09-21

- `uv run pytest -q`: **322 passed, 25 explicitly skipped**, with one existing NumPy ABI warning. Skips are opt-in provider/live cases and the CI-supplied wheel artifact; credentials were not used.
- TMD + source CLI + migration inventory contracts passed **48 tests**; the source CLI matrix passed **35 tests**. The full terminal/rendering/CLI/PTy subset passed **24 tests**. Ruff, `cargo fmt --all -- --check`, and `cargo test --workspace --locked` passed; Rust ran **24 integration tests**.
- The async stream completion-order contract now uses event gates instead of wall-clock sleep ordering; it passed **5 consecutive runs**, followed by another standalone full-suite pass.
- Rebuilt the CPython 3.12 macOS arm64 wheel after correcting the packaged TMD resource's time semantics. Clean source-tree-outside `core` and `all` wheel smokes each passed (**1 passed**); the child process checked `ThSource`, the OCR helper, and the bundled TMD resource. Wheel SHA-256: `80b16d1b1a5c67815e634383243dbc13d2f3b0d7c1089f490e5bc85041d57cb8`.
- `.github/workflows/offline.yml` parsed with `act -l`. The local Ubuntu 24.04/arm64/Python 3.12 job installed Tesseract successfully, then spent 22 minutes in `uv sync --group dev --locked` expanding dependency wheels and was cancelled before Ruff, Rust, or pytest steps. This is **not** a container CI pass. Host runs of those checks are recorded above; GitHub-hosted Actions has not run from this uncommitted workspace.
- TMD `--latest` now reports OCR-bound UTC observation times and reuses the exact inspected GIF bytes for query-driven CLI downloads. Both public endpoints returned April 2023 frames during September 2026 retrieval; palette and native geometry remain unverified.
- AWS S3, S3-compatible, and Aliyun OSS remain deferred as requested. SIDARMA/PAGASA/UK/WU credential status is recorded without values in `provider-credential-audit.md`. Release and legacy-repository archival remain blocked; after closing the readiness-report task, **132 tasks are checked and 22 remain open**.

## Local adapter, benchmark, and CI continuation — 2026-09-22

- The offline `offline` Actions job completed under SHA-verified Act v0.2.89 on Ubuntu 24.04 arm64 / CPython 3.12.14: locked `ci` sync, Ruff, **24 Rust integration tests**, and **325 passed / 25 skipped** Python tests. The focused PTY/rendering/CLI contract suite passed **24 tests** in a clean `ci` environment.
- Synthetic offline replays for `id`, BMKG tiles, OpenSnow tiles, and Weather Underground passed **10 tests** for discovery/acquisition, raw-byte hashes, safe time/identity fields, and fixture-key redaction. These tests deliberately confirm that unverified scientific decoding still fails closed; they do not satisfy real-source raw or geometry acceptance.
- `uv run python scripts/validation/benchmark_sources.py --offline --output validation-results/benchmarks.json` completed: **4/6 measured**, with `id_sidarma` and `ph` unavailable because no usable canonical fixture exists. MY uses `FixtureSource`; FR and AU are raw-replay measurements. No matched legacy baseline exists, so no acceleration claim is made.
- Credential audit found legacy literals for SIDARMA, PAGASA, UK, and BMKG's old `id` scraper; SIDARMA returned 403, PAGASA produced no verified raw, UK DataPoint is retired, and no WU key was found. Values remain out of the new repository. AWS S3, S3-compatible, and Aliyun OSS remain deferred per user instruction.
- This remains a partial quickstart: MY/SIDARMA/PAGASA and other source science/geometry evidence, deferred provider tests, remaining platform wheel jobs, and full hosted Actions execution are outstanding. After closing T028, T049, and T104 on their verified scopes, the task list is **135 checked / 19 open**.

## Current local acceptance progress — 2026-09-22

- The standalone `.github/workflows/offline.yml` `terminal-contract` job passed under the verified local Act v0.2.89 runner on Ubuntu 24.04 arm64 / CPython 3.13.15. Locked `ci` sync succeeded and the actual workflow command passed **24 tests**. The terminal job now runs independently of the longer offline matrix; GitHub-hosted Actions has not been triggered from this local worktree.
- Rebuilt the current CPython 3.12.7 macOS arm64 wheel (SHA-256 `4cbb80fed7fe2cbc0f8911ffc546ad8d3ad2b498067e1670d852e69ac157e879`). Clean child-environment `core` and `all` installed-wheel tests passed, including NetCDF/HDF5, GeoTIFF, and Zarr readback. The remaining declared OS/Python/extra wheel matrix is open.
- A fresh public latest-raw run passed **16 source cases** with TLS verification enabled and no credentials. It then stalled in the Taiwan numeric-grid smoke and was interrupted after 518.48 seconds; the BR SIPAM case was not reached. The two credentialed tests were deselected. Prior dated passing results remain recorded separately; no new source-science or geometry claim is made.
- Re-ran the offline source benchmark: **4/6 canonical fixture paths measured**, plus deterministic SIDARMA/PAGASA synthetic-only adapter replays (cold/warm requests 2/1 and 3/2 respectively). Both canonical fixtures remain unavailable, no provider bytes/credentials are included, and no old-chain comparison is claimed; T148 remains open.
- The current offline suite passed **326 tests / 24 explicit skips** with one existing NumPy ABI warning; the packaged-wheel artifact test ran now that the current wheel is present. Ruff, Cargo formatting, and all **24 Rust integration tests** pass. The task file remains **135 checked / 19 open**. T150 lacks hosted Linux/Intel matrix results; T152 remains open. AWS S3, S3-compatible, and OSS testing remains deferred as requested.

## Local quickstart continuation — 2026-09-22

- `uv run --no-sync radiust list sources --json` returned all **24** catalog entries. `uv run --no-sync radiust doctor` exited successfully with local dependencies/cache/output writable and `network_probe=not_requested`. `radiust cat --file tests/fixtures/sources/my/raw/east.png --renderer text` reported PNG dimensions and `metadata=unknown`, without inferring radar geometry or values.
- `uv run --no-sync pytest -q -p no:cacheprovider tests/contract/test_offline_e2e.py`: **2 passed**. The workflow-equivalent terminal/render/CLI command passed **24 tests**, including PTY coverage. The local `terminal-contract` Actions job had already passed through Act; no hosted Actions run was triggered.
- `uv run --no-sync pytest -q -p no:cacheprovider tests/contract/test_migration_inventory.py tests/integration/test_source_cli_matrix.py`: **39 passed**. All source adapter records and the local registered-source CLI matrix passed their contracts; this does not replace missing canonical source samples or scientific evidence.
- Built current CPython 3.10 x86_64 macOS wheel under Rosetta: `radiust-0.1.0-cp310-cp310-macosx_11_0_x86_64.whl`, SHA-256 `1fb1e6d1e97e377004b0d353a4a10723c449c9da10a48ef09aa6daade00097f7`. All eight constrained isolated-wheel selections (`core`, each of six extras, and `all`) passed outside the source tree with NetCDF/HDF5 and enabled GeoTIFF/Zarr readback. A local Act Ubuntu 24.04 arm64 / CPython 3.13.15 `wheels.yml` job also passed its wheel build, **3 packaging tests**, and local artifact upload; wheel SHA-256 `6f2519c5db1950a879ea123d537832ca6aa281219c0180d7d6bbbaffc1a2141a`. Other Linux/native-Intel/Python combinations and hosted matrix jobs remain unverified.
- The benchmark now exercises MY, SIDARMA, and PAGASA through their registered adapters with deterministic synthetic responses, labeled synthetic-only. `benchmarks.json` reports **3/6 canonical measurements + 3 synthetic-only replays**; there is no matched old-chain baseline, so T148 stays partial. No synthetic token appears in the report.
- Current full offline verification: **327 passed, 24 explicit skips, 1 existing NumPy ABI warning**; Ruff, Cargo formatting, and `cargo test --workspace --locked` (24 Rust integration tests) passed. All **100 JSON** evidence/resource files and **3 workflow YAML** files parsed; `git diff --check` passed. The new workflow contract test verifies Maturin uses the interpreter in `UV_PROJECT_ENVIRONMENT`.
- The old pinned checkout contains legacy SIDARMA, PAGASA, and UK key/token literals, but its `.env` has no corresponding credential variable names; WU has no legacy key. Existing probes remain unusable/inconclusive for SIDARMA/PAGASA and UK is retired, so no credentialed live test ran. Values were not copied or printed. Adapter implementation files exist, but source-specific tasks remain open where canonical samples, authorization, time binding, physical palette, or geometry are unverified. AWS S3, S3-compatible, and OSS remain deferred. T152 is still partial.

## Linux ARM64 matrix and adapter replay continuation — 2026-09-22

- Fixed local wheel-matrix contamination: each build/extra cell now uses an isolated wheel directory and passes its one selected wheel through `RADIUST_WHEEL`. The new packaging workflow contract tests first failed against the shared-directory workflow, then passed after the fix.
- Ran the actual `.github/workflows/wheels.yml` build job with local Act v0.2.89 for Ubuntu 24.04 arm64 / CPython 3.10.21, 3.11.16, 3.12.14, and 3.13.15. All four built and installed the cell's own wheel; **3 packaging tests passed in each job**. Each local artifact archive contains exactly one wheel; wheel and archive hashes are recorded in `packaging-matrix.json`. This raises local platform build coverage to **12/16**; Linux x86_64 build cells and GitHub-hosted jobs remain unverified.
- Source tests for KR, PAGASA, Spain, Indonesia, Cambodia, Windy, WU, OpenSnow, and BMKG, plus migration inventory and source CLI contracts, passed **54 tests**. These verify offline adapter/replay behavior and CLI/inventory integration; missing canonical payloads, authorization, time semantics, palette, and geometry remain open where listed.
- The current full default suite passed **329 tests, 24 explicit live/provider skips**, and one existing NumPy ABI warning. Ruff passed, the wheel workflow parsed with Act, JSON/workflow YAML parsed, and `git diff --check` passed. The separate local Act `terminal-contract` job remains verified at **24 passing tests**; GitHub-hosted Actions was not dispatched.
- No credential value was copied or used. SIDARMA's legacy key returned 403, PAGASA yielded no verified raw frame, UK DataPoint is retired, and no legacy WU key was found. AWS S3, S3-compatible, and OSS remain deferred as requested. The task list remains **135 checked / 19 open**; T150 and T152 stay open for the remaining platform/provider/source-evidence gates.

## Packaging matrix completion and current full recheck — 2026-09-22

- Completed all **16/16** declared wheel build/core cells locally: macOS arm64, macOS x86_64 under Rosetta, Ubuntu 24.04 arm64 under Act, and Ubuntu 24.04 x86_64 under AMD64 emulation. Each Linux wheel passed its packaging tests and clean installed-wheel core smoke; each local Linux artifact archive contains exactly one matching wheel. Hashes and the environment caveat are in [`packaging-matrix.json`](packaging-matrix.json).
- All **28/28** Linux x86_64 extra selections passed for CPython 3.10–3.13: `geotiff`, `zarr`, `playwright`, `recovery`, `scraping`, `storage`, and `all`. The local Act harness grouped selections into four Python-version runner jobs and retained artifact download/selection, locked CI sync, wheel bootstrap install, and the installed-wheel test. It removed `needs: build` to consume artifacts already produced locally and looped over extras to avoid repeated runner setup; the 28 independent production matrix jobs and GitHub-hosted Actions were not dispatched.
- The full Python suite passed **330 tests, 24 explicit live/provider skips**, with one existing NumPy ABI warning. Ruff and `cargo fmt --all -- --check` passed; `cargo test --workspace --locked` passed all **24 Rust integration tests**. The packaging workflow contract tests passed **4/4**; workflow YAML and validation JSON parsed, and `git diff --check` passed.
- The independent local `terminal-contract` Actions job remains **24/24 passed** on Ubuntu 24.04 arm64 / CPython 3.13.15. This covers automated PTY and CLI contracts; visible Kitty/iTerm2 review, SSH/tmux interaction, native Intel execution, and hosted Actions remain unverified.
- The source adapter inventory and offline replay evidence remain as recorded above. Open source tasks require canonical/authorized raw data or verified time, palette, and geometry. SIDARMA's old key returned 403, PAGASA produced no verified frame, UK DataPoint is retired, and no WU key was found; no credential values were copied or used. AWS S3, S3-compatible, and Aliyun OSS remain deferred per instruction.
- T150 is complete for the declared build and extra selections with local/emulation limits recorded. T148 remains partial pending representative canonical benchmark inputs and a comparable legacy baseline; T152 remains open because provider/source-science, terminal visual, hosted-CI, and deferred storage gates are incomplete. The task list is **136 checked / 18 open**.

## Adapter response-boundary regression and benchmark refresh — 2026-09-22

- Four JSON-based adapters leaked `UnicodeDecodeError` when an upstream response contained invalid UTF-8. New tests reproduced the defect first; `EsSource`, `IdSidarmaSource`, `PtSource`, and `RainViewerSource` now convert both UTF-8 and JSON syntax failures to the public `DecodeError` type. The four new regressions and adjacent source tests passed **16 tests**; the full suite passed **334 tests / 24 explicit live-provider skips**, and Ruff passed.
- Re-ran `uv run --no-sync python scripts/validation/benchmark_sources.py --offline --output validation-results/benchmarks.json`. It records **3 canonical measurements and 3 synthetic-only adapter replays**; no synthetic token is present. `tests/integration/test_benchmark_sources.py` passed. T148 remains partial because synthetic MY/SIDARMA/PAGASA runs are not canonical source-performance measurements and there is no matched old-chain baseline.
- The task count remains **136 checked / 18 open**. This response-decoding fix improves error consistency but does not replace the missing source-specific raw, license, time, palette, or geometry evidence.

## PAGASA timeline failure classification — 2026-09-22

- Added red-first replay cases for non-UTF-8 JSON, invalid JSON syntax, and a malformed timeline schema. Before the fix, the first two were reported as `NoDataError` and the malformed object escaped as `AttributeError`.
- `PhSource` now raises the public `DecodeError` for invalid encoding, invalid JSON, or missing/non-object `data` and non-list `timeline`; an explicit empty timeline remains a valid no-frame response.
- PAGASA source/fallback tests passed **11 tests**. The default suite skips the opt-in browser replay. An explicit macOS run could not launch Chromium because the host denied Chromium's Mach bootstrap registration (`Permission denied`), so it is not counted as a browser pass.
- Added a credential-free `browser-contract` GitHub Actions job for the loopback CSRF/session replay. Its workflow regression passed; local Act completed locked Playwright sync and OS dependencies, then Chromium download from the Playwright CDN timed out after retries before the test ran. GitHub-hosted execution remains unverified.
- The full offline suite passed **338 tests / 24 skipped** with the existing NumPy ABI warning. Full Ruff, workflow YAML parsing, and whitespace checks passed.
- No credentials or provider calls were used. The 18 open tasks are unchanged; canonical PAGASA raw, authorized credentials, and verified palette/geometry remain external acceptance requirements.

## PAGASA URL-error redaction and terminal CI recheck — 2026-09-22

- Red-first tests reproduced Playwright-style exceptions echoing the PAGASA timeline token and a signed image URL. `PhSource` now maps browser failures to URL-free public errors while preserving authentication, missing-dependency, and resource-limit categories. Discovery and acquisition redaction regressions plus PAGASA source/fallback tests passed **13 tests**; no provider request or credential was used.
- Re-ran the workflow's `terminal-contract` job with local Act v0.2.89 on Ubuntu 24.04 arm64 / CPython 3.13.15. The locked dependency sync succeeded and the job command passed **24 tests** in 2.07 seconds. GitHub-hosted Actions and visible terminal emulator rendering remain unverified.
- Updated the PAGASA audit and migration record to distinguish the earlier passing loopback browser replay from the latest blocked host retry and Act CDN timeout. The browser CI job is configured; hosted execution remains unverified.
- A follow-up raw-manifest regression found the signed URL could also appear in browser `RawFrame.metadata` and the fallback artifact filename. `BrowserAcquirer` now omits the navigation URL and derives filenames from the URL path without query/fragment data. The red-first manifest test failed before the fix; browser, PAGASA fallback, and Windy tests passed **29 tests** afterward.
- Fresh full verification after both URL-redaction fixes: `uv run --no-sync pytest -q -p no:cacheprovider` — **341 passed, 24 skipped**, with one existing NumPy ABI warning; `uv run --no-sync ruff check python tests scripts` passed. All three workflow YAML files, the updated PAGASA migration JSON, and benchmark JSON parsed; changed paths passed newline/trailing-whitespace checks.

## Cross-adapter credential error redaction — 2026-09-22

- Fixture-only transport failures verify that public errors do not echo the configured Indonesia BMKG token, SIDARMA `x-api-key`, PAGASA timeline token, Weather Underground key, or a sensitive header sent through the shared legacy POST path. The GET/POST boundaries sanitize request credentials, and the tile adapter applies the same safety to its API-key request.
- Focused source and credential regression group: **27 passed**; the KMA source suite also passed **4 tests**, including invalid UTF-8/JSON handling. Current full offline suite: **348 passed / 24 explicit skips**, with one existing NumPy ABI warning; Ruff, Rust formatting, and workflow/migration/resource JSON parsing passed. The terminal-contract workflow’s local Act result remains **24/24 passed**.
- No live credentials or credentialed provider requests were used. The exact unauthenticated latest-raw smokes for CAM, KR, ES, and Windy passed **4/4**; each RawFrame was closed and its bytes were not retained. The migration remains **136 checked / 18 open**; credentialed cases, missing canonical source-science evidence, and user-deferred S3/OSS tests remain open.
- Re-ran the actual `.github/workflows/offline.yml` `terminal-contract` job through SHA-verified local Act v0.2.89 on Ubuntu 24.04 arm64 / CPython 3.13.15. Locked `ci` sync and the workflow command passed; **24 tests** passed. GitHub-hosted Actions remains untriggered.

## Catalog metadata and complete local recheck — 2026-09-22

- Added explicit historical capability to every HEAD source resource and catalog product. Latest-only adapters (MY, SIDARMA, TMD, tile families, UK, and current WU/OpenSnow/BMKG locators) now advertise `historical: false`; timeline/directory adapters expose the capability they implement. Taiwan's current numeric grid still accepts an exact discovered `at` frame while its product metadata does not claim an archive.
- `radiust list products au --json`, `id_sidarma --json`, and `tw --json` now serialize frozen `ProductInfo.units` safely. The prior `dataclasses.asdict()` path raised `cannot pickle 'mappingproxy' object`; the regression test covers all three shapes and `list stations` remains covered.
- Final local recheck after this change: **365 pytest passed / 24 explicit skips**, with one pre-existing NumPy ABI warning; Ruff, `cargo fmt --all -- --check`, `cargo test --workspace --locked` (**24 Rust integration tests**), JSON parsing, benchmark integration, and `git diff --check` passed.
- The current terminal-contract command from `.github/workflows/offline.yml` passes **27 tests** locally, including PTY/ANSI and the CLI product-metadata regression. This is automated terminal/CI contract evidence; hosted Actions and visible Kitty/iTerm2/SSH/tmux review remain unverified.
- `benchmark_sources.py --offline` was regenerated: **3/6 canonical measurements plus 3 synthetic-only registered-adapter replays**; no old-chain comparison is claimed. T148 remains partial because MY/SIDARMA/PAGASA canonical inputs and a matched legacy baseline are unavailable.
- The task file remains **136 checked / 18 open**. T011/T038/T053/T061/T065 and the evidence-gated source tasks still need authorized raw, palette, time, or geometry evidence; T084/T085 stay deferred for AWS S3, S3-compatible, and OSS; T152 remains partial. No credential value was copied or used.

## Resource metadata cleanup and benchmark-task status — 2026-09-22

- Removed duplicate `historical` keys from the six affected source resources; JSON parsing now has one authoritative capability value per resource.
- T148 is now checked because its implementation, six cold/warm measurements, output report, synthetic-only labeling, and legacy-baseline limitation are all recorded. `validation-results/benchmarks.json` intentionally remains `task_status=partial`: three inputs are synthetic adapter replays and no comparable old-chain baseline exists.
- The task file is **137 checked / 17 open** at this intermediate benchmark-status checkpoint. The local-first quickstart record below subsequently checks T152; no credential value was copied or printed.

## Local-first quickstart execution record — 2026-09-22

- Executed the local portions of this guide: isolated installed-wheel checks (16/16 declared build/core cells and 28/28 extra selections already recorded), offline E2E, source CLI matrix, and the workflow-equivalent terminal command. The current host suite is **365 passed / 24 explicit live-provider skips**, Ruff and Cargo formatting pass, and the Rust workspace reports **24 passing integration tests**.
- The terminal/CI route is runnable: the current-worktree local Act `terminal-contract` job passed its 24-test workflow command, and the host command now passes 27 tests after the product-metadata regression. Public live raw smoke evidence is recorded separately; SIDARMA/PAGASA/WU credential cases are skipped because the credential audit found no usable current test credentials, and UK is retired.
- AWS S3, S3-compatible, and Aliyun OSS tests are intentionally deferred per user instruction. Source-specific canonical raw, license, palette, time-binding, and native-geometry gaps remain explicitly listed by source. These are recorded as unaccepted evidence, not converted into success claims.
- T152's local execution and evidence-recording work is complete. The release remains blocked by the still-open source/provider evidence rows and the user-deferred storage matrix; no credential value was copied or printed.

## Source-resource JSON contract recheck — 2026-09-22

- Added a migration contract that rejects duplicate keys in every source resource JSON, preventing silent metadata overwrite. The source/adapter and migration focus passed **50 tests**; the full offline suite passed **366 tests / 24 explicit skips** with the existing NumPy ABI warning.
- Ruff and `git diff --check` passed. This closes a local metadata-regression gap; it does not change the external raw, credential, palette, geometry, or provider gates.

## Adapter migration scope closure — 2026-09-22

- The local implementation portion of the remaining source migrations is complete for SIDARMA, Korea, PAGASA, Spain, legacy Indonesia, Cambodia, Windy, Weather Underground, OpenSnow, and BMKG. Source-specific replay and inventory checks passed **183 tests** after SIDARMA was changed to require an explicit external API key.
- The local task count is **148 checked / 6 open**. MY authorization, SIDARMA representative raw/science evidence, the all-source science gate, and the user-deferred object-storage provider matrix remain external acceptance work.

## Current-worktree terminal CI rerun — 2026-09-22

- The local Actions `terminal-contract` job completed on Ubuntu 24.04 arm64 / CPython 3.13.15 with the locked `ci` dependency group and **27 passed** terminal, rendering, PTY, and CLI contract tests. The host command and full local suite agree.
- This validates the runnable CI path selected for terminal acceptance. It does not represent a hosted GitHub Actions run or visible Kitty/iTerm2/SSH/tmux inspection.

## Current provider configuration recheck — 2026-09-22

- [`docs/live-provider-tests.md`](../docs/live-provider-tests.md) now records the secret-free runtime and GitHub Actions variable names for the credentialed source paths and the exact opt-in command. No credential value is stored in the repository.
- The environment mapping/redaction regression and source inventory checks pass **184 focused tests**. The remaining six task rows are unchanged external/deferred gates, so the quickstart does not claim a full online or remote-storage acceptance.
- The offline migration audit also passes structural validation for **24/24** current sources and writes `validation-results/migration-audit.json`; it reports the six open gates instead of treating them as completed.

## Migration audit recheck — 2026-09-22

- The audit contract is included in the full offline run: **369 passed / 24 explicit skips**. The report is structural evidence for adapter coverage and does not replace canonical provider science evidence.

## Offline CI wheel-selection recheck — 2026-09-22

- A current-worktree local Act run first found that a macOS wheel in the local validation directory was being installed in Linux. The fallback wheel selector now checks platform tags; the repaired Ubuntu arm64 job passed with **369 passed / 25 skipped**.
- The runnable CI path now includes both the migration audit and the platform-safe installed-wheel smoke. The explicit `RADIUST_WHEEL` path used by the wheel matrix remains unchanged.

## Final host regression after wheel selection fix — 2026-09-22

- The complete host Python suite passed **370 tests / 24 explicit skips**. Local Act remains verified at **369 passed / 25 skipped** because the Linux container has no compatible fallback wheel; both results are expected and documented.

## Weather Underground provider path recheck — 2026-09-22

- The manual provider job now accepts `RADIUST_TEST_WUNDERGROUND_API_KEY`; its raw tile smoke skips before network access when unset. The legacy audit still has no usable WU credential.

## Credential-free provider CI recheck — 2026-09-22

- Ran the manual `provider` job from `.github/workflows/live.yml` with `run_provider=true` through local Act on Ubuntu 24.04 arm64 / CPython 3.13.15. Dependency setup and the job completed successfully; SIDARMA, PAGASA, and Weather Underground each skipped before any provider request because their test variables were absent (**3 skipped, 18 deselected**).
- The exact secret-free workflow route is executable. Hosted GitHub Actions remains unrun, and the six external/deferred task rows remain unchanged.
