# Migration status

`migration/inventory.json` preserves the old repository commit `8d251601ca551fbd5c05451f1fb337fc4b75362c` and all 24 HEAD source paths. Each HEAD path now has a dedicated adapter module, versioned source resource, migration record, and fixture manifest. Sixteen sources retain checked-in raw artifacts; eight sources have explicit blocked manifests with no fabricated raw data.

The checked-in artifacts prove acquisition and replay contracts. They do not automatically prove scientific decoding. `rainviewer` is the current source with a provider-backed pixel transform and reference value evidence. The other retained image and tile sources remain blocked where palette semantics, native extent, frame-time binding, or control points are not verified. `my` now has a registered live HEAD/GET adapter, synthetic HTTP-transcript CLI test, and successful two-station live raw-only CLI evidence with TLS verification enabled. Its endpoints advertise GIF while returning validated PNG bytes; outputs from the bounded run were deleted after hashing because reuse permission is unavailable. Authorized canonical raw, verified time semantics, and native geometry remain open.

The AU adapter has additional live evidence: the opt-in CLI run on 2026-09-18 downloaded 61 latest BoM station frames with zero failures, and the public representative smoke passed AU together with seven other sources. The online result is recorded in [`validation-results/live.json`](../validation-results/live.json); it is acquisition evidence, not scientific acceptance.

The three blocked upstream paths (`bmkg`, `cam`, `opensnow`) record the provider probes that failed to yield canonical raw artifacts. Credential-dependent paths (`id`, `id_sidarma`, `ph`, `wunderground`) fail closed without external credentials. SIDARMA now uses the replacement `radar/v1/arsip` UTC archive endpoint; JAK metadata and a hashed raw PNG were verified on 2026-09-30. Its CMAX palette and native pixel geometry remain unverified. `uk` is handled as an accepted retirement exception because Met Office DataPoint was decommissioned and has no like-for-like Radar Composite replacement. No legacy credential or token is copied into the new repository.

The geometry audit is in [`migration/geometry-audit.json`](../migration/geometry-audit.json). It found no real two-dimensional curvilinear source payload. The existing `CurvilinearGrid` contract remains covered by offline tests; no source-specific curvilinear adapter was added without source evidence.

The two historical Brazil lines are product-confirmed and remain open. CPTEC now replays its recovered legacy CAPPI contract and discovers current layers/times through WMS. One archived official WMS frame is retained as a hashed 512×512 PNG; the latest advertised frame still returns an upstream mosaic/CRS error. The WMS image is a portrayal, so its physical reflectivity palette and pixel geometry remain unverified. SIPAM has a live adapter plus a retained 1000×1000 raw PNG and declared station bbox, but still lacks a verified physical dBZ palette and pixel control points. Their evidence and blockers are in [`migration/history`](../migration/history).

The migration is therefore still open. Closing M4 requires either canonical raw plus science and geometry evidence for every source, or a separately reviewed and accepted exception. The current blocking items are summarized in [`migration/exceptions.json`](../migration/exceptions.json). Provider smoke, packaging, benchmark, terminal, and release-readiness checks remain separate gates.

## Rust-native migration boundary

The source inventory above describes the legacy Python migration records and their evidence; it is not a claim that every source has verified science support in Rust. The Rust compile-time registry contains 24 adapters for 24 catalog IDs and 26 discovery targets, including PH's HTTP timeline adapter; Windy remains raw-only. The native CLI supports bounded raw-only output and PNG+sidecar, NetCDF4, GeoTIFF, and Zarr v2 commits for validated RainViewer composite and TW grid paths. Python independently reads the four writer fixtures; the Zarr fixture preserves legacy dtype/chunk/Blosc settings, and nested store files use the same manifest-last commit path. The remote generation/pointer protocol is implemented with memory-backed fault tests; real AWS S3/OSS acceptance, macOS deployment-version packaging, and broader source science remain open. Other adapters and raw fixtures do not by themselves establish source science, valid-time semantics, or geometry. See [`migration.md`](../migration.md) for the Rust feature plan and current command limits.

