# Gray display codes and dBZ values

Radiust reports source-image display bytes as `gray` and scientific reflectivity values as `dbz` with units `dBZ`. The names describe different data: a gray image is a rendered code plane; a dBZ result is a numeric raster with quality and provenance. Neither mode is inferred from image appearance.

## Explicit local decoding

`gray-dbz-v1` is an explicit declaration for an image whose visible pixels are grayscale integer codes in the inclusive range 0–224:

```text
dBZ = gray × 5/16
```

The step is 0.3125 dBZ; code 0 maps to 0 dBZ and code 224 maps to 70 dBZ. A visible code outside the range, a non-integer value, a color pixel, a corrupt image, or an ambiguous multi-frame image fails validation. Use `--frame-index` for a multi-frame image. A local image has no inferred observation time, CRS, or geographic grid.

Alpha is interpreted before numeric decoding. Alpha zero means missing (`NaN` plus the missing quality bit); any nonzero alpha leaves the numeric value unchanged. Opaque black is a valid zero. For 16-bit alpha, the original `uint16` values and bit depth are retained in the result and numerical formats. A PNG preview may derive 8-bit alpha with a nonzero-preserving conversion; that preview alpha is not the numeric alpha plane.

```bash
radiust cat --file ./gray.png --raw
radiust cat --file ./gray.png --gray
radiust cat --file ./gray.png --dbz
radiust download --file ./gray.png --dbz --format netcdf --output ./data
```

With the SDK, call `Client.decode_gray_file()` or `AsyncClient.decode_gray_file()`, then pass the returned Rust-owned `RasterResult` to `write()` without inventing a `FrameRef`. Use `read_dbz()` to read an existing numeric dBZ file; the values are not multiplied by 5/16 again. See [Python SDK](python-sdk.md#gray-与-dbz).

## Source gray evidence and clipping

Source gray decoding is enabled only where the retained source/product/path/station evidence and input constraints match a `passed` entry in [`paths.json`](../tests/fixtures/gray-dbz/paths.json). The Rust Core applies that source's historical display operations, carries quality and origin masks through crop, repair, and resize, then converts the resulting gray code to numeric dBZ. FR/PT historical brightness encoding includes alpha in gray generation; its dBZ result is therefore quantized rendered intensity, not an inversion of the original colors or a newly calibrated physical measurement.

For these evidence-passed source paths only, `source-upper-clip-v1` computes `min(gray, 224) × 5/16`. This accommodates pixels emitted above 224 by a matched historical source rule. The original gray/RGBA pixels stay unchanged; each clipped location receives `encoding_adjustment=1`. Existing invalid quality stays invalid and its dBZ remains NaN. NZ has six such pixels (codes 225–229); its clip count is six, while the valid clip count depends on the preserved quality mask.

The gray evidence matrix has 15 passed paths: AU composite, CA rain, ES composite, FR composite, ID composite, KR composite, MY east and peninsular, NZ rain, PT composite, SG composite, TH kkn240Loop, TH RoyalRain CAPPI, TW observation, and VN CMAX. Eight paths remain blocked: BMKG composite, ID SIDARMA CMAX, PH composite, RainViewer composite gray, TH cmp1, TW HTTP observation, TW grid gray, and Windy reflectivity. A blocked gray path does not block an independent native dBZ decoder. In particular, RainViewer composite, TW grid, and RDCAP reflectivity use their direct numeric/native decoders and do not pass through gray quantization.

## Numeric files and output limits

Pixel dBZ writes PNG, NetCDF, and Zarr v2. A complete trusted geometry mapping also permits GeoTIFF, which stores the numeric values plus matching quality, optional origin-quality, encoding-adjustment, alpha, and provenance components. Reads verify that all declared TIFF components share dimensions, CRS, transform, and row orientation, and bind their digests into the numeric-file receipt. NetCDF and Zarr store `reflectivity` as float32 and use `row`/`column` pixel coordinates when geographic evidence is unavailable. All numeric formats preserve actual time and geometry evidence without inventing missing values. PNG output is a display image with a metadata sidecar; it does not store the numeric raster. Geographic regridding, bbox, and resolution requests fail when a complete trusted mapping is unavailable. Numerical pixel output remains valid when time or geography is unknown.

`read_dbz()` validates the real variable and units, computes a content-bound numeric-file identity and read receipt, and carries embedded gray/source processing as upstream provenance. Writing that result again without a frame reference is allowed; a supplied `FrameRef` conflicts with a local numeric identity. An old unbound `RadarField` still requires its source `FrameRef` for `write()`.

Source dBZ download attaches raw bytes only when requested and only from the same acquisition/receipt. Raw replay validates the manifest, artifact hashes, frame identity, and RDCAP bindings locally; it never performs a network refetch. A failed checksum or binding is an integrity error.

## Validation dimensions

Validation results keep these claims separate:

| Dimension | What evidence covers |
| --- | --- |
| Gray | Historical source display operations reproduce the retained gray baseline and bytes. |
| dBZ | The declared formula, native numeric values, quality masks, clip positions, units, and identities match the offline contract. |
| Geometry | Only explicit, complete coordinate/CRS evidence permits geographic output; unknown remains unknown. |
| Live | A separate live source run and independent readback are required; offline fixtures do not establish live availability or physical calibration. |

Current offline passing evidence is recorded in [`validation-results/gray-dbz-local.json`](../validation-results/gray-dbz-local.json), [`validation-results/gray-dbz-compat.json`](../validation-results/gray-dbz-compat.json), and the targeted Core/CLI/Python contracts. Those results do not upgrade blocked paths, unknown geometry, or live-provider status.
