# CLI experience implementation log

## Source raw preview hard cancellation (2026-09-23, T009/T014/T015/T049)

`cat SOURCE --raw` now runs discovery, frame selection, acquisition and raw
preview in a spawned worker process. The worker is isolated in its own POSIX
process group; the parent owns one per-operation temporary root, keeps
`cache.enabled=false`, passes only the requested source configuration, strips
storage credentials/endpoints, and never gives the worker formal output
credentials. A bounded IPC message carries progress and preview metadata; the
PNG pixels are staged under the parent-owned temporary root. The parent checks
the image size/hash receipt and resource limits, reconstructs `RawPreview`,
then deletes only the operation root. The existing configured temp root and
caller-owned files remain intact.

The lifecycle tests now exercise malformed and over-limit images, multi-file
frames, frame identity changes, acquisition/display interruption, raw hash and
ownership, and preservation of cache/output sentinels. A real subprocess test
invokes the Click `cat --raw` path, blocks a worker that ignores SIGTERM, then
sends SIGINT to the CLI parent. It exits 130 in under five seconds, escalates
to kill the worker, removes its owned temp root, reaps the worker PID and
preserves existing cache/output files. The worker contract test verifies
scientific `fetch`/`decode` are never called and that only the selected
source's configuration reaches the child. PTY termios restoration also passed.
The blocked worker is an injected non-cooperative operation; no live provider
or network request was used.

Validation on this checkpoint:

- `uv run pytest tests/contract/test_cli_raw_preview.py tests/integration/test_raw_preview_lifecycle.py tests/contract/test_legacy_display.py tests/contract/test_legacy_gray_source.py -q`: **40 passed**.
- `uv run pytest tests/contract/test_cli_raw_preview.py tests/integration/test_raw_preview_lifecycle.py tests/contract/test_cli_cat.py tests/contract/test_raw_replay.py tests/terminal/test_renderers.py tests/integration/test_raw_modes.py -q`: **54 passed**.
- `uv run pytest tests/terminal/test_cli_pty.py -q`: **9 passed**.
- `uv run ruff check python tests scripts/validation` and `git diff --check`: passed.

T009, T014, T015 and T049 are checked on this evidence. T049 verifies the
legacy-display mechanism and synthetic verified tile path; no provider legacy
rule was enabled and US5 source migration remains incomplete. This checkpoint
does not close SC-004 for aggregate discovery or T052/T060, and does not close
the global request/host/source quota matrix in T053.

The complete offline regression command
`uv run pytest tests/contract tests/integration tests/terminal tests/sources/test_tile_adapter_replay.py -q`
then exited 0 on 2026-09-23: **458 passed, 3 credentialed-provider tests
skipped, 1 existing NumPy ABI warning** in 36.40 seconds. The first attempt
after introducing the spawn boundary had three failures because older cat
tests monkeypatched `Client.discover` in the parent process; those tests now
inject the isolated raw-preview result while retaining default/latest query
and revision-collision assertions. No source behavior was weakened to obtain
the passing rerun. The remaining full quickstart commands and SC-006 human
review are tracked separately; T066 remains unchecked.

## Legacy display audit recheck (2026-09-23, T050)

`compare_legacy_display.py` exited 1 as expected and wrote a complete report:
**25 blocked, 0 passed, 0 difference_pending**. The structural audit exited 0
with **24 source records, no missing records, no structural errors**, and the
display ledger still reports `coverage_closed=false` and `ready=false`.
`tests/contract/test_migration_audit.py` and
`tests/contract/test_migration_inventory.py` passed **14 tests**. The updated
US2/US6 records distinguish display evidence from existing scientific status
and retain null rule/input/output/baseline hashes and crop values where no
legal source-matched evidence exists. T050 is checked for the audit and
reporting work; source rule migrations T037–T045 remain open.

## Terminal report acceptance (2026-09-23, T028 automated portion)

`uv run pytest tests/contract/test_cli_output.py tests/contract/test_cli_reports.py tests/terminal/test_capabilities.py tests/terminal/test_cli_pty.py tests/terminal/test_cli_progress.py -q` passed **32 tests**. The PTY/renderer automated matrix includes 40/80/120-column output, CJK and combining-width handling, NO_COLOR and pipe safety, stderr progress cleanup, JSON/quiet conflicts, and SIGINT termios restoration. Direct `NO_COLOR=1 uv run radiust list sources` exited 0 and the CLI emitted 24 source rows; quiet JSON and ordinary `list sources --json` also emitted schema-v1 JSON.

The required human timing review remains pending: no real reviewer has timed
success, no_data and partial_failure reports at 30 seconds each. T028 remains
unchecked until those three reviews are recorded; automated layout tests do
not count as human acceptance.

## Aggregate discovery IPC and plugin-load boundary (2026-09-22, T052/T056/T058 partial)

An untrusted source worker can supply the IPC length header and only one byte
of the body, causing `Connection.poll()` to report readable while a direct
`recv_bytes()` blocks indefinitely. Both catalog and target IPC now bound the
*full read* by the same monotonic batch deadline, then terminate/reap the
isolated worker in the caller's existing cleanup path. Late success cannot
override an expired deadline. Three injected cases verify partial catalog
messages, partial target messages with owned-temp cleanup and a late result;
the partial-message cases also verify that no new child remains active.

`SourceRegistry` now defers entry-point enumeration/loading until `infos()`,
`get_info()` or `get()` rather than running third-party plugin code during
global-registry/CLI module import. This lets `discover all` enter the
deadline-limited catalog worker before the default registry executes a plugin;
its per-source worker also loads adapters inside the process boundary.
The new registry contract first failed against the eager loader, then passed
after the change. The focused registry/catalog selection passed **11 tests**,
the discover deadline/limits/CLI selection passed **42 tests** before the
registry change, and Ruff passed. This strengthens the blocked plugin/IPC
cases; it does not finish per-request/host/source quota enforcement, full
browser descendant ownership or T052/T056/T058 acceptance.

