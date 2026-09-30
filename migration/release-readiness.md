# radiust v1 release and migration readiness

Checked: 2026-09-30

Decision: **blocked for release and legacy-repository archival**.

This report maps the specification's functional requirements and success criteria to implementation evidence. A checked implementation task means its scoped code or local contract passed; it does not override a source-specific blocker or turn a deferred provider test into a pass.

## Inventory and accepted exceptions

- The offline catalog contains 24 current source IDs, and the migration inventory maps all 24 to adapter/resource/migration records. It also maps both historical Brazil implementations. The source/CLI replay matrix exercises all non-exception local fixtures; it does not count synthetic shared-plumbing fixtures as physical source validation. See [`us6-inventory.md`](../validation-results/us6-inventory.md) and [`source-cli-matrix.json`](../validation-results/source-cli-matrix.json).
- The user deferred AWS S3, S3-compatible, and Aliyun OSS live acceptance. T084/T085 remain open; local filesystem and in-memory object-storage contracts do not replace those tests. See [`storage-providers.json`](../validation-results/storage-providers.json) and [`us3.md`](../validation-results/us3.md).
- The user instructed that credentialed source acceptance be skipped when usable credentials are absent. The legacy SIDARMA literal returned HTTP 403; the PAGASA token did not yield a verified raw frame; UK DataPoint is retired and its key was not sent; no Weather Underground key was found. No credential value is reproduced here. See [`provider-credential-audit.md`](../validation-results/provider-credential-audit.md).
- SIDARMA recovery on 2026-09-30: a user-provided capture key works with the replacement UTC `radar/v1/arsip` endpoint. JAK archive metadata and a 4084×4084 raw PNG are retained with hashes; the old ssx route remains rejected. This removes the JAK canonical acquisition blocker, while palette, native pixel geometry, all-station and historical acceptance remain open. See [`id_sidarma.md`](blockers/id_sidarma.md).
- For terminal acceptance, the user selected a runnable GitHub Actions path in place of interactive Kitty/iTerm2/SSH/tmux visual review. PTY and protocol contracts are present in the `terminal-contract` job; this is the user-selected acceptance path, while emulator screenshots and interactive recovery remain unverified. See [`terminal-matrix.md`](../validation-results/terminal-matrix.md).
- `.specify/memory/constitution.md` is still the untouched placeholder template. This report does not represent it as ratified project governance.

## Functional requirement mapping

