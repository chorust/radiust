# US3 partial validation

Date: 2026-09-20

- Local output commit tests pass, including manifest completeness, repair, raw modes, root behavior, overwrite generation history, and conflict handling.
- The OpenDAL memory backend contract passes **2 Rust tests** for streaming read/write, SHA-256 receipts, safe keys, and size limits.
- The Python object/provider contracts pass **9 tests** across anonymous HTTP reads, URI normalization, provider redaction, and remote generation commit fault injection.
- The remote commit tests cover manifest-last publication, unique generation, supersedes history, cancellation before the pointer fence, and response-loss reconciliation/unknown outcome.
- The download pipeline now accepts `s3://` and `oss://` targets, stages locally, uploads immutable generations through the Rust OpenDAL bridge, verifies read-backs, and publishes the logical manifest pointer last. A loopback S3-compatible smoke produced `written` followed by `skipped` under an isolated bucket prefix.
- The current CPython 3.13 macOS arm64 wheel was installed outside the source tree; its CLI `download my --output s3://... --json` returned exit 0 with `written=1`, then `skipped=1`, and the wheel exposed the Rust object writer.

AWS S3, S3-compatible, and Aliyun OSS real-provider execution is recorded as `not_run` in [`storage-providers.json`](storage-providers.json); no production prefix was touched.

**Gate result: incomplete; real provider evidence and full raw/format failure matrix remain open.**

## Recheck on 2026-09-20

- The focused offline storage suite passed **25 tests**: `uv run pytest -q tests/contract/test_output_formats.py tests/contract/test_encoding.py tests/contract/test_raw_replay.py tests/integration/test_output_commit.py tests/integration/test_raw_modes.py tests/integration/test_remote_commit.py tests/integration/test_object_storage.py tests/integration/test_storage_providers.py`.
- Added the missing `tests/contract/test_raw_replay.py`: six cases now cover verified local replay, raw-only decode from retained files with public networking disabled, receipt hash/size mismatch, incomplete/empty manifests, traversal/symlink escapes, and invalid manifest inputs.
- The focused run exercises NetCDF, PNG, GeoTIFF, and Zarr output checks, raw-only and raw supplementation, local output repair/overwrite/conflict, remote cancellation and lost-pointer-response reconciliation, and anonymous object reads. The provider tests in this run validate configuration/redaction only; they are not real AWS/S3-compatible/OSS writes.
- `validation-results/storage-providers.json` still records all three remote providers as `not_run`; no isolated test URI and credentials were configured. The cross-provider gate and T069/T084/T085 acceptance remain open.

**Recheck gate: offline evidence passed; US3 and T085 remain incomplete pending isolated real-provider evidence and the remaining storage acceptance tasks.**

## Commit and raw-mode fault coverage — 2026-09-20

- The focused commit/raw-mode suite passed **19 tests** across `test_output_commit.py`, `test_remote_commit.py`, and `test_raw_modes.py`; Ruff passed for all three files. T066 and T067 are now checked in `tasks.md`.
- Added local root-lock contention, short-variant-hash collision against full output identity, manifest corruption detection and repair, plus same-revision raw supplementation assertions.
- Remote fault injection now covers artifact-stage failure, corrupted read-back, cancellation before the publish fence, generation-manifest response loss, and final-pointer response loss both with and without visible commit. Explicit overwrite retains the previous immutable generation's original bytes and links it through `supersedes`.
- Raw-mode coverage verifies a mosaic-only cache cannot satisfy a raw-tile request, a changed source revision reacquires matching tiles, raw-only avoids decode/processing, and cache clear leaves formally committed raw files readable.
- This closes the offline T066/T067 contract cases. The T069 harness is implemented; T084 real-provider runs and T085's complete storage acceptance remain open. No mock result is counted as provider validation.
- The T069 provider harness now includes local round-trip plus opt-in AWS/S3-compatible/OSS read-back tests, redacted configuration reporting, and a required dedicated test path with a unique per-run child. Its targeted run passed **4 tests** and skipped the **3 remote-provider cases** as unverified; the three provider test URI environment variables are unset, so no remote writes were attempted.

## Latest implementation verification — 2026-09-20

- The focused commit/raw/output-format/regression run passed **34 tests**. It includes local marker withdrawal, cancellation fencing and lost-response reconciliation; remote staging/read-back/pointer failure injection; raw supplementation, mosaic-only cache misses, raw-only decode bypass, and retained committed raw after cache clearing.
- The full offline suite recheck now passes **286 tests, with 16 explicit skips and 1 warning**. Three isolated remote-provider targets remain `not_run`; they are not counted as provider passes.
- `uv run ruff check python tests`, `cargo fmt --all -- --check`, and `cargo test --workspace` passed; the Rust workspace has **24 integration tests**.
- T066/T067 offline coverage and T069 harness are complete. T084 real-provider execution and T085 full storage acceptance remain open.
