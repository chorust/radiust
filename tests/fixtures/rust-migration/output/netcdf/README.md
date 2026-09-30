# Rust NetCDF4 readback fixture

`field.nc` is a synthetic 2×2 NetCDF4 field written by `radiust-core` with the
NetCDF writer. It contains float32 reflectivity, uint16 quality flags, a UTC
time coordinate, EPSG:4326 coordinates/CRS, affine metadata, units, and
provenance. `tests/contract/test_rust_netcdf_readback.py` reads it using the
independent Python xarray/netCDF4 stack. Regenerate it with
`cargo run --example rust_netcdf_fixture -- tests/fixtures/rust-migration/output/netcdf/field.nc`.
