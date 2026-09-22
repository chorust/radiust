# Terminal and CI validation

**Status:** Partial; visual terminal acceptance remains open.
**Checked:** 2026-09-22

The local environment is macOS 26.6.2 arm64. The project virtual environment uses CPython 3.12.7. `uv run pytest -q tests/terminal` passed 11 tests, including `test_cli_ansi_renderer_runs_in_a_real_pty_process` with `TERM=xterm-256color`. The CLI emitted its title, legend, ANSI color sequence, and north-up half-block output in the PTY. The terminal protocol tests also passed bounded Kitty chunking and iTerm2 OSC image-sequence framing. A separate combined run of the terminal, renderer, and CLI output/acquisition contracts passed 17 tests.

The tests verify generated terminal output and protocol framing. They do not establish how an image looks in a terminal emulator. Automated access to the installed iTerm2 app was denied by the desktop safety boundary; Kitty and tmux executables are unavailable in this environment. A temporary loopback-only OpenSSH probe was also blocked before authentication: macOS `sshd` logged `sandbox initialization failed: Operation not permitted` for its pre-auth child. No privilege escalation or sandbox bypass was attempted. Therefore no interactive SSH session was established. Ctrl-C recovery has not been checked in a visible terminal session. These modes remain unaccepted.

`act -W .github/workflows/offline.yml -l` parsed and listed the `offline` and `terminal-contract` jobs. `act -W .github/workflows/live.yml -l` parsed and listed its public and provider jobs. The terminal job runs `tests/terminal` plus CLI acquisition/output contracts on GitHub Actions. Local `act --dryrun` could not start its runner because Docker could not connect to the OrbStack socket; it failed before executing workflow steps. The GitHub-hosted workflow was not dispatched from this session.

The installed `act` is version 0.2.75 and reports that it is affected by CVE-2026-34041 and CVE-2026-34042. It was used only to list/parse workflow jobs; no workflow body was executed locally.

## Local contract recheck — 2026-09-21

`uv run pytest -q tests/terminal tests/contract/test_cli_acquisition.py tests/contract/test_cli_output.py tests/contract/test_cli_cat.py tests/contract/test_rendering.py` passed **24 tests**, including the real POSIX PTY ANSI subprocess and protocol transcript contracts. This verifies emitted bytes and cleanup under the automated harness; it does not provide visual Kitty/iTerm2, SSH/tmux, or visible Ctrl-C acceptance. T128 and dependent T129 remain open.

## Latest terminal/CI access recheck — 2026-09-21

The iTerm2 process is present in the desktop application inventory, but a new
UI-control attempt was explicitly denied by the desktop safety boundary
(`Computer Use is not allowed to use the app ... for safety reasons`). I did not
try another mechanism to control or screenshot the app. The PTY and protocol
tests below remain the terminal evidence; visible rendering, interactive SSH/
tmux, and Ctrl-C restoration are still unverified.

The current GitHub working tree contains uncommitted implementation files and
changes, so no hosted Actions run can execute this exact workspace from a remote
ref. The repository has no recorded workflow run for this checkout, and local
`act` still cannot start because the OrbStack Docker daemon is unavailable.
Workflow YAML and local CI commands are checked separately; hosted CI remains
pending a published branch.

## Automated terminal/CI recheck — 2026-09-21

- `uv run pytest -q tests/terminal tests/contract/test_rendering.py tests/contract/test_cli_acquisition.py tests/contract/test_cli_output.py`: **17 passed**, including the POSIX PTY ANSI process and renderer/protocol contracts.
- `uv run pytest -q`: **306 passed, 24 expected skips**. The 20 live cases skip before network access; three remote-storage tests remain deferred by instruction, and the installed-wheel test expects a CI-built artifact.
- The `live.yml`, `offline.yml`, and `wheels.yml` workflow files all parse as YAML. Workflow parsing and local contracts do not count as a hosted Actions run. `act` still cannot start jobs while the OrbStack Docker daemon is unavailable; visual Kitty/iTerm2 and SSH/tmux checks and visible Ctrl-C recovery remain open.

## Current local terminal and workflow recheck — 2026-09-21