The post-change *combined* test invocation was run but did not pass in this
execution environment: **447 passed, 3 skipped, 3 failed, 2 errors**. Three
local HTTP tests failed or errored on `PermissionError: [Errno 1] Operation not
permitted` binding loopback sockets; the descendant cleanup test could not
execute its `ps` probe for the same OS permission reason. These are explicit
environmental verification gaps, not passing tests or code correctness proof.
The remaining affected registry/discovery selection, excluding only that
OS-blocked `ps` test, passed **51 tests, 1 deselected**. Ruff and `git diff
--check` passed. Re-run the full suite and the real descendant check on a host
with loopback bind and process inspection permitted before closing T066.

A second post-change combined run excluding exactly the five environment-
denied cases passed **447 tests, 3 provider skips, 5 deselected** in 26.98 s;
Ruff and `git diff --check` both passed. This is a verified partial regression,
not a replacement for the required unrestricted full-suite acceptance.

## Task 002 offline acceptance checkpoint (2026-09-22, T066 in progress)

Latest post-fix regression was split into the same non-overlapping test paths
because the session-output read for the combined command was unavailable:
`uv run pytest -q tests/contract tests/terminal tests/sources/test_tile_adapter_replay.py`
exited 0 with **290 passed** (8.37 s); `uv run pytest -q tests/integration`
exited 0 with **158 passed, 3 explicitly skipped provider tests, 1 NumPy ABI
warning** (23.38 s). In total the requested four paths contain **448 passed,
3 skipped**, but this is two verified invocations, not a claimed completed
single-command run. Ruff and `git diff --check` passed after the test edits.
An earlier combined run had **1 failure** in the descendant-cleanup test:
the test's 0.8-second spawn budget sometimes elapsed before the child started;
simply raising that budget exposed a separate 2-second marker-sleep assertion
that could not detect a running descendant. The test now gives the spawned
worker a 3-second budget, gives the synthetic descendant a 60-second lifetime,
and verifies the actual child PID is gone or a zombie after cleanup instead
of inferring termination from a 2-second marker. Its isolated rerun passed;
the post-fix split integration invocation above also passed.

Direct quickstart recheck: `radiust cat --file ...kkn240Loop.gif --renderer
text` exited 0 and reported `GIF`, `680x680`, first frame index 0,
`display=original`; the corresponding `...takhli.png` command exited 0
with `PNG`, `1020x800`, `display=original`. Offline
`radiust discover all --json` produced parseable single-object JSON with 26
items (21 network_restricted, 4 missing_credentials, 1 retired), each in
exactly one status. These are local smoke checks, not online acquisition or
historical gray-rule evidence.

At this checkpoint T049's raw/legacy/gray/tile/lifecycle-focused selection
passed **55 tests**, but acquisition and renderer cancellation were not yet
verified; see the later raw worker evidence below. T050's migration/audit/legacy-evidence
selection passed **34 tests**; the audit found all **25** known provisional
paths consistently blocked across the manifest, source records, output and
packaged index with zero structural errors. Coverage remains unclosed and
ready=false; the us2/us6 documents enumerate missing historical rule version,
valid raw/output fingerprints, old baseline, crop and reviewed differences
instead of manufacturing them. The *audit wiring* is implemented, but T050
remains unchecked pending its T049 integration prerequisite; neither the audit
nor synthetic fixtures complete source migration or SC-008–010.

T052 gained two additional real-process tests: an externally delivered POSIX
SIGINT to the discovery parent terminates the blocked worker, returns a single
interrupted report with exit code 130 and deletes the owned temp root within
five seconds; a shared-deadline run preserves a completed target as `success`,
marks the blocked active target `timeout` and leaves its queued successor
`not_started`. `tests/integration/test_discovery_deadline.py`,
`tests/integration/test_discovery_limits.py` and
`tests/contract/test_cli_discover_all.py` passed **40 tests** in 9.80 seconds.
The full browser/plugin/backpressure/descendant and quota matrix remains
unverified, so T052/T053/T060/T062 are still unchecked.

T029's synthetic mechanism coverage now includes a positive/negative rule
identity-and-version check and an end-to-end raw preview case: an unknown rule
returns the original detached image; a matching, separately evidenced rule
whose input contains an unrecognized opaque RGB pixel raises a display error
without falling back or modifying raw bytes. The updated
`tests/contract/test_legacy_display.py` passed **8 tests**; together with
`test_cli_raw_preview.py`, the focused run passed **25 tests**. These test-only
rules are not provider evidence.

T030/T031 synthetic evidence and tile replay tests passed **41 tests** in
3.93 seconds. They cover manifest path/hash/license rejection, blocked/pending
evidence, rule-fingerprint invalidation, provenance-preserving tile ordering,
alpha, missing/duplicate/wrong-sized/wrong-revision tiles and fail-closed
unknown combinations. Their completion records test coverage only; no source
rule or legal old baseline has been supplied or accepted.

T034–T036 are complete as *mechanism implementation*: the provenance- and
evidence-gated engine, ordered product-step overrides and bounded tile
composition are exercised by those contract/tile tests; the offline comparator
verifies bytes and SHA-256, computes pixel/alpha/background/missing differences,
rejects escaping fixture paths and retains blocked/difference_pending results.
The actual 25-path manifest produced `0 passed / 25 blocked` and comparator
exit 1. This does **not** complete any source migration T037–T045, register a
provider rule or satisfy SC-009.

