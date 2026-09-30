# US1 Native Source and Output Matrix

Run date: 2026-09-30. This report records the offline native matrix and keeps unavailable provider/science evidence explicitly blocked. Fixture adapters serve retained local bytes; they do not establish live provider behavior.

## Passed offline contracts

- The native catalog registers all 24 source adapters and expands the 26 expected discovery targets. Offline discovery preserves network-off, missing-credential, and retired-source outcomes. Raw-acquisition preflight rejects every target before network access when networking is disabled.
- The retained source fixtures contain 19 frames and 26 artifacts across 16 sources. The fixture replay contract verifies frame identity and every artifact SHA-256, then commits and reads back local raw manifests without Python.
- Native CLI process tests pass all 9 cases across the seven command groups: list, discover, download dry-run, local-file cat, doctor, config, and cache status/clear. Each test launches the Rust executable with a cleared environment.
- A single-source Rust Engine end-to-end fixture now exercises discovery, acquisition, scientific decode, NetCDF write, manifest-last commit, artifact size/SHA verification, and independent native field readback. It uses local embedded tiles and starts neither Python nor provider requests.
- The output matrix verifies RainViewer/composite through PNG, NetCDF4, GeoTIFF, and Zarr v2 writers with complete local manifests and readback. TW/grid NetCDF preserves the provider-native EPSG:3821 geometry. Raw-only local manifest, older v1 compatibility, fault injection, and cache contracts are covered separately by the core storage tests.

## Blocked and intentionally unsupported

- The 24-source catalog and offline preflight matrix is not a claim that all 24 providers were acquired live. Retained fixture coverage and science capability remain source-specific in [`migration/inventory.json`](../migration/inventory.json). Eight sources have no retained frame/artifact evidence in that matrix; credentials, retired status, or upstream errors remain visible as such.
- Real AWS S3/S3-compatible and Aliyun OSS write/readback acceptance remains open under T041. In-memory protocol/fault tests do not count as provider verification.
- Legacy display parity remains 15 passed, 0 pending, and 8 blocked; missing path-specific raw/output/provider evidence is listed in [`legacy-display.json`](legacy-display.json). Broader source science, quality/missing-value semantics, datum transformations, and projections beyond EPSG:4326↔EPSG:3857 remain open under T034. Unverified tile layouts and source-matched display paths remain open under T023.
- `source_tile_preview` includes a negative WU regression proving that a RainViewer tile artifact cannot be composed under the WU source identity. It establishes fail-closed behavior only; no WU tile dimensions, coordinates, or display rule are inferred or accepted. The user reports that no source-matched sample pairs are currently available.
- `legacy_display_parity::blocked_legacy_display_paths_keep_original_pixels` now checks all eight blocked catalog paths (`bmkg`, `id_sidarma`, `ph`, `rainviewer`, `th/cmp1`, `tw-http`, `tw/grid`, and `windy`): no blocked rule is applied, no resize occurs, and the input RGBA bytes are preserved. It uses retained raw fixtures for RainViewer, TH, TW-HTTP, and Windy; synthetic RGBA only exercises the fail-closed branch for BMKG, ID-SIDARMA, PH, and numeric TW grid. This is a fail-closed contract only; the eight source-path parity results remain blocked.
- Unsupported scientific sources continue to fail closed. The locally verified four-format matrix must not be generalized to other sources or products.

## Reproduction

```bash
cargo test -p radiust-core --test source_matrix --test download_netcdf --test source_tile_preview --offline --locked
cargo test -p radiust-cli --test native_e2e --offline --locked
```

Recorded results: core source/output contracts **21 passed**, tile preview contracts **7 passed**; native CLI process contracts **9 passed**. No remote cloud-provider calls or provider artifacts were acquired. During the later archive-catalog research, one GET to a public CWA catalog URL timed out without returning content; it was not retried and did not use a user credential.
