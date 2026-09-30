# id_sidarma remaining evidence blockers

On 2026-09-30, user-provided SIDARMA request captures showed that the legacy
`/sidarma/sidarma-nowcast/android/ssxJAK.json` route returned 403 while
`/radar/v1/arsip` accepted the same externally supplied API key. The successful
capture used a local-clock query window and returned the `No Data` placeholder.
Live probes with UTC start/end parameters returned nine JAK CMAX frames with
explicit UTC timestamps. A 4084×4084 PNG was downloaded without artifact
authentication and retained with its SHA-256 in the source fixture. No key was
copied into the repository.

Image requests require the public `SidarmaMobile/2` User-Agent; an empty
User-Agent returned 403. The adapter preserves this header without including
the API key in image requests or frame locators.

The native adapter now queries a bounded one-hour UTC archive window for latest
frames, validates paired URL/time arrays and archive station/product/time
identity, and skips `No Data`. The 47-station legacy inventory remains available;
only JAK acquisition has current live evidence. Historical queries remain
unsupported.

Scientific decode remains blocked: there is no verified CMAX color-to-dBZ
palette or native pixel geometry. The inherited station bbox does not establish
image pixel control points. T053/T065 science and geometry requirements remain
open even though the canonical raw acquisition blocker has been removed.