T046's integration inventory and fixture manifest have matching provisional
25-path coverage. Each path records explicit missing-material blockers and
none claims an unreviewed difference was accepted. The regenerated comparator
report has 25 blocked, 0 passed and 0 difference_pending; the independent
migration audit has no structural errors, reports coverage_closed=false and
ready=false. No source migration T037–T045 can be checked from this inventory.

T047/T048 registry-to-CLI mechanism tests include versioned rule selection,
configuration-fingerprint change rejection, blocked-station fallback,
matched-rule execution failure, standalone-file original mode and a synthetic
verified multi-artifact source `cat` retaining the complete tile identity.
The built-in wheel contains no passed provider rule, so these checks confirm
fail-closed integration without claiming a real legacy display migration.

From the project root, the specified regression command
`uv run pytest -q tests/contract tests/integration tests/terminal tests/sources/test_tile_adapter_replay.py`
exited 0: **445 passed, 3 provider-gated skipped, 1 NumPy ABI warning** in
26.99 seconds. `uv run ruff check python tests scripts/validation` and
`git diff --check` both exited 0. The focused quickstart contract/audit/gray
test selection exited 0: **73 passed**. The installed-wheel smoke is recorded
separately below.

The quickstart's direct `radiust list sources --json` exited 0 with 24 source
records. Its `cat --file ...gif --raw --renderer text` conflict exited 2 as
specified. Offline `radiust discover all --json` exited 5 and returned one JSON
report with 26 targets: 4 missing_credentials, 21 network_restricted, 1
retired, and all counts consistent; the command did not authorize provider
requests. `scripts/validation/audit_migration.py` exited 0 while explicitly
reporting display migration `25 blocked / 0 passed`, unclosed coverage and
`ready=false`. `compare_legacy_display.py` exited 1 as required for its
25 blocked entries and wrote `validation-results/legacy-display.json`.

Acceptance ledger (a passing mechanism test proves only the scope named here):

| Criterion | Evidence and outstanding acceptance |
| --- | --- |
| SC-001 | Installed-wheel PNG/GIF single-file previews and source raw lifecycle tests pass; a real parent SIGINT now proves the blocked preview worker is reaped and its owned temp root removed within five seconds. No live provider request was made. |
| SC-002 | Deterministic default/explicit-latest tests in the regression cover selection equivalence; no live latest-provider claim. |
| SC-003 | Injected catalog/status accounting and offline 26-target report pass; blocked plugin/catalog edge cases remain T052/T058/T062. |
| SC-004 | Raw `cat` now has a process-group hard-cancel test and PTY termios restoration passes; aggregate-discovery browser/plugin/descendant cases still remain under T052/T060. |
| SC-005 | Automated 40/80/120-column PTY, pipe and JSON tests pass; the real 30-second human review of three report types remains pending under T028. |
| SC-006 | **Pending:** no real 30-second human reviewer result for success, no_data and partial_failure reports. |
| SC-007 | Raw preview workers disable cache writes, strip storage credentials and preserve cache/output sentinels through cancellation; broader redirect, byte-limit and concurrency arbitration cases remain under T053. |
| SC-008 | 25 provisional paths across 22 sources are registered as blocked; old-source/product completeness cannot be claimed without old code/config evidence. |
| SC-009 | Synthetic comparator mechanism tests pass; 25 real source-matched legal raw/old-baseline pairs remain blocked. |
| SC-010 | Synthetic legacy-rule CLI/registry tests pass without scientific promotion; zero real provider display paths are verified or enabled. |

T066 remains unchecked because these results do not meet all ten acceptance
criteria or substitute for the human and real source evidence.

## Isolated wheel installation smoke (2026-09-22, T065)

`uv build --offline --wheel --out-dir /private/tmp/radiust-task002-wheel`
exited 0 and built `radiust-0.1.0-cp312-cp312-macosx_11_0_arm64.whl`.
The `[tool.maturin].include` rule in `pyproject.toml` includes
`python/radiust/resources/**/*.json` in the wheel. A fresh isolated Python
3.12 environment installed the built wheel and its cached runtime
dependencies with `uv pip install --offline`, exit 0. With `/private/tmp` as
working directory, importing the installed native `_core` succeeded and the
installed `radiust.resources/legacy_display/index.json` resolved through
`importlib.resources`: all 25 registered paths remained `blocked`; none was
automatically enabled.

The installed `radiust cat --file ... --renderer text` succeeded for the
fixture GIF (680×680, first frame, `display=original`) and PNG (1020×800,
`display=original`). In the same isolated installation without SciPy,
`require_recovery_dependency()` raised `MissingDependencyError` with the
expected `recovery requires the 'recovery' extra` message; the probe's exit
code was 1 as expected. No provider gray-display evidence was generated by
this packaging smoke test.

## Provisional display-path inventory and evidence model (2026-09-22, T032–T033)

The inventory now records **25 provisional source/product/path_id entries across
22 source IDs**, covering the 14 named regions, five tile source types and the
independent tw-http, th_royalrain and id_sidarma acquisition paths. The TH GIF
composite and MY regional branches are separately enumerated using retained
source fixture and catalog evidence. WU maps to wunderground. Every entry is
still blocked; **zero legacy display paths are validated or auto-enabled**.

The source inventory, current catalog and migration notes establish known
branches only. The old display commit
`8d251601ca551fbd5c05451f1fb337fc4b75362c` was not present in the
current repository object database. Old code/config and product override
coverage remain unclosed; the inventory does **not** assert complete old
product/path coverage or SC-008 acceptance. Source-specific matched legal raw,
gray baselines and old display rules remain missing. MY's historical display
PNG cannot stand in for its original upstream GIF; TW's verified numeric grid
is not evidence of a legacy gray rendering rule. Scientific migration records
and existing source status fields were not changed.

