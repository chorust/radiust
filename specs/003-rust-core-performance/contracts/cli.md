# Native CLI Contract

This contract extends the existing v1 CLI behavior documented in `docs/cli.md` and `specs/002-cli-experience/contracts/cli.md`. The user-facing executable is `radiust` on macOS arm64. The native binary runs without Python; the Python wheel's console entry invokes the same command behavior.

## Commands and compatibility

| Command | Required behavior after migration |
| --- | --- |
| `list` | Preserve source/product/station catalog and retired/capability status without network. |
| `discover` | Preserve single-source selectors; add `discover SOURCE...` and keep `discover all`. All targets report independently. |
| `download` | Preserve processing, raw/raw-only, output format and error-policy options; process independent frames within configured limits. |
| `cat` | Preserve source/file selection, raw/legacy/decoded boundary, terminal and text renderers; first-frame path need not decode unrelated frames. |
| `doctor` | Report native and source-specific optional capabilities; network checks remain opt-in. |
| `config` | Show/validate effective settings with secret redaction; accept existing precedence. |
| `cache` | Status, GC and clear act only on cache, never formal output. |

Existing JSON envelopes, report item keys and exit status semantics are a compatibility baseline. In particular, current single-source discover returns a compact frame-list envelope, while `discover all` returns a per-target aggregate report; only explicit multi-source selection takes the new aggregate shape. Human-readable output can improve without changing machine meanings. For non-`cat` commands, stdout contains one complete final report under `--json`, while progress and diagnostics go to stderr. Quiet/TTY/NO_COLOR behavior and sensitive-text escaping remain as in the prior contract.

For `download`, the effective configuration supplies `output.format` when `--format` is omitted, and an explicitly supplied `--format` overrides it. This intentionally fixes the legacy Click command's hard-coded `netcdf` default masking the configured format.

## Discover query grammar

```text
radiust discover SOURCE [single-source options]
radiust discover SOURCE SOURCE... [--latest] [--max-age SECONDS] [--json]
radiust discover all [--latest] [--max-age SECONDS] [--json]
```

- Omitted time selector means latest. Multi-source/all accepts only latest and max-age, with no product/station/base-time/at/range selector. Explicit duplicate source, `all` plus any other source, malformed time or unknown source fails with usage exit 2 **before** network requests.
- Single source retains product/station, `--at`, full `--start/--end`, base-time and other existing valid selectors. Neither `download all` nor `cat all` is introduced.
- `runtime.discovery_workers` is a positive integer, default 4. Existing defaults remain `frame_concurrency=2`, `request_concurrency=16`, `host_concurrency=4`, `decode_workers=2`, `discovery_deadline=300s`; source-specific stricter limits apply. Config precedence follows the existing CLI/config/environment/default order. Invalid or zero values fail validation.
- Multi-source/all uses one deadline beginning with target expansion. On expiry, active targets become `timeout`, unstarted targets `not_started`. On Ctrl-C, active targets become `cancelled`, unstarted targets `not_started`; completed targets retain their result. Cleanup remains bounded by the existing 5-second deadline expectation in the prior CLI contract.

## Discover JSON v1 additive shape

Single-source JSON continues to use existing `query.source` and `items` shapes. Multi-source and `all` reports retain `schema_version: 1`, `command: "discover"`, `run_id`, `query`, `counts`, `items`, `error`, `interrupted`; `query.sources` is added only for multi-source selection. `query.source` remains `"all"` for `all`; explicit list selection uses `source: null` and its ordered `sources` array. Consumers should use `sources` for explicit lists, while all existing keys retain their type. Example:

```json
{
  "schema_version": 1,
  "command": "discover",
  "run_id": "example-run",
  "query": {"source": null, "sources": ["au", "vn"], "latest": true, "max_age": null},
  "counts": {"total": 2, "success": 1, "no_data": 0, "stale": 0, "missing_credentials": 0, "retired": 0, "network_restricted": 1, "upstream_failed": 0, "ambiguous": 0, "timeout": 0, "cancelled": 0, "not_started": 0},
  "items": [
    {"source": "au", "product": "observation", "station": null, "status": "success", "valid_time": "2026-09-24T00:00:00Z", "error": null, "frame": {"source": "au", "product": "observation", "station": null, "valid_time": "2026-09-24T00:00:00Z", "base_time": null}, "capabilities": {"scientific_decode": false, "reason": "not validated"}},
    {"source": "vn", "product": "observation", "station": null, "status": "network_restricted", "valid_time": null, "error": {"code": "network_restricted", "message": "Network access is disabled", "stage": "discover", "retryable": false}, "frame": null, "capabilities": null}
  ],
  "error": null,
  "interrupted": false
}
```

The example is schematic and does not assert the current product/station availability or scientific status of AU/VN. Real target expansion follows the catalog. `items` sort by source/product/station/valid_time as in the previous aggregate report; missing values precede known values. Source-level placeholder items preserve an unexpandable source. Successful items may include sanitized `source_urls`; raw locator values, URL userinfo, query, fragment, credentials and unsafe response bodies are not emitted.

`counts.total` equals the sum of all terminal status counts; status fields remain present with zero values. Exit precedence: interrupted 130, usage 2, all success 0, mixed success 4, no success with only no_data/stale or empty catalog 3, other no-success 5. A failed target does not discard completed targets.

The existing single-source empty-result/exception exit behavior must be frozen as golden before changing it: a returned empty list may exit 0, a raised `NoDataError` exits 3, and a download query with no refs may currently exit 2. The native migration should preserve these observable cases unless an intentional, documented versioned change is approved. This rule does not override the new multi-source aggregate rules above.

## Batch download behavior

- `on_error=collect/continue` retains every attempted frame outcome; `on_error=stop/raise` ceases dispatch on first failure and cancels active unfinished frames. Already committed frames stay reported. Existing SDK distinction between `raise` and `stop` remains documented at its boundary.
- `--dry-run` reports `planned` items without acquisition or commit; this remains a successful report when all items are planned. Current CLI `raise/stop` can discard a partial report into an error envelope; the new complete-report behavior is an intentional migration change and must be documented explicitly.
- Frame execution is bounded by `runtime.frame_concurrency`; network, host and decode activity also respect their separate shared limits. Report order follows input frame order, regardless of finish order. Progress may follow completion order on stderr.
- Output conflicts, corrupt existing manifests, interruption and unknown remote commit outcome must be visible as safe errors. A written/skipped result requires a verified formal output manifest, not just a cache hit or staged file.

## Preview and output

`cat` source raw/legacy/decoded selectors and `cat --file` rules remain as in `specs/002-cli-experience/contracts/cli.md`. Source-preview JSON includes a `source_urls` array with valid HTTP(S)/FTP locator URLs after removing userinfo, query, and fragment; this exposes useful source paths without publishing credentials or signed query strings. Local-file previews do not include source URLs. Local NetCDF selection and `--renderer text` must run natively. `download --format netcdf|geotiff|png|zarr` must run without Python and preserve format semantics described by `specs/001-radiust-v1-migration/contracts/encoding.md`; format reader comparison is semantic, not byte-for-byte. A missing optional Chromium or Tesseract affects only applicable sources; `doctor` explains it without running a network probe unless asked.
