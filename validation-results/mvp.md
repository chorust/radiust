# MVP validation

Date: 2026-09-18

## Passed locally

- CPython 3.13 on macOS arm64: `uv sync --group dev --extra geotiff --extra zarr`.
- `ruff check python tests`.
- `cargo fmt --all -- --check` and `cargo test --workspace` (16 Rust integration tests).
- Offline `my` flow: list → discover → fetch → decode → NetCDF/raw commit → manifest readback → repeat skip.
- Independent `netCDF4.Dataset` readback: `reflectivity` shape `(640, 826)`, `quality` dtype `uint16`, `Conventions=CF-1.8`.
- GeoTIFF data/quality CRS and dtype readback, and Zarr v2 data/quality readback, with their optional extras installed.
- The final CPython 3.13 macOS arm64 wheel imports outside the source tree, lists all catalog entries, fetches the packaged offline fixture, writes NetCDF and passes an independent h5py readback.

## Limits of this evidence

The fixture contains two retained legacy display PNGs. It does not prove the Malaysia upstream GIF palette, valid-time binding, response metadata or native geometry. The result is therefore an offline lifecycle/MVP validation, not a scientifically accepted source migration.

CF Checker 4.1.0 validated the offline fixture NetCDF against CF-1.8 on 2026-09-20 with zero errors or warnings; the installed UDUNITS library was exposed through `DYLD_LIBRARY_PATH`. Other Python minors, architectures, live providers, remote provider matrix and real terminal sessions remain separate evidence items.