| Requirement | Evidence | Readiness |
| --- | --- | --- |
| FR-001 | T034/T044/T114; source catalog and [`us6-inventory.md`](../validation-results/us6-inventory.md) | Implemented; all catalog entries have a route. Per-source acceptance is tracked separately. |
| FR-002 | T009/T034/T046/T142; registry and catalog contracts | Implemented for offline catalog/configuration and explicit probing. |
| FR-003 | T029/T033; query contract tests | Shared UTC and mutually exclusive query validation passes. |
| FR-004 | T029/T032; T038 remains open; [`my.md`](blockers/my.md) | Partial: common selection checks pass; MY lacks accepted canonical time/geometry evidence. TMD footer time is now bound and stale `max_age` is enforced. |
| FR-005 | T012/T016/T070; identity contracts | Implemented and locally verified. |
| FR-006 | T035/T081; T038 remains open; [`th.json`](sources/th.json) | Partial: integrity/version checks pass; source time-binding and mutable-source evidence remain incomplete for some adapters. |
| FR-007 | T022/T025/T144; resource-lifecycle and cancellation regressions | Implemented and locally verified. |
| FR-008 | T013/T015/T020; scientific model contracts | Shared single/multi-variable, quality, grid, and provenance structures are implemented; source coverage remains limited by FR-036. |
| FR-009 | T013/T020/T050; [`us2.md`](../validation-results/us2.md) | Framework semantics pass; not every source has verified physical values or palette references. |
| FR-010 | T037/T050/T054/T055; decoder contracts | Exact/threshold palette rules and unknown-color behavior are implemented; source-specific rules remain unverified where fixtures are missing. |
| FR-011 | T051/T056/T057; tile contracts | Tile byte/color/alpha behavior is covered locally; blocked providers have no raw-backed validation. |
| FR-012 | T019/T058/T113; grid contracts | Grid models and transforms are implemented; several sources still lack native control points. |
| FR-013 | T052/T059/T060; regrid contracts | Explicit target-grid conversion and interpolation contracts pass; this does not validate each source geometry. |
| FR-014 | T030/T042/T044; SDK and CLI acquisition tests | Separate and combined acquisition paths are implemented and tested. |
| FR-015 | T130/T134/T135/T136; batch/stream tests | Ordered batch results, bounded streaming, and partial-result behavior pass local contracts. |
| FR-016 | T039/T077/T078/T079/T080; local format tests | PNG+sidecar, NetCDF, GeoTIFF, and Zarr v2 local writer/readback contracts pass; installed-wheel dependency bundling remains unverified. |
| FR-017 | T031/T039/T068/T077/T078; writer/readback tests | NetCDF values, quality, time, CRS, coordinates, and provenance pass Rust and independent xarray fixture readback; source/output coverage is partial. |
| FR-018 | T041/T071/T072; T084 remains open | Local output is implemented. Remote-provider behavior is explicitly deferred, so the full requirement is not accepted. |
| FR-019 | T066/T070; path/identity contracts | Root confinement and identity collision handling pass locally. |
| FR-020 | T066/T073/T081; manifest and repair tests | Completion manifests, validation, skip, and repair behavior pass local contracts. |
| FR-021 | T040/T066/T073; overwrite/cancellation regressions | Explicit overwrite and commit-fence behavior pass local tests. |
| FR-022 | T067/T074/T075/T082; raw-only/replay tests | Raw, raw-only, replay, and local integrity behavior pass. |
| FR-023 | T131/T139/T140; cache lifecycle tests | Local cache bounds, dry-run GC, ownership, and cleanup contracts pass. |
| FR-024 | T074/T131/T138; cache repair tests | Local corruption, index repair, leases, and shared-blob protection pass. |
| FR-025 | T018/T132/T141/T142; configuration tests | Precedence, validation, and redaction contracts pass. |
| FR-026 | T021/T035/T133/T143; resource-limit tests | Shared request/frame/byte/pixel/tmp limits and retry budgets pass local fault tests. |
| FR-027 | T025/T130/T144; cancellation regressions | Cancellation cleanup and late-commit prevention pass local tests. |
| FR-028 | T046/T123/T140/T142; CLI contracts | Required list/discover/download/cat/config/doctor/cache commands are present and contract-tested. |
| FR-029 | T045/T132/T137; CLI reporting tests | Machine/human output and exit-status contracts pass locally. |
| FR-030 | T017/T027/T145; logging/redaction tests | Stable error/report fields and secret redaction pass local contracts. |
| FR-031 | T117/T123; terminal input contracts | File/source input and selection validation are implemented. |
| FR-032 | T068/T076/T117; render contracts | Display is separated from scientific arrays; several sources correctly remain `unknown` where references are absent. |
| FR-033 | T121/T125/T126/T128/T129; [`terminal-matrix.md`](../validation-results/terminal-matrix.md) | User-selected runnable CI/PTY acceptance path is implemented. Real terminal-emulator and interactive SSH/tmux behavior was not inspected. |
| FR-034 | T003/T006/T048; T150 checked; [`packaging-matrix.json`](../validation-results/packaging-matrix.json) | All 16 declared wheel build/core cells and all 28 Python/extra selections passed in local validation. Linux x86_64 used AMD64 emulation; GitHub-hosted and native Intel execution were not run. |
| FR-035 | T009/T034/T114; inventory audit | All current and historical inventory entries have migration routes or recorded exceptions. |
| FR-036 | T010/T086/T116/T147; [`source-cli-matrix.json`](../validation-results/source-cli-matrix.json) | Blocked: several sources lack a canonical raw fixture or source-backed decode/grid/time references. Offline replay coverage is not equivalent to this requirement. |
| FR-037 | T111/T112/T115; this report | Migration differences and unavailable/retired sources are recorded. Old-repository archival remains blocked pending source and release gates. |

## Success criteria

