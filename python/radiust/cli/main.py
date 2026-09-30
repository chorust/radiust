"""Console entry point that forwards every command to the Rust CLI."""

from __future__ import annotations

import os
import sys
import tempfile
from collections.abc import Sequence
from typing import Any


class _RustCliCommand:
    """Tiny command protocol adapter for console scripts and Click's test runner.

    Argument parsing, validation, reporting, and command behavior all remain in
    ``radiust-cli``. The ``name``/``main`` surface lets existing CliRunner based
    tests invoke the same Rust entry point without reimplementing Click logic.
    """

    name = "radiust"

    def __call__(self) -> int:
        return self.main(args=None, standalone_mode=False)

    def main(
        self,
        args: Sequence[str] | None = None,
        prog_name: str | None = None,
        standalone_mode: bool = True,
        **_kwargs: Any,
    ) -> int:
        del prog_name
        from radiust import _core

        argv = list(sys.argv[1:] if args is None else args)
        if args is None:
            code = int(_core.cli_main(argv))
        else:
            code = self._invoke_capturing_native_output(_core, argv)
        if standalone_mode:
            raise SystemExit(code)
        return code

    @staticmethod
    def _invoke_capturing_native_output(core: Any, argv: list[str]) -> int:
        """Forward native fd output into the active Python test/output streams."""
        saved = (os.dup(1), os.dup(2))
        with tempfile.TemporaryFile() as stdout_file, tempfile.TemporaryFile() as stderr_file:
            try:
                os.dup2(stdout_file.fileno(), 1)
                os.dup2(stderr_file.fileno(), 2)
                code = int(core.cli_main(argv))
            finally:
                os.dup2(saved[0], 1)
                os.dup2(saved[1], 2)
                os.close(saved[0])
                os.close(saved[1])
            stdout_file.seek(0)
            stderr_file.seek(0)
            sys.stdout.write(stdout_file.read().decode("utf-8", errors="replace"))
            sys.stderr.write(stderr_file.read().decode("utf-8", errors="replace"))
        return code


main = _RustCliCommand()