`display/rules.py` introduces versioned rule identity, an ordered explicit
operation list with individual basis, input constraints, SHA-256 configuration
fingerprints and independent display evidence. A `passed` evidence record
requires baseline/provenance, rule identity, hashes and comparison fields;
unaccepted differences and missing evidence are rejected. It does not register
or enable source rules; later T034–T050 require the real rendering engine,
offline source-matched comparison and audited registry. The separate
`migration/legacy-display.schema.json` defines inventory, rule, per-path
evidence and future per-source display_migration shapes without altering
acquisition or scientific acceptance state.

New synthetic **mechanism** tests in
`tests/contract/test_legacy_display_evidence.py` passed: 8 passed. The full
offline regression after these edits: `uv run pytest -q`: **539 passed,
25 skipped, 1 NumPy ABI warning**, exit 0. `uv run ruff check` on the new
Python files, JSON parsing/shape checks, and `git diff --check` passed. A
full JSON Schema validator is not installed in this environment; runtime
schema validation and legal legacy goldens remain for T030/T036/T050.

## Aggregate-discovery report and target contracts (2026-09-22, T051)

The discover-all contract now verifies actual product/station ownership,
no-station products, full eleven-status counts and zero fields, known
capability restrictions independent of successful discovery, status/exit code
mapping including interruption, stable null-first ordering, rejection of
filters/invalid max-age before catalog access, safe error projection and
max-age passed into bounded per-target work.

`uv run pytest -q tests/contract/test_cli_discover_all.py tests/integration/test_discovery_deadline.py tests/integration/test_discovery_limits.py`:
38 passed. `uv run ruff check python tests scripts/validation` and
`git diff --check`: passed. This contract check does not prove T052/T053's
entire hard timeout and global/host/source inter-worker arbitration matrix;
those remain unchecked.

## PTY and narrow-terminal automated matrix (2026-09-22, T022)

`uv run pytest -q tests/terminal/test_cli_pty.py tests/terminal/test_cli_progress.py`:
11 passed. Cases cover 40/80/120 columns with Chinese, combining characters,
long station IDs and errors retained; real POSIX PTY SIGINT termios restoration;
TTY stderr progress with unknown/known total and final cursor clearing before
the image summary; NO_COLOR PTY without escape sequences; piped CLI text and
single-object JSON without controls; and root/subcommand quiet/verbose
conflicts before source discovery. `uv run ruff check python tests
scripts/validation` and `git diff --check`: passed. Timed human evaluation
of success/no_data/partial_failure reports remains T028, not covered by this
automated matrix.

## SDK-independent progress callbacks (2026-09-22, T027)

Optional stage/count callbacks are now accepted by synchronous discover,
fetch, fetch_many and download, by asynchronous fetch_many/download, and by
the corresponding public download and batch facades. The download pipeline
emits actual discover/acquire/decode/commit and per-frame completion events;
raw-only frames omit decode. CLI download and scientific cat translate these
events into stderr ProgressEvents and close the progress display before the
final report or image. Batch resolution and completion callbacks operate
without any terminal module. The no-callback SDK path remains silent.

`uv run pytest -q tests/integration/test_pipeline_progress.py tests/contract/test_cli_output.py tests/terminal/test_cli_progress.py`:
22 passed. `uv run ruff check python tests scripts/validation` and
`git diff --check`: passed. T022/T028 still require the specified PTY and
timed human acceptance evidence; those tasks remain unchecked.

Full offline regression after the progress callback integration:
`uv run pytest -q`: **519 passed, 25 skipped, 1 NumPy ABI warning**, exit 0.
Ruff and `git diff --check` passed. Source/credential live cases are skipped;
remaining feature acceptance is not implied by this regression result.

## Human reporting and command diagnostics (2026-09-22, T021/T025/T026)

Human DownloadReport output now uses the same public per-frame identity and
counts projection as JSON instead of flattening all dataclass internals into
one line. Human-only suggestions are attached only to known error codes;
empty schema-v1 reports explicitly display `No data found`. JSON fields,
types and explicit JSON-over-quiet behavior remain unchanged.

List, discover, download, doctor, config and cache have labeled command
headings. Doctor/config add an allowlisted diagnostic group on verbose human
output; cache accepts quiet/verbose at root, group and leaf levels and detects
conflicts across those scopes. Cache status/gc/clear still emit their existing
JSON payloads in verbose JSON mode. All diagnostic fields pass through the
shared safe text/value projection.

`uv run pytest -q tests/contract/test_cli_output.py tests/contract/test_cli_reports.py tests/contract/test_cli_human_reporting.py`:
23 passed. `uv run ruff check python tests scripts/validation` and
`git diff --check`: passed. The complete PTY matrix, real per-frame SDK
progress callback and three reports' timed human acceptance remain unverified.

Current full offline regression after raw and report changes:
`uv run pytest -q`: **514 passed, 25 skipped, 1 NumPy ABI warning**;
the process exited 0. Live/provider tests account for the skips. This does
not establish the remaining deadline, concurrency, PTY or display-migration
acceptance criteria.

## Raw preview integrity and ownership (2026-09-22, T011)

The source raw preview path now consumes an already acquired RawFrame, checks
the selected artifact against its receipt again before rendering and retains
independent RGBA pixels. It rejects unverified or changed path-backed bytes,
preserves original raw/manifest contents, and leaves RawFrame ownership with
the caller. A conservative temporary-memory budget accounts for the RGBA
conversion and detached copy; PNG/GIF identification remains based on the
actual decoded format, with GIF frame index zero.

