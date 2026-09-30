# Rust Zarr v2 fixture

`field.zarr` is a Rust-written, consolidated Zarr v2 directory store. The
`test_rust_zarr_readback.py` contract independently opens it with xarray and
checks values, quality flags, coordinates, time, CRS, dtype, chunks, and codec.

Regenerate it with:

```sh
CARGO_NET_OFFLINE=true cargo run --locked -p radiust-core --example rust_zarr_fixture -- tests/fixtures/rust-migration/output/zarr/field.zarr
```