Rust cache defaults to the isolated `~/.cache/radiust-rust` root. When explicitly pointed at a legacy root, its index can read/write Python's `entries` layout, but cross-version concurrent access is not accepted; retain root isolation. Rust reads a Python-generated v1 manifest fixture and uses manifest-last commits for local raw, PNG, NetCDF, GeoTIFF, and Zarr outputs. An Engine-level RainViewer fixture test now verifies all four decoded output formats, their manifests and artifact hashes. All four decoded format writers also have independent Python fixture readback; committed provider-output cross-read and real remote provider commits remain open. NetCDF/HDF5 use static Rust dependencies. The latest macOS 11.0 arm64 wheel (`a5613331db97599cb05fa84512a2f49464167a3c22ecc96d819f1072e777661c`) passed isolated base install, CLI/extension smoke, and Mach-O audit on macOS 26.6.2. A fresh `science,geotiff,zarr` extras environment passed four-format and raw-manifest readback; with PROJ environment variables unset, `proj.db` resolved inside that environment. The final macOS 15.0 arm64 wheel (`3bac5070a5eaaf33767066973916c6f02a088f9efb27e427b16e535b84b4f7a0`) passed the same base/Mach-O checks and clean-extras five-test readback. The legacy Python local commit adapter and source-only CLI cat/query/doctor/cache/progress helpers and safety re-export have been deleted; Rust contracts cover the supported CLI/commit paths, and legacy modules now import the shared `radiust.safety` sanitizer. The GitHub source-build workflow and execution on a macOS 11 host have not run. Do not infer S3/OSS acceptance from local tests.

The native CLI uses configured `output.format` when `download --format` is omitted; an explicit flag overrides it. The legacy Click command's fixed NetCDF default masked this setting, so the native precedence is an intentional migration behavior and is covered by an offline CLI process contract.

Rust processing identity includes separate decoder, source-resource, and encoder versions in the processing hash. A semantic change to any of those rules requires incrementing its version so a future Rust commit path cannot mistake an old output for the current result. This rule is used by the native local commit path; migration acceptance remains partial until every required format and remote provider path is covered.

## Gray and dBZ names

Active image display code and user documentation use `gray`; scientific reflectivity values use `dbz` and report units `dBZ`. The canonical local declaration is `gray-dbz-v1`: visible integer codes 0–224 map by `gray × 5/16`, while transparent pixels remain missing and time/geolocation stay unknown. Existing source display rules are referenced from `python/radiust/resources/gray/`; the historical `legacy_display` resource directory, serialized rule fields, wire version, config hashes, and Python `LegacyGrayDbzDecoder` behavior are preserved for compatibility. The CLI's source-only `--legacy-display` remains an alias for `--gray`; use `--gray` in new commands. Generic scientific decoding continues to report its actual variable and units.

旧脚本将 `--legacy-display` 改为 `--gray`；读取反射率数值时显式使用 `--dbz`。来源操作先检查 raw 和 gray，再按已通过的逐路径规则请求 dBZ：

```bash
radiust cat nz --product rain --latest --raw
radiust cat nz --product rain --latest --gray
radiust cat nz --product rain --latest --dbz
radiust cat --file ./frames.gif --dbz --frame-index 0
radiust cat --file ./reflectivity.nc --dbz --variable reflectivity
```

本地图片需调用方声明 `gray-dbz-v1`；多帧输入需 `--frame-index`，没有选择时拒绝歧义。NetCDF/Zarr Pixel dBZ 可经 `read_dbz()` 读回并由 `write()` 保存；当前数值文件身份来自实际文件摘要和变量/时次选择，保存时无需伪造来源帧。来源 `download --dbz --raw` 在同一次获取中附带原始 artifact，`--raw-only` 不能与 dBZ 解码组合。未指定模式或变量不是反射率时，generic 路径和真实单位保持旧行为。JSON Envelope v1 添加 `mode_schema_version: 1` / `mode_info`，既有字段与旧 `--legacy-display` 兼容入口保留。

The structural audit can be rerun without network access:

```bash
uv run python scripts/validation/audit_migration.py --json --output validation-results/migration-audit.json
```

It checks all 24 current source adapter/resource/test/fixture/migration links and reports the open specification tasks. It intentionally does not treat replay bytes as provider science evidence. Use `--require-complete` only when the external gates have been supplied and reviewed.
