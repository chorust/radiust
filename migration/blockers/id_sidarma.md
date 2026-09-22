# id_sidarma evidence blocker

The adapter, legacy 47-station radar list, `LastOneHour`/`Latest` parsing, UTC time binding, station geometry, raw preservation, and external credential path are implemented.

On 2026-09-18 the public SIDARMA metadata endpoint returned HTTP 403 without an API key. The legacy repository contained a hard-coded provider key; that credential is intentionally not copied or used for live acquisition. No canonical current upstream raw frame can therefore be fixed without an authorized credential.

The legacy scraper also contains no verified CMAX color-to-dBZ palette. `id_sidarma` therefore rejects scientific decode instead of converting display luminance into reflectivity. T053/T061 remain open until both raw evidence and palette semantics are available.
