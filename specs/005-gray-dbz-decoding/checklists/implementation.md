# 005 Implementation Acceptance Index

**Reviewed**: 2026-10-06
**Scope**: offline implementation contract in `spec.md`, `plan.md`, and `tasks.md`. `[x]` means the requirement has implementation evidence within this scope; it does not claim independent physical calibration, live-source availability, or unknown geometry.

## Functional requirements

| Requirement | Acceptance result | Tasks and evidence |
| --- | --- | --- |
| FR-001 raw / gray / dbz terms and dBZ units | [x] Distinct modes and units appear in CLI, SDK, result metadata, and docs. | T011, T020, T022–T038, T057–T058; `docs/gray-dbz.md`, `validation-results/gray-dbz-local.json` |
| FR-002 0–224 encoding formula | [x] All 225 codes follow `gray × 5/16`; maximum measured error is 0.000001 dBZ. | T010, T013, T039, T049; `crates/radiust-core/tests/gray_dbz_local.rs`, `validation-results/gray-dbz.json` |
| FR-003 numeric values, quality, pixel grid, CLI/SDK and saving | [x] Local and source RasterResults retain quality and row/column data; applicable persistence is covered. | T014–T021, T045–T061, T062–T079; `gray-dbz-local.json`, `gray-dbz-formats.json`, `gray-dbz-sdk.json` |
| FR-004 strict validation and source-only clipping | [x] Local out-of-range and non-gray inputs reject; matched-source values clip only for dBZ while gray remains unchanged and adjustment is recorded. | T013, T032, T045–T049, T060; `gray-dbz-local.json`, `gray-dbz-paths.json`, `gray-dbz-quality.json` |
| FR-005 alpha, missing pixels, and valid black | [x] Alpha zero is missing; nonzero alpha does not scale values; valid opaque black decodes to zero. | T010, T014, T039–T041, T062–T063; `gray-dbz-local.json`, `gray-dbz-formats.json` |
| FR-006 quality and origin distinctions | [x] Missing, outside coverage, unknown color, recovered, and interpolated states remain distinct through conversion and resize. | T039–T050, T059; `crates/radiust-core/tests/gray_quality.rs`, `gray-resize-quality.rs`, `validation-results/gray-dbz-quality.json` |
| FR-007 quantization, clipping, and information limits | [x] Processing and unknown time/geometry limits are exposed in result metadata and user documentation. | T049, T057, T083; `docs/gray-dbz.md`, `gray-dbz-paths.json` |
| FR-008 canonical CLI modes and mutual exclusion | [x] `--gray` / `--dbz`, raw behavior, local/source dispatch, and conflicting modes are covered. | T011, T020, T028–T031, T058, T065, T075, T078; `crates/radiust-cli/tests/gray_dbz_modes.rs`, `gray_dbz_local.rs` |
| FR-009 compatibility entry points and actual units | [x] Compatibility aliases remain explicit; generic science and non-dBZ units retain their existing meaning. | T022–T038, T051, T084; `gray-dbz-compat.json`, `crates/radiust-core/tests/gray_compat.rs` |
| FR-010 active naming consistency | [x] Activity docs and interfaces use gray/dbz; legacy display names remain only as marked compatibility APIs or historical references. | T036, T084; README, `docs/cli.md`, `docs/python-sdk.md`, `docs/migration.md`; naming audit recorded in `gray-dbz.json` |
| FR-011 output modes and report compatibility | [x] Reports retain existing envelope fields and add versioned mode metadata. | T020, T030–T038, T057–T058; `crates/radiust-cli/src/report.rs`, `tests/contract/test_gray_dbz_reports.py` |
| FR-012 evidence-gated source conversion | [x] Only matched passed source rules convert; blocked and unknown paths do not fall back to raw success. | T041–T045, T052, T058–T061; `gray-dbz-paths.json`, `gray-dbz-sdk.json` |
| FR-013 all 15 passed source paths | [x] 15/15 retained gray baselines match pixel-for-pixel and have dBZ arithmetic/quality evidence; NZ clipping is recorded. | T001, T060–T061; `validation-results/gray-dbz-paths.json`, `gray-dbz-compat.json` |
| FR-014 eight blocked paths and independent native routes | [x] All 8 remain blocked with reasons; RainViewer, TW grid, and RDCAP native dBZ tests remain separately enabled. | T001, T042, T051, T060–T061; `gray-dbz-paths.json`, `crates/radiust-core/tests/dbz_native.rs` |
| FR-015 native reflectivity and other physical units | [x] Native dBZ values preserve their own numeric path; non-reflectivity science is not relabeled dBZ. | T042, T051–T052; `crates/radiust-core/tests/dbz_native.rs`, `gray-dbz-sdk.json` |
| FR-016 explicit local encoding declaration | [x] Local decoding is opt-in, records input identity/declaration, and rejects a conflicting FrameRef. | T010–T021, T064, T075; `crates/radiust-core/tests/gray_dbz_local.rs`, `raster_identity_commit.rs` |
| FR-017 dimensions, orientation, and evidence-backed geography | [x] Pixel dimensions/orientation persist; unknown time/geography stay unknown; GeoTIFF requires complete trusted geometry. | T014, T019, T062, T067–T071; `pixel_dbz_formats.rs`, `gray-dbz-formats.json`, `gray-dbz-paths.json` |
| FR-018 numeric persistence and independent readback | [x] NetCDF/Zarr pixel outputs independently read back; GeoTIFF is accepted with trusted geometry and rejected without it. | T062–T063, T067–T071, T079; `tests/contract/test_gray_dbz_formats.py`, `tests/integration/test_gray_dbz_persistence.py`, `gray-dbz-formats.json` |
| FR-019 identity, provenance, and processing distinction | [x] Local/source/numeric identities, file receipts, component digests, upstream provenance, and transaction behavior are covered. | T006, T064, T072–T080; `raster_identity_commit.rs`, `gray-dbz-commit.json` |
| FR-020 historical bytes, identities, and gray pixels | [x] SHA/size audit: 155 protected artifacts checked, zero missing or mismatched; retained gray baselines have zero pixel differences. | T001, T024, T027, T064, T080, T084, T086; `gray-dbz-compat.json`, `gray-dbz-paths.json` |
| FR-021 CLI and sync/async SDK consistency and partial results | [x] Representative offline source/local matrices verify values, modes, units, partial outcomes, ordering, and failure status. | T043, T053–T058, T061, T065–T066, T074–T078; `tests/integration/test_gray_dbz_sdk.py`, `gray-dbz-sdk.json` |
| FR-022 limits, deadlines, cancellation, and atomic output | [x] Resource limits and cancellation are exercised; interrupted commits restore prior output and do not publish incomplete manifests. | T008–T009, T044, T050, T064, T066, T072–T080; `gray-dbz-quality.json`, `gray-dbz-commit.json` |
| FR-023 user and migration documentation | [x] Source/local examples, formats, geometry boundaries, quantization, alpha, and compatibility migration are documented. | T036–T037, T081–T083; README, `docs/cli.md`, `docs/python-sdk.md`, `docs/migration.md`, `docs/gray-dbz.md` |
| FR-024 separate validation dimensions | [x] Reports distinguish gray matching, encoding dBZ, physical/geographic evidence, and live acquisition. | T060–T061, T085–T086; `gray-dbz.json`, `gray-dbz-paths.json`, `docs/gray-dbz.md` |

