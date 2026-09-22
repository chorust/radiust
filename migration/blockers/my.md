# my migration blocker

The checked-in files under `tests/fixtures/sources/my/raw/` are retained legacy
PNG display outputs from commit `8d251601ca551fbd5c05451f1fb337fc4b75362c`.
They are useful for explicitly injected offline lifecycle and packaging tests,
but do not preserve the native upstream GIF payload, response metadata, palette
semantics, or native geometry. The registered `MySource` now performs live HEAD
discovery and raw image acquisition. It preserves the legacy
`Last-Modified - 9 minutes` timestamp rule as an unverified inference and fails
if the response omits a timezone-aware timestamp or image content type. It is
not marked as a scientifically validated source migration.

To close this blocker, capture a legally usable upstream raw response and its
discovery response, record the valid time binding and native geometry, then add
reference pixels and quality expectations to the fixture contract.

## Official endpoint recheck — 2026-09-20

The current METMalaysia Peninsula radar page still links to
`https://www.met.gov.my/data/radar_peninsular.gif`. A browser capture of that
page's observed image asset returned one 826×640 PNG frame (163,011 bytes,
SHA-256 `b8cbdd85fee3707757cac49298889964f64065a45d719e904acfaf2e1ee3ae4f`).
The picture itself labels the product `CAPI (dBZ)` / `COMP_PM`, shows
`09:40 / 20-Sep-2026`, and names an Azimuthal Equidistant projection and 518 km
range. This is useful acquisition evidence, but it does not provide a
machine-readable frame time or a pixel-to-coordinate transform/control points.

The official myMETdata radar product page describes downloadable raw data in
Rainbow native format and images in PNG at 5- or 10-minute intervals, with one
year of availability; it also presents a fee for the image product. This does
not establish permission to retain the public-page image in this repository.
METMalaysia's copyright notice states that its site content, including images,
is government-owned and that reproduction or distribution requires prior
express written consent. No captured image was added to the repository.

The official Web Service API currently documents general weather forecast
data and directs requests for other meteorological data to METMalaysia. It
does not document a public radar endpoint. The myMETdata raw-radar listing
identifies Rainbow native data and a fee of RM100 per 100 MB; no account,
order, or licensed sample is available in this workspace. These pages do not
provide the missing frame-level valid-time binding or georeferencing control
points.

The old scraper's fallback to request time and its fixed nine-minute adjustment
to `Last-Modified` are not a verified observation-time contract. The registered
`MySource` currently preserves that adjustment as explicitly unverified and
fails closed if `Last-Modified` is absent. A bounded Radiust HTTPS request from
this environment failed certificate verification because the chain contains a
self-signed certificate; certificate checks were not disabled, and the attempt
wrote no payload. The browser image capture took about 16 minutes and did not
retain response headers. The public image route and current adapter therefore
remain provisional acquisition code, not an accepted live `latest` contract or
a canonical replay fixture.