- `uv run pytest -q tests/terminal tests/contract/test_rendering.py tests/contract/test_cli_acquisition.py tests/contract/test_cli_output.py tests/contract/test_cli_cat.py`: **24 passed**, including the POSIX PTY ANSI subprocess and protocol transcript contracts.
- `act --dryrun` could not start the offline or live runner because the Docker daemon is unavailable. The wheel workflow dry run skipped unsupported platforms; no hosted workflow was dispatched. All three workflow files parse as YAML, but this is not a CI execution result.
- Kitty/iTerm2 visual output, SSH/tmux sessions, and visible Ctrl-C restoration remain unverified. T128 and dependent T129 stay open.

The latest full offline rerun after adding the retained TMD six-frame CLI replay passed **314 tests with 25 expected skips** and one existing NumPy ABI warning. The adapter fixture test does not replace the outstanding visible-terminal or hosted-CI evidence.

## Current local verification — 2026-09-21

- Re-ran the complete terminal/rendering/CLI contract subset:
  `uv run pytest -q tests/terminal tests/contract/test_rendering.py tests/contract/test_cli_acquisition.py tests/contract/test_cli_output.py tests/contract/test_cli_cat.py` — **24 passed**.
- The runtime is macOS 26.6.2 arm64, CPython 3.12.7, with `TERM=xterm-256color`. `kitty` and `tmux` executables are not installed; OpenSSH 10.3p1 is present, but no interactive remote session was exercised. iTerm2 UI control remains denied by the desktop safety boundary, with no screenshot or app-level visual review.
- The full local offline suite passed **315 tests with 25 explicit skips**; `.github/workflows/offline.yml`'s equivalent dependency/lint/Rust/pytest and PTY contract commands passed locally. Hosted Actions and `act` job execution remain unavailable from this uncommitted worktree.
- These runs verify ANSI bytes, renderer orientation/legend, protocol framing, non-TTY rejection, and PTY execution. They do not verify visible Kitty/iTerm2 rendering, SSH/tmux passthrough, or interactive Ctrl-C restoration; T128 and dependent T129 remain open.

## Patched local runner attempt — 2026-09-21

