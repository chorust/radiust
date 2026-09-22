# Migration status

`migration/inventory.json` preserves the old repository commit `8d251601ca551fbd5c05451f1fb337fc4b75362c` and all 24 HEAD source paths. Each HEAD path now has a dedicated adapter module, versioned source resource, migration record, and fixture manifest. Sixteen sources retain checked-in raw artifacts; eight sources have explicit blocked manifests with no fabricated raw data.

The checked-in artifacts prove acquisition and replay contracts. They do not automatically prove scientific decoding. `rainviewer` is the current source with a provider-backed pixel transform and reference value evidence. The other retained image and tile sources remain blocked where palette semantics, native extent, frame-time binding, or control points are not verified. `my` now has a registered live HEAD/GET adapter, synthetic HTTP-transcript CLI test, and successful two-station live raw-only CLI evidence with TLS verification enabled. Its endpoints advertise GIF while returning validated PNG bytes; outputs from the bounded run were deleted after hashing because reuse permission is unavailable. Authorized canonical raw, verified time semantics, and native geometry remain open.

The AU adapter has additional live evidence: the opt-in CLI run on 2026-09-18 downloaded 61 latest BoM station frames with zero failures, and the public representative smoke passed AU together with seven other sources. The online result is recorded in [`validation-results/live.json`](../validation-results/live.json); it is acquisition evidence, not scientific acceptance.

The three blocked upstream paths (`bmkg`, `cam`, `opensnow`) record the provider probes that failed to yield canonical raw artifacts. Credential-dependent paths (`id`, `id_sidarma`, `ph`, `wunderground`) fail closed without external credentials. `uk` is handled as an accepted retirement exception because Met Office DataPoint was decommissioned and has no like-for-like Radar Composite replacement. No legacy credential or token is copied into the new repository.

The geometry audit is in [`migration/geometry-audit.json`](../migration/geometry-audit.json). It found no real two-dimensional curvilinear source payload. The existing `CurvilinearGrid` contract remains covered by offline tests; no source-specific curvilinear adapter was added without source evidence.

The two historical Brazil lines are product-confirmed and remain open. CPTEC now replays its recovered legacy CAPPI contract and discovers current layers/times through WMS. One archived official WMS frame is retained as a hashed 512×512 PNG; the latest advertised frame still returns an upstream mosaic/CRS error. The WMS image is a portrayal, so its physical reflectivity palette and pixel geometry remain unverified. SIPAM has a live adapter plus a retained 1000×1000 raw PNG and declared station bbox, but still lacks a verified physical dBZ palette and pixel control points. Their evidence and blockers are in [`migration/history`](../migration/history).

The migration is therefore still open. Closing M4 requires either canonical raw plus science and geometry evidence for every source, or a separately reviewed and accepted exception. The current blocking items are summarized in [`migration/exceptions.json`](../migration/exceptions.json). Provider smoke, packaging, benchmark, terminal, and release-readiness checks remain separate gates.

The structural audit can be rerun without network access:

```bash
uv run python scripts/validation/audit_migration.py --json --output validation-results/migration-audit.json
```

It checks all 24 current source adapter/resource/test/fixture/migration links and reports the open specification tasks. It intentionally does not treat replay bytes as provider science evidence. Use `--require-complete` only when the external gates have been supplied and reviewed.