**T011/T038 remain open** until METMalaysia provides written reuse permission
or a suitable licensed data/API route, plus machine-readable valid time and
native georeferencing evidence. The official pages checked were
[`Radar Peninsula`](https://www.met.gov.my/en/pencerapan/radar-semenanjung/),
[`myMETdata radar images`](https://mymetdata.met.gov.my/shop/product/76/radar-images),
[`myMETdata radar raw`](https://mymetdata.met.gov.my/shop/product/77/radar-raw),
the [METMalaysia Web Service API](https://api.met.gov.my/), and
[`Copyright Notice`](https://www.met.gov.my/en/info/kenyataan-hak-cipta/).

## Locked legacy repository audit — 2026-09-20

The local repository recorded by `migration/inventory.json` is available at
`旧项目`, and it contains the pinned
commit `8d251601ca551fbd5c05451f1fb337fc4b75362c`. Its tracked tree contains 97
files; neither that tree nor the reachable Git object paths contains a
`.gif`, `.png`, `.jpg`, `.jpeg`, `.webp`, `.nc`, `.h5`, `.zip`, or `.bin` raw
artifact. The legacy `core/scrapers/my_scraper.py` fetches the current public
image at runtime, falls back to request time, and subtracts nine minutes from
`Last-Modified`; it does not preserve a source raw fixture or validate that
timestamp rule. The repository working tree is clean. Therefore the old
commit supplies implementation history, not the missing canonical source
payload or scientific reference.

## Ignored legacy output recheck — 2026-09-21

A fresh inspection of the old checkout's ignored `output/my/` directory found
two extra files whose names end in `_merca_r_source.gif`. They are not tracked
by the pinned commit. Both have PNG signatures, so the `.gif` suffix does not
identify canonical upstream GIF bytes:

| Local ignored file | PNG dimensions / bytes | SHA-256 |
|---|---:|---|
| `MY-ICUSRC-MYCOMP71_20251229094001_merca_r_source.gif` | 826×640 / 130,752 | `5fbdb05aef222040120f603d12bbd3ce2402f0cfe382df25e5b394dc33df3e92` |
| `MY-ICUSRC-MYCOMP72_20251229094201_merca_r_source.gif` | 1113×650 / 175,002 | `36526789719a319a8ef9e4e09b7b0a255ffbad34875e53e83d1d09eb3bdf5415` |

Visual inspection shows rendered CAPI (`dBZ`) map artwork. The first image is
labeled `COMP_PM`, Azimuthal Equidistant, 518 km; the second is `COMP_SS`,
Albers Equal Area, 785 km. Both display `17:40 / 29-Dec-25`, although their
filenames carry different 09:40:01 and 09:42:01 stamps. The legend labels its
thresholds in `mm/h`, while the product caption says `dBZ`. These labels are
useful investigation leads, not authoritative time binding, palette semantics,
or pixel-to-coordinate control points. No discovery response or HTTP headers
were retained with either file.

No candidate was copied into the fixtures. METMalaysia's [copyright notice](https://www.met.gov.my/en/info/kenyataan-hak-cipta/)
says portal images are government-owned unless indicated otherwise and that
reproduction requires express prior written consent. The [myMETdata radar listing](https://mymetdata.met.gov.my/shop/product/77/radar-raw)
lists raw data in Rainbow Native Format and images in PNG, at 5- or 10-minute
intervals, with a fee; no licensed sample or written reuse permission is available here. The
candidate files are rendered local outputs, not the licensed raw sample needed
by T011/T038. Those tasks remain open pending authorized canonical raw bytes,
discovery/time evidence, and usable native geometry.

## Live registered-adapter CLI recheck — 2026-09-21

After adding Certifi's public roots alongside the platform trust store, an
explicit `download my --latest --raw-only --no-cache --json` run completed with
exit code 0: both `east` and `peninsular` were written, with no failed or skipped
items. TLS chain and hostname verification remained enabled. For both endpoints
the HEAD response advertised `image/gif` and returned
`Last-Modified: Mon, 21 Sep 2026 07:11:04 GMT`; the GET payloads verified as PNG.
The adapter now records the advertised type and timestamp, emits `.png` raw
artifacts with `image/png`, and rejects payloads that do not verify as PNG.

The East payload was 1113×650 RGB, 162,817 bytes, SHA-256
`886db571a9d2ec348ffcf6b7b67a9b9e1297e42c61fc2bde467a782ad5edaadc`; the
Peninsular payload was 826×640 RGB, 144,513 bytes, SHA-256
`7b6d646beaf419a97f3d4683ca4c70b47a198dde3acb8e51a0f1e55b3ee0396e`. The
CLI applied the legacy `Last-Modified - 9 minutes` rule and reported
`2026-09-21T07:02:04Z` for both. These are acquisition observations, not
independent validation of the observation-time rule. Both payloads were deleted
with the temporary CLI output because the reuse permission gap remains; the
summary is in [`live-cli-my.json`](../../validation-results/live-cli-my.json).

This clears the earlier transport-only TLS failure. It does not clear T011/T038:
there is still no licensed canonical sample, authoritative valid-time binding,
verified palette, or native pixel-to-coordinate control points.
