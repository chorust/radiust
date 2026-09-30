# Core and Python SDK Contract

The Rust core is the sole owner of discovery, acquisition, decoding, preview, download, persistence and catalog/config/cache operations. Native CLI and Python binding call the same operations. This is a library boundary for a future desktop application, not a desktop/IPC/Swift API in this release.

## Core operation surface

```text
Engine::new(effective_config, catalog, progress_sink, cancellation) -> Engine
Engine::sources() -> SourceCatalog
Engine::discover(query_or_source_set) -> DiscoveryReport
Engine::fetch(query_or_frame) -> RadarField | RadarDataset
Engine::download(query_or_frames, output_options, error_policy) -> DownloadReport
Engine::preview(query_or_frame_or_file, preview_options) -> Preview
Engine::doctor(options) -> CapabilityReport
Engine::cache(operation) -> CacheReport
```

The signatures above are semantic contracts; concrete Rust ownership and async trait shapes are implementation choices. Operations expose structured data, stable error code/stage/retryability, progress events and cancellation, not console output. CLI formats reports and terminal imagery. `Engine` shares transport pools, global/host/source budgets, cache leases and output commit fences across tasks, and has deterministic close/drop behavior. Caller-owned cancellation propagates to pending and running work. Public results do not expose secret locators or unbounded response bodies.

`Source` is a compile-time Rust extension with catalog metadata and discover/acquire/verified-decode operations. A source may declare raw preview and source-specific optional external capability. Acquisition returns a fully identified `RawFrame`; source code cannot publish formal outputs, bypass shared network policy or declare unvalidated scientific data. The catalog includes all 24 built-in source IDs. Third-party Python entry-point loading is removed in the new version; source authors follow the documented Rust extension path.

## Python binding and adapters

```text
sources() / Client.sources()                      -> source descriptors
Client.discover(query) / AsyncClient.discover(...) -> frame refs or structured discovery report
fetch(query) / Client.fetch(query)                 -> bound RadarField | RadarDataset
fetch_many(...) / iter_fetch(...)                  -> bound batch results, preserving prior order/error rules
download(...) / Client.download(...)               -> DownloadReport
bound_field.to_xarray() / bound_dataset.to_xarray() -> xarray scientific object (optional extra)
```

The exact convenience signatures should preserve the present Python SDK's query and error behavior except the approved default fetch return-type change. The bound result is usable for identity, shape, scientific values, quality, grid, units and provenance without importing xarray. `to_xarray()` is explicit and available only with the optional scientific dependency; it preserves values, quality bits, coordinates, UTC time and missing-value meaning. Optional Python-side Zarr adapters remain available; native CLI Zarr v2 writing does not call them. Large arrays remain core-owned until requested; conversion must not invalidate the original result.

Existing synchronous and asynchronous context management, `fetch` versus `download` distinction, one-frame ambiguity errors, ordered `fetch_many` and `on_error` semantics remain as documented in `specs/001-radiust-v1-migration/contracts/python-sdk.md` unless specifically superseded here. `fetch` does not commit formal output. Synchronous usage inside a running event loop still directs callers to async entry points. Python exceptions map stable core error codes without losing stage, safe message or partial batch result. Cancellation/KeyboardInterrupt is not a successful empty result.

## Compatibility and migration

| Old behavior | New behavior | Required migration guidance |
| --- | --- | --- |
| `fetch()` immediately yields xarray-backed scientific wrapper | Bound core scientific result by default | Show `result.to_xarray()` and corresponding optional dependency installation. |
| `radiust.sources` Python entry-point plugins | Built-in/compile-time Rust source extension | Show source trait/catalog integration, fixture replay and rebuild workflow; state that old plugins do not auto-load. |
| Python console entry orchestrates CLI | Console entry forwards to native command logic | Command names, JSON and exit meanings remain; no Python required for standalone binary. |

The new version must not silently fall back to Python implementations for a missing native source, output format or browser path. Capability errors are explicit and source-specific. The release documentation must enumerate any intentionally changed Python object type or plugin loading behavior.
