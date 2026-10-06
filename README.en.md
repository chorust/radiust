# Radiust

[English](README.en.md) · [简体中文](README.md)

**Helping everyone access, understand, and use public weather radar observations.**

*Public radar, directly to you.* (Proposed tagline)

Radiust aims to connect people to radar sources through open tools running on their own devices. It prioritizes official sources while also supporting reliable, traceable third-party products. Data is acquired, cached, and processed locally by default; core observation features require neither a Radiust account nor a data relay operated by Radiust.

The project currently provides a CLI, a Python SDK, and the reusable Rust `radiust-core` library. The Python console command and SDK share the Rust Engine. **The Desktop application for the general public has not yet been implemented.** Viewing observations, playing time sequences, choosing default sources, switching sources manually, and falling back when a source fails are planned Desktop features. The current focus is observations, with data interfaces intended to support future forecasting models. Cross-source radar mosaics are also deferred.

[Project philosophy](PHILOSOPHY.md) · [Roadmap and Desktop acceptance goals](ROADMAP.md) · [Data source policy](DATA_SOURCES.md) · [Brand usage rules](TRADEMARKS.md)

The linked project documents and technical guides are currently in Chinese.

The current version is **0.1.0 alpha**. A complete, reviewable, fixture-backed MVP workflow can run offline: catalog and time queries, acquisition, decoding, NetCDF/PNG output, raw manifests, batch operations, caching, configuration, and text previews. The repository's `my` sample comes from historical PNG output; verifiable upstream GIF data, response metadata, and native geometry are still missing. It therefore does not establish a completed scientific source migration. Actual acquisition and scientific decoding capabilities are recorded separately in `migration/`; a catalog status of `needs_configuration` alone does not mean that acquisition has not been implemented.

See the [architecture guide](docs/architecture.md) for current module boundaries, end-to-end data flow, and local/remote output commit workflows.

## Offline quickstart

Install development dependencies from the repository root:

```bash
uv sync --group dev
```

Run a workflow without accessing the public network:

```bash
radiust list sources --json
radiust discover my --at 2025-12-29T06:50:01Z --json
radiust download my --at 2025-12-29T06:50:01Z --output ./data --json
```

A second run with the same arguments reports `skipped` based on the completion manifest. To preserve raw data:

```bash
radiust download my --at 2025-12-29T06:50:01Z --output ./data --raw --json
radiust download my --at 2025-12-29T06:50:01Z --output ./data --raw-only --json
```

When previewing files in a pipeline, select text mode explicitly:

```bash
radiust cat --file ./data/path/to/frame.nc --renderer text
radiust cat --file tests/fixtures/sources/th_royalrain/raw/takhli.png --renderer text
radiust cat --file tests/fixtures/sources/th/raw/kkn240Loop.gif --raw --renderer text
radiust cat --file tests/fixtures/gray-dbz/local/225-codes.png --gray --renderer text
radiust cat --file tests/fixtures/gray-dbz/local/225-codes.png --dbz --renderer text
radiust discover all --json
```

`--raw` shows source pixels, `--gray` declares a gray-code display, and `--dbz` strictly decodes a declared local gray image. The local rule accepts visible integer codes 0–224 and computes `dBZ = gray × 5/16`; transparent pixels are missing, opaque black is valid zero, and time/geolocation remain unknown. No mode is inferred from appearance. The old source-only `--legacy-display` flag remains as a compatibility alias for `--gray`.

Local multi-frame GIF/WebP files require an explicit frame selection. Source workflows first preserve the original artifact, then preview the rule-generated gray image. Only paths with accepted source evidence can request dBZ:

```bash
radiust cat --file ./frames.gif --raw --frame-index 0
radiust cat --file ./frames.gif --dbz --frame-index 0
radiust cat nz --product rain --latest --raw
radiust cat nz --product rain --latest --gray
radiust cat nz --product rain --latest --dbz
```

Inspect numeric files with `radiust cat --file ./reflectivity.nc --dbz --variable reflectivity`. The Python SDK can read them with `read_dbz()` and save new numeric output with `write()`, without inventing a `FrameRef` for a local file. For sources, `download --dbz --raw` attaches the original artifacts from the same acquisition to the decoded output; `--raw-only` conflicts with `--dbz`. Omitting a mode retains the existing generic/native behavior, and other scientific variables keep their actual units. JSON reports retain Envelope v1 and add `mode_schema_version: 1` and `mode_info`.