| Criterion | Evidence | Result |
| --- | --- | --- |
| SC-001 | T009/T116/T114; [`migration/inventory.json`](inventory.json) | Inventory coverage passes: 24 current IDs and two historical entries are accounted for. This says nothing about science acceptance. |
| SC-002 | T086/T116/T149; `T065` open; [`us6-inventory.md`](../validation-results/us6-inventory.md) | Blocked: the required real sample and science acceptance is missing for multiple sources; credentialed tests are skipped per user instruction. |
| SC-003 | T029/T032/T066; `T038` open; TMD time evidence in [`th.json`](sources/th.json) | Partial: shared query and TMD mutable-time cases pass; source-bound time evidence is incomplete for MY and other blocked adapters. |
| SC-004 | T013/T050/T052/T068; `T065` open; [`us2.md`](../validation-results/us2.md) | Blocked: not all sources have accepted reference pixels, palettes, grids, and independent readback evidence. |
| SC-005 | T066/T067/T082; `T084/T085` open; [`us3.md`](../validation-results/us3.md) | Local contract evidence passes; full three-target acceptance is deferred and remains incomplete. |
| SC-006 | T130/T133/T146; [`us5.md`](../validation-results/us5.md) | Local batch, stream, limit, and cancellation criteria pass. |
| SC-007 | T131/T138/T146; [`us5.md`](../validation-results/us5.md) | Local cache concurrency, repair, GC, and no-cache comparison pass. |
| SC-008 | T117/T118/T128/T129; [`terminal-matrix.md`](../validation-results/terminal-matrix.md) | User-selected CI/PTY path is accepted. Literal Kitty/iTerm2 screenshots and interactive SSH/tmux records are not available. |
| SC-009 | T048/T124; T150; [`packaging-matrix.json`](../validation-results/packaging-matrix.json) | Local matrix acceptance passes: 16/16 wheel build/core cells and 28/28 extra selections. Results are local/emulated, not GitHub-hosted; the report preserves that distinction. |
| SC-010 | T132/T145/T146; [`us5.md`](../validation-results/us5.md) | Local configuration, report, exit-code, and secret-redaction cases pass. |
| SC-011 | T148; [`benchmarks.json`](../validation-results/benchmarks.json) | Partial: RainViewer/AU/FR use canonical replay inputs; MY/SIDARMA/PAGASA have synthetic adapter-only measurements because permitted canonical raw is unavailable. No matched old-chain baseline or speedup claim exists. |

## Remaining release blockers

1. MY raw reuse/license acceptance and source-bound valid-time/native geometry (`T011`, `T038`, `T049`).
2. MY authorization/time/geometry, SIDARMA representative raw/science evidence, and the global source-science/geometry gate (`T011`, `T038`, `T053`, `T065`). UK is already recorded as the accepted DataPoint retirement exception; credential-gated PAGASA/WU and the blocked provider adapters now have local fail-closed migration contracts.
3. AWS S3, S3-compatible, and OSS provider tests deferred by the user (`T084`, `T085`).
4. Complete canonical six-source benchmark evidence; T148's implementation/report and T152's local quickstart execution record are checked, while their evidence limits remain explicit.
5. GitHub-hosted Actions and native Intel execution were not dispatched; local Act/worktree checks pass but do not claim hosted-run evidence.

**Release/archival conclusion:** do not publish v1 or archive `旧项目` yet. The adapters and shared SDK/CLI are implemented, and T150's local wheel matrix plus T152's local quickstart record are complete. Source science/evidence, deferred storage providers, incomplete canonical benchmark evidence, and hosted/native platform evidence remain material external acceptance gaps.

## Current worktree recheck — 2026-09-22

- `scripts/validation/audit_migration.py` reports all 24 current source adapter/resource/test/fixture/migration links present, with `missing=[]`; it reports **148 checked / 6 open** tasks and keeps `ready_for_v1=false`.
- The current-worktree local Act Ubuntu 24.04 arm64 offline job passed the migration audit, Ruff, Rust (**24 integration tests**), and Python (**369 passed / 25 skipped**). The extra skip is the expected absence of a compatible wheel artifact in that local container.
- The installed-wheel smoke now filters fallback wheels by the active interpreter's platform tags. An explicitly supplied `RADIUST_WHEEL` remains required and authoritative in the declared wheel matrix.
- This recheck does not change the release conclusion: MY/SIDARMA canonical evidence, the global source-science gate, and user-deferred remote-storage acceptance remain open.

