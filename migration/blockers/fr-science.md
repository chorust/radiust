# fr scientific decode blocker

Météo-France page/session discovery and the current WMS acquisition path were verified live on 2026-09-18. A canonical 700×600 RGBA `BASE_REFLECTIVITY` PNG is fixed under `tests/fixtures/sources/fr/raw/` with its SHA-256 and geometry/time evidence.

The legacy post-process stores an already recovered reflectivity value as `gray = clip(dBZ * 16 / 5, 0, 224)`, so a retained artifact explicitly identified as that output can be inverted with `gray / 16 * 5`. The current WMS artifact is raw RGBA imagery; it is not proven to be the legacy post-process output, and its provider color mapping and radar-native geometry remain unverified. The migrated adapter therefore preserves the raw WMS PNG and still rejects scientific decode for this input. T053/T063 remain open on the scientific reference requirement.

## Provider-native alternative checked 2026-09-20

Météo-France's official Open Data documentation describes a separate metropolitan reflectivity mosaic: 5-minute products, 20-hour retention, no archive, gzip-compressed BUFR, nominal 1 km resolution, dBZ units, and a `validity_time` catalog field. The API requires a registered bearer token. Its BUFR FAQ specifies the reflectivity code intervals (0–79) and missing-value code 255; the documentation cautions that code 0 is near the noise floor. The product is available under the Etalab Open Licence with Météo-France attribution.

This offers a route to provider-native science without guessing the WMS display ramp. It does not validate the legacy WMS PNG or its EPSG:3857 portrayal request, and does not meet the current T063 WMS acceptance by itself. An authorized API token and a retained BUFR sample plus local descriptor tables are still needed before a BUFR source path can be tested. Keep T053, T063, and T065 open.

References: [radar data products and licence](https://confluence-meteofrance.atlassian.net/wiki/spaces/OpenDataMeteoFrance/pages/670924818/Donn%2Bes%2Bradars), [targeted radar API](https://confluence-meteofrance.atlassian.net/wiki/spaces/OpenDataMeteoFrance/pages/853639355), [official BUFR reflectivity encoding](https://confluence-meteofrance.atlassian.net/wiki/spaces/OpenDataMeteoFrance/pages/1353023522).
