# US1 validation

Date: 2026-09-18

## Offline contract and local fault checks

- The US1 query/SDK/registry/encoding/offline CLI/manifest/output/raw-mode set ran with `uv run pytest -q ...`: **27 passed**.
- The full source-tree suite is recorded in [`foundation.md`](foundation.md); see its dated recheck below for the current count.
- The checked-in legacy `my` fixture can complete the offline list/discover/download path and supports sync/async tests.

## Independent NetCDF read

The current local output was read with the independent `netCDF4` package during the targeted validation run; no SDK reader was used for that read-back.

## Remaining gate failures

- `cfchecks --version` cannot start because the environment has no UDUNITS-2 shared library (`cfunits requires UNIDATA UDUNITS-2`). Install the platform library before claiming CF checker evidence.
- T038 is still blocked by T011: the checked-in `my` files are legacy display PNGs and do not establish canonical upstream raw bytes, native geometry, or time binding. The offline path therefore cannot be promoted to the real-source US1 acceptance gate.

**Gate result: blocked; the offline SDK/CLI contracts pass, but real-source and CF-checker evidence are incomplete.**

## Recheck on 2026-09-20

- The current offline fixture output was read independently with `netCDF4 1.7.4`: reflectivity shape `(640, 826)`, `dBZ`, quality `uint16`, and `Conventions=CF-1.8`.
- CF Checker **4.1.0** checked the file against CF-1.8 using Standard Name Table 94 and reported **0 errors, 0 warnings, 0 information messages**. On this macOS environment, the installed UDUNITS library becomes visible to cfunits with `DYLD_LIBRARY_PATH=/opt/homebrew/opt/udunits/lib`.
- `uv run pytest -q`: **238 passed, 13 skipped, 1 warning**; the live/provider and clean-wheel skips remain explicit and are not counted as source acceptance.

## Recheck on 2026-09-20 — full suite

- `uv run pytest -q`: **286 passed, 16 skipped, 1 existing NumPy binary-compatibility warning**. This verifies the offline implementation and regression suite only.
- T049 remains blocked by T011: the MY fixture still lacks legally traceable canonical raw bytes, source valid-time binding, and native geometry. A green offline suite and independent NetCDF/CF readback do not satisfy the real-source gate.

CF checker execution is now verified for this offline fixture. T049 remains open because this legacy-image fixture does not satisfy T011's canonical upstream raw, valid-time, or native-geometry evidence.

## Recheck on 2026-09-21

- The full default suite passed **304 tests, 22 expected skips, and one existing NumPy binary-compatibility warning**. Sync/async SDK, query-time rejection, CLI output, local commit, and independent NetCDF/CF readback contracts remain green.
- T049 remains open: the passing offline `FixtureSource` path does not replace the missing authorized MY upstream GIF/raw, source-time evidence, or native geometry required by T011/T038. No data from the user's previous temporary CLI output was treated as canonical raw evidence.

## Current local recheck — 2026-09-21

- `uv run pytest -q`: **315 passed, 25 explicit skips**, with one existing NumPy ABI warning. The offline SDK/CLI, sync/async, query, commit, and regression contracts remain green.
- `uv run pytest -q tests/terminal tests/contract/test_rendering.py tests/contract/test_cli_acquisition.py tests/contract/test_cli_output.py tests/contract/test_cli_cat.py`: **24 passed**. This covers local preview/CLI behavior, not source-backed MY validation.
- The fixture NetCDF's independent `netCDF4 1.7.4` and CF Checker 4.1.0 checks remain valid for file conformance. T049 stays open because T011/T038 do not yet have authorized canonical MY raw, independently bound valid time, or native geometry.

## Current local Actions recheck — 2026-09-21

- The host suite passed **316 tests with 25 explicit skips**. The workflow's `offline` job also passed locally under `act` v0.2.89 on Ubuntu 24.04 arm64 / CPython 3.12.14, including locked sync, Ruff, 24 Rust integration tests, and all 316 pytest cases.
- The CI run is one local matrix combination. T049 remains open because the legacy MY display fixture is not an authorized canonical raw sample and does not establish source-bound valid time or native geometry.

## Read/acquisition closure recheck — 2026-09-22

- The user supplied a real CLI run from the registered `my` adapter: the first `download my --latest` wrote one frame and skipped the already-present frame; repeating skipped both. A clean output root wrote both frames. `radiust cat --file ... --renderer text` read the resulting NetCDF and reported the `east` frame as `(650, 1113)`, `dBZ`, with UTC valid time `2025-12-29T06:52:01Z`. The separate recorded raw-only probe also returned exit 0 and wrote two live frames; raw payloads were hashed and deleted after validation because reuse permission was unavailable.
- Offline US1 contract/fault coverage, sync/async SDK paths, time-query rejection, and local atomic-output behavior pass in the current suite. `netCDF4 1.7.4` independently read the representative NetCDF fixture, and CF Checker 4.1.0 reported **0 errors, 0 warnings, 0 information messages** for CF-1.8.
- Current `uv run pytest -q`: **325 passed, 25 skipped**, one existing NumPy ABI warning. The read/acquisition and file-readback verification requested by T049 is complete. This does not make the temporary MY payload a distributable fixture or validate its physical palette/native grid; T011/T038 remain open.

**T049 complete for the read/acquisition validation scope; MY raw licensing and scientific acceptance remain separate blockers.**
