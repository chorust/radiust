# radiust-cli

Native command-line interface for the [Radiust radar toolkit](https://github.com/chorust/radiust).

```sh
cargo install --locked radiust-cli
radiust --help
radiust list sources --json
```

Source builds require Rust 1.92 or later, CMake, and a C/C++ toolchain. NetCDF/HDF5
are compiled statically. Source and scientific capabilities remain subject to the
limits documented in the repository's installation guide and `DATA_SOURCES.md`.

The executable is named `radiust`; the package on crates.io is `radiust-cli`.