## Separate Rust-native migration note — 2026-09-27

This note tracks `specs/003-rust-core-performance`; it does not replace or advance the Python migration/release decision above. The native registry currently contains 23 adapters for 24 catalog source IDs / 26 targets. TMD binds frame time to optional Tesseract footer OCR, RoyalRain keeps filename-timed raw CAPPI images, and TW retains its image-plus-metadata observation and provider-native numeric grid JSON. RainViewer and TW grid have verified Rust decoders; other paths do not close their remaining scientific validation gates. `ph` remains without an adapter; Windy supports latest HTTP discovery and four-tile raw acquisition but has no browser/CDP path or verified science. ID and BMKG support raw-only acquisition; OpenSnow fails closed, UK is retired, and Weather Underground has credential-gated latest locator behavior only. The native CLI commits raw-only outputs, PNG+sidecar, NetCDF4, GeoTIFF, and Zarr v2 for RainViewer composite/TW grid through a local v1 manifest-last store.

The Rust cache root `~/.cache/radiust-rust` must remain separate from Python `~/.cache/radiust`: the index schemas differ, Rust rejects a Python `entries` database and unindexed objects, and cache cross-version repair remains open. Rust reads a Python-generated v1 manifest fixture and locally publishes output manifests last, with skip/repair/rollback tests. The native Zarr v2 writer passes independent xarray readback for values, quality, time, CRS, chunks, Blosc/LZ4, and consolidated metadata; decoded Zarr downloads now commit nested store files through the local manifest-last path and skip verified repeats. The pure-Rust GeoTIFF writer passes independent Rasterio readback for EPSG:3857 and EPSG:3821 CRS, affine, orientation, nodata, quality, and provenance. Independent readback of committed provider output and remote S3/OSS generation/pointer commit remain unverified. Rust `write_stream`/`read_to_path` storage methods are not connected to a remote commit fence. NetCDF/HDF5 are now statically built: macOS 11.0 and 15.0 arm64 wheels passed local install/Mach-O audits without Homebrew references, though the GitHub wheel workflow and actual macOS 11 host remain untested. Keep cache roots separate and do not infer provider acceptance from local tests.

Rust `ProcessingSpec` includes decoder, source-resource, and encoder versions in `processing_hash`; increment the relevant version whenever the corresponding semantic rules change. Local raw, PNG, NetCDF, GeoTIFF, and Zarr output commits use this identity contract. AWS S3, S3-compatible, and OSS provider acceptance remains unverified and is not inferred from local or in-memory tests. The Rust workspace regression passed 341 tests with one ignored manual benchmark; the Python suite passed 660 tests with 25 live/provider opt-in skips. Four offline CLI benchmark scenarios each had 30 paired samples and matching semantic fingerprints on release binary SHA-256 `b49cc56fb834311640a35f06bbdff891af14a823efb42e0e18c976d936ee35eb`; list p95 was 0.6340/0.0192 s and discover-all p95 was 1.1497/0.0187 s for Python/Rust respectively. Release readiness remains partial because source science, providers, packaging, full CLI parity, and the complete performance matrix are open.

## Current worktree recheck — 2026-09-30

- `uv run --no-sync python scripts/validation/audit_migration.py` reports `missing=[]`, 148 of 154 tasks checked, and six open legacy v1 tasks: T011/T038 (MY reuse, valid time, and geometry), T053/T065 (source science evidence), and T084/T085 (real S3/S3-compatible/OSS acceptance and dependent round-trip gate). Legacy display coverage is 15 of 23 paths passed, zero pending differences, and eight evidence-blocked; `ready_for_v1=false`.
- The separate Rust-native feature recheck is recorded in [`rust-migration-acceptance.md`](../validation-results/rust-migration-acceptance.md). Its current arm64 wheels pass local install, CLI/Mach-O/resource checks, and four-format readback, while the GitHub source-build and actual macOS 11 host remain unverified. This does not close the legacy v1 source-science or remote-provider gates above.
