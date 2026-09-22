# Setup validation

Date: 2026-09-18

- `uv lock`: passed; lock file generated for the declared core and development dependencies.
- `uv sync --group dev`: passed on CPython 3.13.0, with NumPy 2.2.6, xarray 2025.6.1 and h5netcdf 1.8.1.
- `cargo check --workspace`: passed with Rust 1.92.0 and the mixed PyO3/maturin dependency graph.
- `cargo fmt --all -- --check`: passed.
- `maturin build --release --interpreter .venv/bin/python`: passed; a CPython 3.13 macOS arm64 wheel was produced in the isolated build directory.
- `pytest tests/contract tests/sources`: passed locally in the CPython 3.13 virtual environment.
- `uv sync --group dev --extra geotiff --extra zarr`: passed; optional GeoTIFF and Zarr round-trip tests are enabled locally.
- `cfchecks` remains blocked in this environment because the UDUNITS-2 shared library is unavailable; no CF acceptance claim is made.

The system interpreter is CPython 3.9 outside the declared support range; validation uses the repository `.venv` with CPython 3.13. Other Python minors and Linux architectures remain CI evidence until their clean wheel jobs run.

## Recheck on 2026-09-20

- The offline fixture NetCDF passed CF Checker 4.1.0 against CF-1.8 with zero errors, warnings, or information messages. The installed Homebrew UDUNITS library is discoverable after setting `DYLD_LIBRARY_PATH=/opt/homebrew/opt/udunits/lib`.
- CPython 3.13 macOS arm64 clean-wheel smoke passed separately for core, every named extra, and `all`; the Linux x86_64 CPython 3.10–3.13 extra matrix is configured in `wheels.yml` and awaits a CI run.
