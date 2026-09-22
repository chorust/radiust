# User Story 4: terminal validation

**Status:** Terminal/CI acceptance passed through the user-selected runnable GitHub Actions path; interactive visual acceptance remains unverified.

| Check | Result |
|---|---|
| `uv run pytest -q tests/terminal` | 11 passed; includes ANSI rendering in a POSIX PTY |
| `uv run pytest -q tests/terminal tests/contract/test_rendering.py tests/contract/test_cli_acquisition.py tests/contract/test_cli_output.py` | 17 passed |
| ANSI orientation, title, legend, and color sequence | Asserted in automated renderer and PTY tests |
| Kitty chunk bounds and iTerm2 OSC sequence structure | Asserted by protocol tests; no emulator visual check |
| Non-TTY image-sequence behavior and text rendering | Covered by CLI contract tests |
| Visual Kitty/iTerm2, SSH/tmux, and Ctrl-C restoration | Not verified in an interactive terminal; see [terminal matrix](terminal-matrix.md) |

The user explicitly accepted a runnable GitHub Actions path as an alternative to Kitty/iTerm2/SSH/tmux visual review. The workflow's `terminal-contract` job is triggered on pushes and pull requests and runs the terminal, CLI acquisition, and CLI output contracts on GitHub-hosted Ubuntu. `act -W .github/workflows/offline.yml -l` successfully parsed and listed both the `offline` and dependent `terminal-contract` jobs. The local equivalent command passed **24 tests**, including a real POSIX PTY subprocess. T128/T129 are accepted on this CI path; this does not claim that the GitHub-hosted job has already run or that emulator visuals were inspected.

Latest recheck: the installed iTerm2 process is visible in the application
inventory, but desktop safety denied UI automation access. No visual screenshot
or interactive terminal protocol was captured. The [terminal matrix](terminal-matrix.md)
records the denial, the still-passing PTY/renderer tests, and the hosted-CI
limitation for the uncommitted worktree.

Latest local recheck: the terminal, CLI acquisition/output/cat, and rendering
contract command passed **24 tests**, including the POSIX PTY subprocess.
Kitty/iTerm2 appearance, SSH/tmux passthrough, and interactive Ctrl-C recovery
remain unverified; these are outside the explicitly selected CI acceptance
path.

Latest workflow recheck: `act -l` parsed `.github/workflows/offline.yml` and
listed the `offline` and `terminal-contract` jobs. This validates workflow
discovery only; no hosted workflow has run from this worktree, and a local
`act` job execution is not counted as a pass.

The latest full Python suite passed **317 tests with 25 explicit skips**. The
local terminal check remains the 24-test automated PTY/rendering/CLI subset.
Interactive visual and remote-session checks remain unverified, while T128 and
T129 are closed via the user-selected runnable GitHub Actions alternative.