`uv run pytest -q tests/integration/test_raw_preview_lifecycle.py tests/contract/test_cli_raw_preview.py tests/contract/test_raw_replay.py tests/contract/test_cli_cat.py tests/terminal/test_renderers.py tests/integration/test_raw_modes.py`:
42 passed. `uv run ruff check python tests scripts/validation` and
`git diff --check`: passed. This is bounded raw decoding and ownership
validation, not proof of T009/T014's five-second cancellation and hard
termination requirements; those tasks remain unchecked.

## Latest identity and CLI parity (2026-09-22, T019–T020)

Fixture and legacy adapters now use a shared per-product/per-station latest
selection that retains distinct candidates tied at the latest valid time.
Fixture acquisition binds artifacts to the entire selected FrameRef, including
its revision, so equal-time revisions cannot silently use another candidate's
payload. Single-frame cat rejects ambiguity; SDK Query still requires an
explicit selector.

Default/explicit matrix (offline tests): discover and download matched for
default product and multiple stations, including the same latest timestamp;
source cat matched for scientific and raw modes and rejected multiple candidates.
Explicit at/range and base-time remain explicit; max-age remains attached to
latest. File-mode multi-time NetCDF still requires an explicit time selector.

`uv run pytest -q tests/integration/test_latest_identity.py tests/contract/test_cli_latest_defaults.py tests/contract/test_query.py tests/contract/test_cli_acquisition.py tests/contract/test_cli_raw_preview.py`:
39 passed. `uv run ruff check python tests scripts/validation` and
`git diff --check`: exit 0. No live provider calls or source golden validation
were performed. Full feature acceptance remains outstanding.

Current full offline regression after the latest identity fix:
`uv run pytest -q`: **501 passed, 25 skipped, 1 NumPy warning**.
Credentialed/live provider tests account for the skips; full Task 002
requirements, process budget and legacy golden evidence are not accepted by
this general regression alone.

## Baseline (2026-09-22)

`uv sync --group dev` completed with existing project constraints. The baseline
`uv run pytest -q tests/contract/test_cli_acquisition.py tests/contract/test_cli_cat.py tests/contract/test_cli_reports.py tests/contract/test_cli_output.py tests/contract/test_query.py tests/terminal/test_cli_pty.py`
passed: 21 tests in 1.15 seconds. Network access was not enabled.

This is a baseline only, not acceptance of the new feature.

## Implementation verification (2026-09-22)

`uv run pytest -q` completed with 421 passed and 25 skipped. The skips require live-source access or provider credentials. A NumPy binary compatibility warning remains. This is an offline regression result, not live-provider or legacy display acceptance.

The current raw preview handles standalone PNG/GIF and rejects unverified multi-artifact compositions. The legacy display inventory is provisional: per-source old rules, licensed raw/golden samples and pixel comparisons have not been established. Gray-display migration and source-specific validation remain blocked. Full discover-all deadline, worker descendant cleanup, global host/request arbitration, SIGINT preservation and human PTY acceptance also remain unverified. TODO checkboxes remain open until their complete acceptance criteria are met.

After the bounded file read and CJK display-width fixes, `uv run pytest -q tests/contract tests/integration tests/terminal` passed: 279 passed, 3 credential-gated skips, 1 NumPy warning. `uv run ruff check python tests scripts/validation` passed. These checks do not validate live sources or the missing legacy fixtures.

## Additional offline checks

- The source-worker boundary now creates a POSIX process group and stops the worker's subprocesses with TERM followed by KILL. Two integration tests passed, including a subprocess that explicitly ignores TERM; this validates that injected scenario only, not all browser, plugin or SIGINT paths.
- Single-source discover/download/cat now emit best-effort TTY-only stage progress to stderr and clear it before final human/JSON output or raw preview. SDK-level per-frame progress callbacks and the human PTY acceptance matrix remain outstanding.
- The shared CLI ISO-8601 time parser now feeds all three time-selecting commands. Query/conf focused regression passed: 12 tests. The raw-file byte limit is enforced during reading, before image decoding.
- `uv run radiust discover all --json` with default offline configuration exited 5 as specified. Its JSON had schema_version=1, exactly 26 target items, 4 missing_credentials, 21 network_restricted, 1 retired, and counts.total equal to the sum of individual status counts. No public-source request was enabled.
- The historical old-implementation commit `8d251601ca551fbd5c05451f1fb337fc4b75362c` was not available in the current repository's object database. The present legacy inventory cannot establish complete old source/product path coverage or substitute for legitimate matched raw/golden source evidence. No old credentials were accessed during these checks.

## Final discover-all limits, cancellation and offline acceptance (2026-09-23, T052–T067)

The coordinator retains global, shared-host and per-source worker leases until
each child exits. Until child-to-parent request IPC exists, every discovery
worker receives one local request/frame/host slot; workers share one
conservative parent host bucket, so even unknown-host and browser-backed work
cannot multiply the configured aggregate ceiling. `HTTPTransport` applies
thread-safe request and hostname semaphores through response-body completion,
moves the host lease only after redirect authorization, and releases host
slots during retry backoff. Playwright contexts route requests through local
request/host semaphores and release leases on request-finished/request-failed.
This intentionally serializes unknown discovery work rather than distributing
a full global request quota to every child.

Parent cancellation sets a process-shared event first. The default worker
watcher cancels the active `Client` task and its shared `Cancellation`; source
context cleanup gets 0.5 seconds. The parent then uses process-group TERM,
KILL and bounded joins, and removes only its operation temp root. Injected
cooperative-worker tests confirm it exits without TERM; separate tests cover
blocked HTTP/browser operations, a TERM-ignoring descendant, external SIGINT,
preserved completed results, `cancelled`/`not_started` states, and lease/temp
reclamation. Terminal-session restoration remains covered by the PTY suite.
No live provider or public-network request was used.

