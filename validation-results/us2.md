# User Story 2 validation

Status: framework partial

The decoder, grid, regrid, tile, rendering and output contract tests pass locally. They cover exact palette handling, threshold nearest matching, quality propagation, reversed axes, dBZ linear-domain bilinear interpolation, curvilinear coordinates, no-resize RGBA mosaics and missing-tile failure.

RainViewer and Meteo-France now have retained real raw samples and pass the opt-in public acquisition smoke. `id_sidarma` remains unverified because no authorized provider key is configured. RainViewer has a decoded reference pixel and pixel-center transform evidence; Meteo-France's WMS portrayal request grid is independently projected and checked, while the underlying native radar grid and color-to-dBZ mapping are unknown.

## Recheck on 2026-09-20

- RainViewer's source contract passed: the recorded frame timeline binds exact UTC valid times at ten-minute intervals; the registered adapter preserves all four 512×512 PNG tiles and maps the official Universal Blue RGBA reference to 10 dBZ with quality 0.
- The source CLI/SDK replay passed through the real RainViewer adapter: SDK fetch, CLI raw-only download, and CLI text cat. Its decoded output is `(1024, 1024)`, uses `dBZ`, and matches the fixture reference at row 215, column 576.
- The public opt-in RainViewer smoke passed on 2026-09-20 as part of the 8-source run. The legacy 256px WebP red-channel transform is intentionally not carried over because it has no verified physical mapping; the adapter uses the documented Universal Blue color scheme and PNG tiles instead.
- A true online CLI run with an explicit temporary network-enabled config wrote the 2026-09-20 02:50 UTC RainViewer frame and all four raw tiles. `radiust cat --renderer text` read back a `(1024, 1024)` dBZ field, and a repeat exact-time CLI download returned `skipped=1`. Independent netCDF4 readback and CF Checker 4.1.0 both passed with zero CF-1.8 errors/warnings; raw hashes and command results are in [`live-cli-rainviewer.json`](live-cli-rainviewer.json).
- The catalog and registry contract now includes a current-directory shadow-catalog test; `uv run pytest -q tests/contract/test_registry.py` passed **5 tests**. **T064 is complete** for all three US2 source descriptors, lazy factories, required-extra metadata and directory isolation.
- **T062, T063, and T064 are complete for their source-adapter scopes.** T053 remains open for SIDARMA's missing credential/raw evidence; T061 and the broader US2/T065 gate remain open for provider science and geometry references.

## Météo-France live WMS science audit — 2026-09-20

- Using the session token derived from the official radar page at request time, the WMS GetCapabilities request returned HTTP 200. The response identifies itself as WMS 1.1.1; `BASE_REFLECTIVITY` is `queryable=0`, advertises no styles or `LegendURL`, and the service advertises only GetCapabilities, GetMap, and GetFeatureInfo operations.
- A direct GetLegendGraphic probe returned an upstream HTML request-rejection page rather than a legend image. It yielded no palette values and does not alter the blocker.
- This confirms that the retained PNG is a rendered portrayal without a service-provided pixel-value or color-ramp reference. The request's 700×600 EPSG:3857 pixel-center transform is retained in FrameRef metadata and independently checked against pyproj, but does not establish the underlying radar-native grid or a physical dBZ mapping. A new retained-frame replay checks the artifact hash, all manifest RGBA samples, provider frame time, and intentional decode rejection; together with the existing WMS request tests, this completes T063's display-only migration and correction of the unsupported luminance transform. T065 remains open for actual dBZ mapping and radar-native geometry. No session cookie or token was saved; structured details are in [`migration/sources/fr.json`](../migration/sources/fr.json).
- Météo-France describes its national radar reflectivity mosaic as a dBZ product, but that product description does not document the WMS portrayal's exact PNG color mapping or pixel grid. [Official radar product description](https://services.meteofrance.com/en/node/37209).

## Regrid regression follow-up — 2026-09-20

- Fixed bilinear sampling at exact source-grid lines so zero-weight neighboring NaNs or invalid-quality pixels cannot contaminate an otherwise valid sample. Quality now receives the interpolation bit only when multiple source samples contribute; multi-variable regridding preserves the per-variable quality flags.
- `uv run pytest -q tests/contract/test_regrid.py tests/contract/test_scientific_model.py tests/integration/test_source_cli_matrix.py`: **39 passed, 1 existing NumPy binary-compatibility warning**. The full suite subsequently passed 286 tests.
- This closes the deterministic interpolation regression evidence. T065 remains open because representative real-source palette and native-grid references for SIDARMA/FR and the other representative contracts are still missing.

## IPMA source-backed palette and grid contract — 2026-09-21

- The PTST2 fixture is now decoded as ordinal IPMA rainfall-intensity intervals using the provider's linked mm/h legend; the adapter does not claim continuous rate values. Four retained colors and their category values are checked against the legend, while transparent returns remain below-detection and unknown opaque colors fail closed.
- The official Madeira map script binds every timeline image to EPSG:4326 bounds, and tests check pixel-center controls against the retained 1500×1526 frame. This closes the PT source contribution; T065 remains open for the other representative products and credential-gated sources.

## Credentialed-adapter offline continuation — 2026-09-22

- `uv run pytest -q tests/sources/test_id_sidarma.py tests/sources/test_ph.py tests/sources/test_ph_fallback.py`: **14 passed**. SIDARMA replay covers CMAX/LastOneHour/Latest de-duplication, UTC parsing, radar bbox metadata, latest-only query rejection, partial station failures, cancellation, and raw acquisition. Its decoder still rejects the image until the provider palette is verified.
- PAGASA replay covers Manila-to-UTC time conversion, CSRF propagation, HTTP/browser discovery and acquisition fallback, browser cancellation, network-policy enforcement, and refusal to bypass authentication or image-integrity errors. The direct HTTP test now also asserts that its fixture-only timeline token does not enter the `FrameRef`, identity, safe JSON projection, or redacted URL.
- No provider credential or canonical SIDARMA/PAGASA image was used. T061/T094 remain open for authorized source evidence and scientific palette/time/geometry validation; this closes only offline migration behavior coverage.
