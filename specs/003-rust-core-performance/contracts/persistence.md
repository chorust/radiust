# Persistence and Format Compatibility Contract

This contract supplements `specs/001-radiust-v1-migration/contracts/storage.md` and `specs/001-radiust-v1-migration/contracts/encoding.md`. Existing completed outputs are user data. Cache is disposable but may be shared by old and new entry points during migration; neither may corrupt the other.

## Identity and output publication

- Preserve the current `logical_id`, resolved revision, `processing_hash` and `output_id` derivation for unchanged inputs and semantics. Source locator URLs, query tokens and credentials do not enter stable/public identity. A decoder, encoder or geolocation semantic change must advance the corresponding processing version before skip comparison.
- Recognize, validate and read v1 manifests. Skip only when every declared artifact exists and matches size/hash, identity and raw-completeness requirements. A valid older result remains usable. Damaged or incompatible formal outputs are incomplete/conflicting, never silently accepted.
- Local commit writes verified artifacts via staging and publishes the final manifest last while respecting the existing root/path lock. The existing Python lock is advisory `flock`, whereas the current Rust `RootLock` uses `create_new`; the native writer must adopt a cross-compatible lock protocol before it writes into a root used by the Python release. Concurrent frames that resolve to the same output location serialize at the commit fence; a different complete identity requires explicit overwrite.
- S3/OSS commits upload immutable generation artifacts, verify receipts, publish generation metadata and update the logical pointer manifest last. A cancelled or failed upload cannot yield a `written` result without verified final manifest. Unknown final-publish outcome must be re-read or reported as unknown; no blind retry that could overwrite a valid result.
- Cache GC/clear only touches cache-owned content, including leases and temporary files. It never deletes formal raw/scientific output or remote generations.

## Cache compatibility

The current Python `CacheStore` schema and object layout are the compatibility source for the shared default cache root. The Rust `cache/index.rs` schema is not assumed compatible merely because both use SQLite: Python uses `entries`, Rust uses `objects`, and both startup repair paths can delete files not represented in their own index. Until a compatible reader/writer, lease and repair path is proven by cross-version tests, the native implementation must use a separate cache root and must not open an old root for write/repair. It must never create a competing index over existing objects. After parity is proven, unknown or corrupt entries are isolated/evicted according to existing cache rules and re-fetched only when network permission allows.

Cache key, validator, content SHA-256, expiry and lease semantics remain observable. Raw cache reuse is not formal-output completion. Mutable latest references without trustworthy revision are revalidated; a cache hit cannot substitute for required origin confirmation. The migration guide must state whether a migrated cache can still be opened by the old Python release; if not, use a new root or an explicit one-way migration rather than silently corrupting old use.

## Format readback matrix

| Format | Native writer/reader obligation | Independent readback evidence |
| --- | --- | --- |
| NetCDF4 | Formal write and `cat --file` selection/read; preserve current CF/time/quality/grid semantics. | Existing NetCDF/CF contract plus Python xarray readback of data, quality and coordinates. |
| GeoTIFF | Preserve the current three-artifact data/quality/provenance group, with readable CRS and affine/geolocation tags; no display-only pixels presented as science. | Raster reader verifies values, quality, provenance, nodata, CRS and bounds. |
| PNG | Preserve raw and approved legacy display pixels/alpha plus the current render sidecar; scientific PNG only where existing rules allow. | Per-pixel dimensions/value/alpha and sidecar comparison. |
| Zarr v2 | Native output without Python, preserving data types, chunks, coordinates, quality, missing semantics and consolidated metadata. | Python xarray with consolidated metadata reads result; compare semantic fields. |

Byte-for-byte file identity is not required across encoders; scientific and display semantics are. Tests use legal offline source fixtures and distinguish verified science from blocked source paths. Do not claim all 24 sources scientifically valid merely because each is listed or raw-acquirable.

## Release gates

1. Old v1 manifest readback, skip/repair/conflict and local/S3/OSS fault-injection scenarios pass without half-visible committed groups.
2. Old cache sample can be read/cleaned without losing unrelated entries; same-root concurrent old/new use has an explicit supported or rejected policy backed by tests.
3. The macOS arm64 native binary and Python wheel install in clean environments. Dependency inspection finds no developer-machine Homebrew paths in released artifacts; actual NetCDF4, GeoTIFF and Zarr readback passes there.
4. Real remote-provider claims remain labeled unverified until an explicitly authorized provider matrix passes. Offline mock success establishes protocol behavior only.