Final verification:

- `uv run pytest tests/contract tests/integration tests/terminal tests/sources/test_tile_adapter_replay.py -q`: **477 passed, 3 credentialed provider tests skipped**, one existing NumPy ABI warning, exit 0 (46.28 seconds).
- `uv run ruff check python tests scripts/validation` and `git diff --check`: passed.
- Focused deadline/limits/browser/transport/progress regression: **60 passed**. Focused source and discover-all contracts for SIDARMA, PAGASA, Windy, config and CLI: **71 passed**.
- Quickstart latest/raw checks: **32 passed**; discover/deadline/limits checks: **51 passed** before the final cancellation additions; terminal/report matrix: **40 passed**; legacy gray/audit mechanism checks: **23 passed**.
- Offline GIF and PNG previews exited 0 (680×680 GIF, first frame; 1020×800 PNG). `cat --file ... --raw` returned 2 with the expected option-conflict message; both fixture hashes remained unchanged. `list sources --json` produced valid JSON. Offline `discover all --json` returned one parseable report with 26 targets: 21 network_restricted, 4 missing_credentials and 1 retired; its expected exit code was 5.
- The legacy comparator returned 1 as expected and wrote **25 blocked, 0 passed, 0 difference_pending**. The migration audit returned 0 with 24 source records, no missing records or structural errors, but `coverage_closed=false` and `ready=false`.

Acceptance ledger (automated evidence does not replace human or source-matched
evidence):

| Criterion | Final evidence and remaining acceptance |
| --- | --- |
| SC-001 | Offline source raw and PNG/GIF previews pass; no formal scientific output is produced by raw preview. |
| SC-002 | Default/explicit latest identity and ambiguity contract tests pass. |
| SC-003 | Catalog expansion, target accounting, all statuses, limits and exit-code contract pass; offline report contains 26 complete target rows. |
| SC-004 | Blocking HTTP/browser, IPC backpressure, worker descendants, cooperative cancellation, hard kill, SIGINT, terminal restore and temp cleanup tests pass within the five-second bound. |
| SC-005 | Automated 40/80/120-column, colorless, pipe/JSON and PTY matrix passes. |
| SC-006 | **Passed:** the user reviewed success, no_data and partial_failure separately and confirmed each was readable within 30 seconds; recorded below under T028. |
| SC-007 | Parent/local request-host-source peaks, disabled-network and missing-credential skips, redirect authorization, byte/resource limits, secret stripping and lease recovery pass; no public provider request occurred. |
| SC-008 | The provisional inventory records 25 known paths across 22 source IDs, but historical old code/config is unavailable and full old product/path coverage remains unclosed. |
| SC-009 | **Blocked:** 25 paths lack legal matched raw/old-baseline evidence; 0 passed comparisons. |
| SC-010 | Mechanism and packaging tests pass, but 0 real provider display rules are verified or auto-enabled; scientific status was not promoted. |

The only unchecked tasks in this feature are source migration batches T037–T045.
T028 passed the actual human timing review recorded below. T037–T045 need old
source/product rules plus legally usable, source-matched raw and golden
evidence; the inventory and synthetic mechanism fixtures do not satisfy them.
T060, T062, T066 and T067 are checked on the final tests and records above.
The migration audit's unrelated open items belong to the separate migration
feature and were not changed here.

Files changed for this final discovery/cancellation verification slice:
`python/radiust/discovery.py`, `python/radiust/discovery_worker.py`,
`python/radiust/discovery_limits.py`, `python/radiust/transport.py`,
`python/radiust/pipeline.py`, `python/radiust/sources/browser.py`,
`tests/contract/test_transport.py`,
`tests/integration/test_discovery_deadline.py`,
`tests/integration/test_discovery_limits.py`,
`tests/integration/test_browser_acquisition.py`,
`tests/integration/test_review_regressions.py`,
`specs/002-cli-experience/tasks.md`, `TODO.md` and this validation record.
No Rust code, credentials, provider data or scientific migration status was
changed by this slice.

## Earlier checkpoint (2026-09-22; superseded by the 2026-09-23 verification above)

The earlier `uv run pytest -q` run passed **428 tests with 25 skips** and one NumPy binary-compatibility warning. That checkpoint predates the final discover-all cancellation and shared-limit work. The current full regression and task status are recorded in “Final discover-all limits, cancellation and offline acceptance” above. Direct PNG/GIF fixture previews were raw previews, not legacy gray baselines. No commit, live-source request or publication was performed.

## T028 human report review packet (2026-09-23)

The requested T021/T022/report/capability selection passed **35 tests**:

```text
uv run pytest tests/contract/test_cli_output.py tests/contract/test_cli_reports.py tests/contract/test_cli_human_reporting.py tests/terminal/test_cli_pty.py tests/terminal/test_capabilities.py -q
35 passed in 2.51s
```

The automated terminal matrix covers 40, 80 and 120 columns; CJK/combining-width wrapping; preservation of source, station, UTC time, status and long error text; pipe/JSON control-sequence safety; and PTY progress cleanup and terminal restoration. Ruff passed for the report renderer and its human-output contract test.

The samples below are deterministic synthetic `DownloadReport` values passed through the production `emit()` renderer at the default 80-column width. They make no provider request and are not migration or provider evidence. The user subsequently reviewed each report independently and confirmed that the outcome/counts, source/product/station/time, and output location or failure/suggestion were understandable within 30 seconds. Those results are recorded below and T028 is checked.

### Success