Source gray evidence, dBZ conversion limits, numeric formats, and validation status are documented in [docs/gray-dbz.md](docs/gray-dbz.md); CLI and SDK examples are in [docs/cli.md](docs/cli.md) and [docs/python-sdk.md](docs/python-sdk.md).
With network access disabled by default, `discover all` still returns the full catalog status. It typically exits with code 5 when no target successfully acquires live data. This does not establish live source acceptance.

See the [CLI guide](docs/cli.md) and [output maintenance guide](docs/output-maintenance.md) for output directory and manifest naming rules.

## Data source support

The native catalog registers **25 sources** across countries and regions in Asia, Europe, Oceania, and North America. Availability and capabilities vary by source, product, and access conditions. See the [data source policy](DATA_SOURCES.md) for the full inventory, acquisition status, and authorization conditions.

| Capability | Examples backed by validation | Limits |
| --- | --- | --- |
| Numeric reflectivity | Taiwan CWA `tw/grid`, RainViewer | Verified dBZ decoding; this does not establish quantitative support for every source |
| Rainfall-intensity categories | Singapore `sg`, Portugal `pt` | Ordinal categories, without precise per-pixel mm/h values |
| Original images / raw data | Other sources, subject to their individual status | Use `--raw-only` to preserve original files; unverified colors must not be interpreted directly as reflectivity or rainfall rates |

Online capabilities for `rdcap` in Taiwan, Japan, and the Philippines remain unverified. `uk` is retired, and the two Brazilian sources are historical migration records outside the native catalog. Details and the [historical validation snapshot](DATA_SOURCES.md#历史获取与科学验证记录) are maintained in the source document.

An adapter or a successful download does not establish accepted scientific decoding or permission for commercial use or redistribution. See [migration records](migration/sources/) and [validation evidence](validation-results/) for source-specific technical evidence.

## Python SDK

```python
from datetime import datetime, timezone

import radiust

query = radiust.Query(
    "my",
    at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc),
)

with radiust.Client() as client:
    refs = client.discover(query)
    field = client.fetch(query)       # Does not commit formal output
    report = client.download(query, output="./data", raw=True)
```

Top-level `fetch`, `afetch`, `fetch_many`, `iter_fetch`, `download`, and their corresponding asynchronous entry points are also available. `fetch` returns an independently loaded `RadarField`/`RadarDataset`; `download` performs encoding and formal output commits. See the [Python SDK guide](docs/python-sdk.md) for public objects and error types.

## Installation and optional capabilities

### Native CLI for macOS Apple Silicon (preview)

macOS arm64 users can download the standalone Rust CLI `radiust` v0.1.0, targeting macOS 11 and later, without installing Python or Rust. This is a preview release; see the [full installation instructions](docs/installation.md#macos-apple-silicon-预编译版本预览) and [GitHub release](https://github.com/chorust/radiust/releases/tag/v0.1.0) for its scope and verification steps. The standalone CLI and the Python package below are separate installation options.

### Python package

The base installation includes the Rust extension. Rust parses YAML configuration, so PyYAML is not a runtime dependency. Install scientific interoperability and other optional capabilities as needed:

```bash
pip install radiust
pip install "radiust[geotiff]"
pip install "radiust[zarr]"
pip install "radiust[playwright]"
pip install "radiust[recovery,scraping]"
```

`radiust[storage]` is intentionally retained as an empty extra. Standard wheels already include OSS/S3 remote commit support through Rust OpenDAL. For `--output s3://...` or `oss://...`, configure `storage.endpoint/region/access_key/secret_key/anonymous`. Remote output uses immutable generations and publishes the manifest last. Real-provider validation for AWS S3, S3-compatible services, and Aliyun OSS is still required; loopback tests cannot replace it. See the [installation guide](docs/installation.md) for full CPython/Rust wheel details.

## Open use and licensing

The software uses the standard [Apache-2.0 license](LICENSE) and welcomes commercial use. The project aims to keep an open, independently usable tool available to the public. The software license does not replace upstream radar data permissions or endorse downstream products. Access, attribution, commercial use, and redistribution conditions are recorded per source; unconfirmed conditions remain explicitly unconfirmed. See the [data source policy](DATA_SOURCES.md) and [brand usage rules](TRADEMARKS.md) for the project name and future visual identity.

## Development and validation

```bash
.venv/bin/ruff check python tests
cargo fmt --all -- --check
cargo test --workspace
.venv/bin/pytest -q
```

Tests disable public network access by default. Live-source, provider, and real-terminal validation require separate credentials, environments, and acceptance records; passing fixture tests does not automatically complete those checks. See the [migration guide](docs/migration.md) and `migration/blockers/` for migration status and outstanding evidence.
