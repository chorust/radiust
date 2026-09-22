# Foundation validation

Date: 2026-09-20

## Automated checks

- `uv run pytest -q`: **286 passed, 16 skipped, 1 warning**. Skips are 12 opt-in live cases, three unconfigured remote storage-provider cases, and the clean-wheel artifact supplied by packaging CI.
- `cargo test --workspace`: **24 Rust integration tests passed** (cache, FTP, object storage, runtime, and tiles); unit and doc-test targets passed with zero tests.
- The source-tree extension was rebuilt with `env -u CONDA_PREFIX uv run maturin develop --skip-install --features extension-module`.
- `tests/contract/test_bridge.py`: **4 passed**, including managed-path rejection, legacy `path_is_within` false semantics, public error mapping, and Python cancellation of a Rust `future_into_py` awaitable.
- The local OpenDAL object-storage contract passed with **2 Rust tests**, and Python URI/provider/anonymous-read contracts passed with **5 tests** across the object and provider matrix modules.
- The offline fixture NetCDF passed independent `netCDF4 1.7.4` readback and CF Checker 4.1.0 against CF-1.8: **0 errors, 0 warnings, 0 information messages**. This validates file conformance only, not the legacy fixture's source science or geometry.

The remaining warning is the existing NumPy binary-compatibility warning emitted while exercising the output-variable regression test. The latest explicit public live run passed 9 cases (eight raw acquisitions and the Taiwan numeric-grid decode); provider credentials remain unverified. The explicit local Chromium replay also passed after the lifecycle fix; it does not provide PAGASA provider evidence.

## M0 status

The shared model, identity, runtime, temporary-owner, cache, transport-policy, and bridge checks pass. T011 remains open because `tests/fixtures/sources/my/` still contains legacy display PNGs rather than a legally traceable canonical upstream raw response with verified native geometry and time binding. The blocker is recorded in [`migration/blockers/my.md`](../migration/blockers/my.md); those files are not being promoted to M0 evidence.

The repository constitution is still the placeholder empty template at [`.specify/memory/constitution.md`](../.specify/memory/constitution.md), so no additional approved principles were applied.

**Gate result: blocked on T011/M0 evidence; automated foundation checks are green.**

## Local adapter-continuation recheck — 2026-09-21

- `uv run pytest -q tests/contract/test_identity.py tests/contract/test_scientific_model.py tests/contract/test_bridge.py tests/contract/test_network_isolation.py tests/contract/test_raw_replay.py tests/contract/test_transport.py`: **22 passed**.
- `cargo test --workspace --locked`: **24 Rust integration tests passed**; all unit and doc-test targets passed.
- The blank constitution template and T011 blocker were rechecked. These results do not replace the missing licensed MY source raw, valid-time binding, or native geometry.

**Current gate result: automated foundation checks pass; M0 remains blocked on T011.**

## Current task-order recheck — 2026-09-21

- `uv run pytest -q tests/contract/test_identity.py tests/contract/test_scientific_model.py tests/integration/test_review_regressions.py tests/integration/test_batch_cancel.py`: **19 passed**, with one existing NumPy ABI warning.
- `cargo test -p radiust-core --test runtime_lifecycle --locked`: **5 passed**; the latest full workspace run passed all **24 integration tests**.
- The default offline suite passed **315 tests with 25 explicit skips**. These checks leave the M0 source gate unchanged: MY still lacks an authorized canonical raw response, provider-bound observation time, and native geometry, so T011 and dependent T028 remain open.

## Current host and local Actions recheck — 2026-09-21

- Host `uv run pytest -q` passed **316 tests with 25 explicit skips** and one existing NumPy ABI warning. Ruff, `cargo fmt --all -- --check`, and `cargo test --workspace --locked` also passed; Rust reported 24 integration tests.
- Executed `.github/workflows/offline.yml`'s `offline` job using official `act` v0.2.89, the local Ubuntu 24.04 arm64 runner image, and CPython 3.12.14. The job passed locked dependency sync, Ruff, the Rust workspace (24 integration tests), and pytest (**316 passed, 25 skipped**).
- This is one local Linux/arm64/Python 3.12 combination. It does not resolve M0: T011 and T028 remain open because no authorized canonical MY raw with provider-bound time and verified native geometry is available.

## CI and adapter replay continuation — 2026-09-22

- The current `.github/workflows/offline.yml` `offline` job completed under SHA-verified official `act` v0.2.89 on the local Ubuntu 24.04 arm64 runner with CPython 3.12.14. Locked sync using the lean `ci` dependency group, Ruff, `cargo test --workspace --locked` (**24 Rust integration tests**), and pytest (**325 passed, 25 skipped**) succeeded.
- In a clean CI-group environment, the terminal/rendering/CLI subset passed **24 tests**. Targeted `id`, BMKG, OpenSnow, and Weather Underground synthetic acquisition/redaction tests passed **10 tests**; they confirm byte/hash handling and fail-closed decoding, not canonical provider data.
- The T028 shared-contract execution/report is complete: its scoped Python contract command passed **22 tests**, and the offline Actions run passed the full Python suite plus **24 Rust integration tests**. M0 is still blocked independently on T011: MY has no authorized canonical raw with independently bound source time and native geometry. The repository constitution is still an empty template, so no source-story gate is released.