```text
DOWNLOAD
command: download
run_id: review-success
query: latest: True
counts: written: 1; skipped: 0; failed: 0; cancelled: 0; not_started: 0;
        planned: 0
interrupted: False
Items: 1
#1
source: demo
product: composite
station: sample-a
valid_time: 2026-09-23T03:00:00.000000Z
logical_id: 82f1ee9b3ad6c9163fd23c33a37e61c609e6eb93e8fee8e53890682273083faf
status: written
output_uri: output/demo/composite/sample-a/20260923T030000Z.png
```

### No data

```text
DOWNLOAD
command: download
run_id: review-no-data
query: source: demo; product: composite; station: sample-a; latest: True
counts: written: 0; skipped: 0; failed: 1; cancelled: 0; not_started: 0;
        planned: 0
interrupted: False
Items: 1
#1
source: demo
product: composite
station: sample-a
valid_time: 2026-09-23T03:00:00.000000Z
logical_id: 82f1ee9b3ad6c9163fd23c33a37e61c609e6eb93e8fee8e53890682273083faf
status: failed
error: code: no_data; message: No frame matched the selected query; retryable:
       False
Suggestion: Check the selected time, product and station for available frames.
```

### Partial failure

```text
DOWNLOAD
command: download
run_id: review-partial
query: source: demo; product: composite; latest: True
counts: written: 1; skipped: 0; failed: 1; cancelled: 0; not_started: 0;
        planned: 0
interrupted: False
Items: 2
#1
source: demo
product: composite
station: sample-a
valid_time: 2026-09-23T03:00:00.000000Z
logical_id: 82f1ee9b3ad6c9163fd23c33a37e61c609e6eb93e8fee8e53890682273083faf
status: written
output_uri: output/demo/composite/sample-a/20260923T030000Z.png
#2
source: demo
product: composite
station: sample-b
valid_time: 2026-09-23T02:00:00.000000Z
logical_id: 5b2bdaf864a299aa8bc7653646a050969ee62042d2a01a9d8268c20e4c8f17f6
status: failed
error: code: upstream_failed; message: Provider request failed; retryable: True
```

### Human timing results

| Report | ≤30 seconds | Reviewer notes |
| --- | --- | --- |
| Success | Pass | User confirmed readable within 30 seconds (2026-09-23) |
| No data | Pass | User confirmed readable within 30 seconds (2026-09-23) |
| Partial failure | Pass | User confirmed readable within 30 seconds (2026-09-23) |

At this earlier checkpoint, the only unchecked task IDs were T037–T045; the old commit and source-matched output pairs had not yet been supplied. The 2026-09-23 follow-up below supersedes that evidence count. No scientific decoder status was promoted.

### Public history recovery check