- The system `act` binary is v0.2.75, below the versions fixed for the two recorded high-severity advisories. A temporary official v0.2.89 Darwin arm64 release was SHA-256 verified against its published asset digest (`48ae218af96725f7635a66de2b87e1e346893b02add0f16b92f560296b2151fc`); the [v0.2.89 release](https://github.com/nektos/act/releases/tag/v0.2.89) is above the [v0.2.86 security fixes](https://github.com/advisories/GHSA-xmgr-9pqc-h5vw) and [v0.2.86 security fixes](https://github.com/advisories/GHSA-x34h-54cw-9825).
- OrbStack started successfully; Docker reported **28.5.2 linux/aarch64**. `act push -W .github/workflows/offline.yml -j offline --matrix python:3.12 --container-architecture linux/arm64 ...` began setup but stalled pulling `catthehacker/ubuntu:act-latest` without progress for over two minutes. It was cancelled before any workflow step ran, so this is **not** a CI pass.
- The exact local workflow commands and terminal contract tests pass outside the container runner. No hosted Actions workflow ran. An existing Postgres container appeared when OrbStack started and was left untouched.

## Subsequent successful local Actions job — 2026-09-21

- Retried with the same SHA-256-verified official `act` v0.2.89 binary and a temporary Ubuntu 24.04 arm64 runner image. `act push -W .github/workflows/offline.yml -j offline --matrix python:3.12 --container-architecture linux/arm64 ...` completed successfully.
- The workflow installed CPython 3.12.14 arm64, synced the locked development environment, passed Ruff, passed `cargo test --workspace --locked` (24 Rust integration tests), and passed the full pytest suite (**315 passed, 25 skipped**, one existing NumPy ABI warning). Skips were the explicit live/provider opt-ins and the CI-supplied wheel artifact.
- This is a local act execution of the `offline` job for one arm64/Python combination. It is not a GitHub-hosted run and does not verify the complete hosted OS/Python wheel matrix. The focused terminal/rendering/CLI contract subset separately passed **24 tests** on macOS, including PTY ANSI output and protocol framing.
- Kitty/iTerm2 visual review, interactive SSH/tmux passthrough, and visible Ctrl-C restoration remain unverified; T128/T129 stay open. The existing Postgres container was left untouched.

## Current-worktree local Actions recheck — 2026-09-21

- Re-ran the actual `offline` job against this worktree with official `act` v0.2.89, `radiust-act-runner:ubuntu24` (Ubuntu 24.04 arm64), and CPython 3.12.14. The job passed locked dependency sync, Ruff, `cargo test --workspace --locked` (**24 Rust integration tests**), and `uv run pytest -q` (**316 passed, 25 skipped**, one existing NumPy ABI warning).
- Invocation: `/private/tmp/radiust-act-v0.2.89/act push -W .github/workflows/offline.yml -j offline --matrix python:3.12 --container-architecture linux/arm64 -P ubuntu-latest=radiust-act-runner:ubuntu24 --no-cache-server --pull=false --action-cache-path /private/tmp/radiust-act-action-cache`.
- This is current-worktree CI evidence for one local Linux/arm64/Python combination. It is not GitHub-hosted CI or the declared wheel matrix. The focused macOS terminal/render/CLI suite passes 24 tests, including ANSI PTY output, but Kitty/iTerm2 visual output, interactive SSH/tmux, and visible Ctrl-C recovery remain unverified; T128/T129 stay open.

## Terminal-contract job retry — 2026-09-21

- Started the workflow's `terminal-contract` job under the same local runner. Its dependency `offline` job entered `uv sync --group dev --locked`, then made no output progress for 7m7s; inspection showed the `uv` process idle in `futex_wait` with no build child. I interrupted that run before it reached the terminal-contract step.
- This attempt is **not a terminal CI pass**. The current-worktree offline job above passed, and the host terminal/render/CLI subset passed 24 tests. The remote dependency installation stall leaves the container terminal job and T128/T129 acceptance open.

## Matrix-limited terminal-contract retry — 2026-09-21

- Retried with `act push -W .github/workflows/offline.yml -j terminal-contract --matrix python:3.12 --container-architecture linux/arm64` and the same verified local runner image. The dependency offline job created Python 3.12.14 and resolved the lock, then stopped producing output while building `cfunits==3.3.7`.
- Cancelled after 5m6s without reaching Ruff, pytest, or the terminal-contract step. This is another runner dependency-build stall, not a test pass or a terminal test failure. The host PTY/terminal/CLI subset remains the actionable local verification; Kitty/iTerm2/SSH/tmux visual acceptance remains open.

## User-selected GitHub Actions acceptance path — 2026-09-21

The user explicitly requested Kitty/iTerm2/SSH/tmux visual acceptance **or** a runnable GitHub Actions path. The CI alternative is selected for T128/T129. `.github/workflows/offline.yml` runs on `push` and `pull_request`; its `terminal-contract` job runs the POSIX PTY, terminal protocol, CLI acquisition, and CLI output contracts after the offline job. `/private/tmp/radiust-act-v0.2.89/act -W .github/workflows/offline.yml -l` successfully parsed the workflow and listed both jobs. The same terminal/rendering/CLI contract command passed locally: **24 passed**. The full Python suite passed **317 passed, 25 skipped**.

This records a runnable GitHub Actions acceptance path, not a completed GitHub-hosted run. Kitty/iTerm2 visual inspection, SSH/tmux passthrough, and visible Ctrl-C restoration remain unverified. The earlier local `act` terminal-job retries stalled while installing/building dependencies before reaching the terminal-contract step; they are not counted as passes. Per the user's selected alternative, T128/T129 are complete on the CI path while these interactive checks remain explicitly unverified.

## Current-worktree CI and terminal recheck — 2026-09-21

- The focused terminal/rendering/CLI/PTy command passed **24 tests** on the current worktree. `/private/tmp/radiust-act-v0.2.89/act -W .github/workflows/offline.yml -l` parses the workflow and lists `offline` and `terminal-contract`.
- Attempted the actual current-worktree `offline` job on Ubuntu 24.04 arm64 / CPython 3.12.14. The Tesseract apt step passed. `uv sync --group dev --locked` spent 22 minutes expanding wheel-cache files and was cancelled before the Ruff, Rust, or pytest steps; this attempt is not a CI pass. The matching host checks passed separately, and no GitHub-hosted Actions run was dispatched.
- T128/T129 retain the user-selected runnable GitHub Actions acceptance path and host PTY/protocol evidence. Hosted execution and the container terminal job remain unverified; Kitty/iTerm2 screenshots and SSH/tmux interactive review were not performed.

## Lean local Actions and terminal contract verification — 2026-09-22

- The workflow now installs the locked `ci` dependency group into `${RUNNER_TEMP}` and the Ubuntu 24.04 arm64 / CPython 3.12.14 `offline` job completed under SHA-verified `act` v0.2.89: Ruff, 24 Rust integration tests, and **325 passed / 25 skipped** Python tests.
- In the matching clean CI-group virtual environment, the exact terminal-contract pytest command passed **24 tests** (PTY, rendering, terminal protocol, CLI acquisition/output/cat). The dependent `terminal-contract` job itself was not separately run under Act; hosted Actions is not available for this uncommitted worktree.
- The workflow is the user's selected terminal acceptance route. Visual Kitty/iTerm2 review, interactive SSH/tmux passthrough, and visible Ctrl-C restoration remain unverified and are not claimed.

## Independent terminal-contract Actions job — 2026-09-22

- Removed the unnecessary `needs: offline` edge so the short terminal contract job can start independently and run beside the full offline matrix. The workflow parser lists both jobs at stage 0.
- Executed the current-worktree job with the previously checksum-verified official `act` v0.2.89 binary and the Ubuntu 24.04 arm64 runner image:
  `/private/tmp/radiust-act-v0.2.89/act push -W .github/workflows/offline.yml -j terminal-contract --container-architecture linux/arm64 -P ubuntu-latest=radiust-act-runner:ubuntu24 --no-cache-server --pull=false --action-cache-path /private/tmp/radiust-act-action-cache`.
- The job installed CPython 3.13.15, completed locked `ci` dependency sync, and ran the workflow's POSIX PTY, rendering, terminal protocol, CLI acquisition/output/cat command: **24 passed**. The local Actions job exited successfully. This is not a GitHub-hosted run.
- The host command also passed **24 tests**. Visual Kitty/iTerm2, interactive SSH/tmux, and visible Ctrl-C restoration remain unverified; the user-selected Actions path provides the terminal acceptance route.
- Re-ran this exact local Act job in the current continuation: the Ubuntu 24.04 arm64 / CPython 3.13.15 workflow again completed successfully with **24 passed**. GitHub-hosted Actions remains untriggered.

## Current-worktree Act re-run — 2026-09-22

- Re-executed the `terminal-contract` job with SHA-verified Act v0.2.89 and the local Ubuntu 24.04 arm64 runner image. CPython 3.13.15 setup and locked `ci` dependency sync completed; the workflow's PTY, rendering, terminal protocol, and CLI command passed **24 tests** in 2.80 seconds. The complete local Actions job succeeded.
- GitHub-hosted Actions was not triggered. This run validates the CI path in the current worktree, not visual Kitty/iTerm2 rendering, interactive SSH/tmux passthrough, or visible Ctrl-C restoration.

## Current CLI metadata terminal recheck — 2026-09-22

- The workflow-equivalent command `uv run --no-sync pytest -q -p no:cacheprovider tests/terminal tests/contract/test_rendering.py tests/contract/test_cli_acquisition.py tests/contract/test_cli_output.py tests/contract/test_cli_cat.py` passed **27 tests** locally.
- The additional three cases cover `list products SOURCE --json` for single-product, credentialed, and multi-product sources after fixing frozen `ProductInfo.units` serialization. This is automated terminal/CLI evidence; the latest local Act terminal run is the preceding **24-test** job before this regression addition.
- Hosted Actions, visible Kitty/iTerm2 rendering, SSH/tmux passthrough, and visible Ctrl-C restoration remain unverified.

## Current terminal/CI status after metadata cleanup — 2026-09-22

- The same workflow-equivalent terminal command still passes **27 tests** locally; the resource JSON cleanup did not change terminal behavior. The current-worktree local Act job remains the recorded **24-test** CI execution, and the three product-metadata cases are host-local additions.
- The runnable GitHub Actions path is configured and parsed. Hosted execution and visible emulator/SSH/tmux inspection remain unverified; no cloud provider test was started.

## Current-worktree terminal-contract rerun — 2026-09-22

- The exact `.github/workflows/offline.yml` `terminal-contract` job was rerun with local Act v0.2.89, Ubuntu 24.04 arm64, CPython 3.13.15, and the locked `ci` group. It completed successfully and ran the current terminal/CLI command: **27 passed**.
- This is a current-worktree local Actions result and supplies the user's selected CI acceptance path. GitHub-hosted Actions, visible Kitty/iTerm2 rendering, and interactive SSH/tmux behavior remain outside this local run.