## Success criteria

| Criterion | Acceptance result | Evidence |
| --- | --- | --- |
| SC-001 225-code formula and tolerance | [x] 225 codes verified; maximum absolute error 0.000001 dBZ. | `gray-dbz.json`; local Core and Python contracts |
| SC-002 invalid, transparent, black, and corrupt cases | [x] Representative cases enforce missing/valid/invalid distinctions and reject invalid inputs. | `gray-dbz-local.json`, `gray-dbz-quality.json`, Core and CLI local tests |
| SC-003 15 passed / 8 blocked source paths | [x] 15 passed with zero gray pixel differences and zero formula mismatches; 8 stay blocked. | `gray-dbz-paths.json`, `gray-dbz-compat.json` |
| SC-004 CLI, sync/async, and native dBZ consistency | [x] Offline CLI/SDK and direct-native contracts pass; representative sync/async partial-result parity is covered. | `gray-dbz-sdk.json`, `tests/integration/test_gray_dbz_sdk.py`, `dbz_native.rs` |
| SC-005 names, help, reports, and compatibility | [x] Naming audit plus compatibility contract and installed-wheel checks are recorded. | T084, `gray-dbz-compat.json`, `gray-dbz.json` |
| SC-006 located and pixel-only formats | [x] NetCDF/Zarr round-trip pixel results; GeoTIFF requires trusted geometry; absent geography remains absent. | `gray-dbz-formats.json`, `pixel_dbz_formats.rs`, Python independent format contracts |
| SC-007 replay, identity, partial, limits, and cancellation | [x] Identity/commit matrix, replay integrity, damaged-output repair, skip, and cancellation evidence recorded. | `gray-dbz-commit.json`, `raster_identity_commit.rs`, `replay_support.rs` |
| SC-008 traceable decisions and usable documentation | [x] Path decisions, limitations, input/processing identity, and runnable CLI/SDK flows are documented. | `gray-dbz-paths.json`, `gray-dbz.json`, README and `docs/gray-dbz.md` |

## Protected artifacts and final gate

- Protected-artifact audit: 155 listed files; 0 missing; 0 size mismatches; 0 SHA-256 mismatches. The retained gray path comparison reports 15 passed, 8 blocked, and 0 difference-pending.
- The latest path runner exits 1 intentionally while the 8 evidence-blocked entries remain. This is the expected gate behavior; the blocked entries are not counted as dBZ successes or implementation regressions.
- Offline implementation acceptance is complete for the specified evidence scope. Live provider acquisition, independent physical calibration, unknown geographic mappings, and S3/OSS were not tested and remain outside this acceptance claim.
- README and active CLI, migration, SDK, output, source-development, and gray/dBZ documentation were reviewed against T036 and T081–T083. Historical specs, plans, tasks, and roadmap statuses were not used to infer closure.