The registered upstream is [chorust/radiust](https://github.com/chorust/radiust). A read-only ref check exposed only `main` (`8bf35e466924cf432ad52469aedc0ca9f8d4022d`) and no tags; its fetched commit history contains four commits. Fetching the requested old SHA into a disposable bare repository returned `not our ref`. The same owner's public repository list contains no obvious legacy-source archive. Thus the old parser/config snapshot is not reachable from the currently advertised public refs. No repository branch or worktree was changed, and no credentials were accessed. At this checkpoint the source batches still lacked an accessible snapshot; the user later supplied one, recorded below.

### Fixture evidence present by batch

The following table is a historical checkpoint recorded before the user supplied the old snapshot. The replay update below supersedes its missing-pair statements for the paths now passed.

The following files are acquisition candidates already retained in `tests/fixtures/sources`; they do not supply the missing old display output or prove that the raw is matched to the old configuration. Fixture `license_basis` text is preserved as written and is not expanded into a broader rights claim.

| Task | Existing raw candidates | Still missing for the batch |
| --- | --- | --- |
| T037 AU/CA/ES | `au/raw/IDR021.T.202609180511.png`; `ca/raw/CASFT.gif`; `es/raw/ESCOMP.png` | Old source/product rules, matched old gray baselines and explicit rights verification for the retained AU/CA artifacts. The ES manifest requires AEMET attribution and current upstream conditions. |
| T038 ID/KR/MY | `id/raw/ID-ICUSRC-MAJ_20251230075900_merca_r_source.png`; `kr/raw/KWK.png`; `my/raw/east.png`, `peninsular.png` | ID raw came from a local ignored output tree and redistribution/use terms are unverified. MY files are historical display artifacts, not canonical raw, with redistribution permission unestablished. Old rules and matched gray baselines are absent. |
| T039 NZ/PH/SG | `nz/raw/NZAU2.gif`; `sg/raw/SGCOMP.png`; no PH raw | Old rules and matched baselines; a lawful PH raw capture. Existing NZ/SG manifests say retained public artifacts remain subject to upstream terms. |
| T040 TH/TW/VN | `th/raw/cmp1.gif`, `kkn240Loop.gif`; `tw/raw/O-A0058-005.png` and metadata JSON; `vn/raw/PLI.png`; alias candidates `th_royalrain/raw/takhli.png` and `tw-http/raw/CV1_3600.png` | Complete old product/station/alias scope, old rules and matched gray baselines. Existing official-source artifacts still require the terms recorded in their fixture manifests. |
| T041 FR/PT | `fr/raw/FRCOMP.png`; `pt/raw/PTST2.png`; `pt/reference/IPMA-radar-rain-intensity-legend.png` | Old `_png_to_map`/product rules and matched old outputs. The PT legend is a reference legend, not a golden gray image. |
| T042 RainViewer | Four tiles under `rainviewer/raw/` | Old tile product combinations, color/range/composition rules and matched legal old baseline. |
| T043 Windy | Four tiles under `windy/raw/` | Old tile rules, time binding and matched baseline; the manifest limits retained live tiles to migration validation and leaves production use subject to upstream terms. |
| T044 OpenSnow/WU | No raw; fixture manifests only | OpenSnow probe returned a provider error; WU requires a key not present in this workspace. Raw, rights, old rules and matched baselines remain absent; no old credentials were read. |
| T045 BMKG | No raw; fixture manifest only | Public tile probe returned a provider error; raw, rights, tile identity, old rules and matched baseline remain absent. |

The inventory additionally has blocked `id_sidarma/cmax`; its fixture manifest says no raw was retained because the API requires an authorized key. Its old-source scope must be resolved from the missing snapshot rather than by reading credentials. These records explain why the existing regional/tile fixtures cannot close T037–T045 by themselves.

### User-supplied old snapshot replay — 2026-09-23

The user supplied `/Users/blizhan/data/code/旧项目/旧项目-旧项目`, branch `lz/release-260324`, containing exact commit `8d251601ca551fbd5c05451f1fb337fc4b75362c`. Read-only inspection confirmed the old parser and country/product YAML in that commit. The old repository was not modified, and its credential/config-auth files were not read.

The ignored local `output/` tree contains 442 same-stem `_source`/`_map` pairs across the eight targeted folders AU, ID, KR, MY, SG, TH, TW, and VN. It also contains 41 `br_cptec` pairs, outside the fourteen-country Task 002 migration set; that source has no parser/config path in the pinned `8d251601` snapshot and its separate migration record points to another source commit. No per-image upstream license or sidecar provenance was present. The user explicitly confirmed the local paired samples can be used for this migration validation; this is recorded as a validation-only attestation and does not assert broader redistribution rights.

The display engine reproduces the pinned old VQ palette parser, masks/inpaint/cookbook, `×3.2` gray encoding and resize behavior, plus FR/PT `_png_to_map` and RainViewer/Windy tile byte transforms. Fifteen paths pass exact comparison: eight historical raw/gray paths (`au/composite`, `id/composite`, `kr/composite`, `my/composite/east`, `my/composite/peninsular`, `sg/composite`, `tw/observation`, `vn/cmax`) and seven outputs generated by replaying pinned old code on migration-validation fixtures (`ca/rain`, `es/composite`, `fr/composite`, `nz/rain`, `pt/composite`, `th/composite/kkn240Loop`, `th_royalrain/cappi`). Current inventory: **15 passed / 0 difference_pending / 8 blocked**.

MY East selects pinned `MY/base.yaml` for the current station: all three locally authorized raw/gray pairs are 1113×650 inputs with 568×640 baselines, and each old replay matches its baseline pixel-for-pixel. The current display engine reproduces the registered pair with output hash `4aebf01dfad273429ba4f568f396e11c14bc80919c1912dc8ccd33b79ac68c36`, shape 568×640, and zero pixel differences. A separate MYCOMP72 replay is retained as supplemental evidence. The current station identity remains provider-native; the decision does not rely on old project aliases or investigate the old route choice. Pair hashes and replay records: [`archive-candidates.json`](../tests/fixtures/legacy-display/my/east/archive-candidates.json).

PH has no data to compare: `output/ph` contains no files, the pinned snapshot has no PH raw/gray pair, and the current acquisition record retained no verified raw; T039 remains open. RainViewer and Windy old transforms were recorded and tried against available current tiles, but the old rules are not required to run on the changed tile formats in this task. The user accepted T042 and T043 as complete, deferring new matching to future algorithm work. For the current Windy task, the user identifies red as radar and green as likely phase; this is a scope decision, not a new pixel-compatibility claim. BMKG returned HTTP 403 with no input tile; the user accepted T045 as complete until a new source becomes available. All three affected paths remain blocked from automatic legacy display conversion, with probe evidence retained.

T038 is complete: current MY East uses `MY/base.yaml`, verified by three archived pairs and an exact current-engine comparison. T040 remains complete with TH `cmp1` outside its gate; the current TW station remains provider-native `CV1_3600`. T042, T043 and T045 are closed at the user-approved task scope, while their unverified display paths remain blocked; T044 keeps OpenSnow/WU outside legacy-display and preserves their acquisition records. Only **T039 (PH)** remains unchecked because no matched raw/gray pair exists. No source scientific status changed.
Earlier full offline regression before the current scope/probe follow-up: `PYTHONPATH=/private/tmp/radiust-legacy-opencv:/private/tmp/radiust-scipy uv run pytest -q` — **631 passed, 25 skipped, one existing NumPy ABI warning**, exit 0 (71.39 seconds). At that checkpoint the display inventory still included OpenSnow/WU and reported 14 passed / 1 difference_pending / 10 blocked. The updated counts and task states are recorded in the investigation follow-up below.


### Legacy-display investigation follow-up — 2026-09-23

The regenerated comparator reports **15 passed / 0 difference_pending / 8 blocked** across 23 in-scope paths. The migration audit has `structural_errors=[]`, `coverage_closed=false`, and `ready_for_v1=false`; eight paths still lack source-matched display evidence or remain outside the accepted transforms. The MY East candidate inventory covers all three locally authorized migration-validation pairs; it does not claim wider redistribution rights. RainViewer/Windy probes remain arithmetic evidence only, and PH/BMKG still have no replayable matched image pair.

After the MY East rule and evidence update, the focused display, registry, migration audit and inventory suite passed **82 tests**. Ruff and `git diff --check` pass. The regenerated audit has `structural_errors=[]`, 15 passed / 0 difference_pending / 8 blocked display paths, and `ready=false` because source-bound evidence remains incomplete. The prior full-suite attempt completed with **631 passed, 25 skipped, 2 failed, and 2 setup errors**. The object-storage and TW HTTP loopback tests could not bind (`PermissionError` on `127.0.0.1`) in this managed environment; the installed-wheel smoke failed during `uv pip install` of the local wheel.
