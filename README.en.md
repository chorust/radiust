<h1 align="center">Radiust</h1>

<p align="center">
  <img src="https://raw.githubusercontent.com/chorust/radiust/main/assets/branding/radiust-mark.png" width="200" alt="Radiust project mark">
</p>

<p align="center">
  <a href="README.md">简体中文</a> · <a href="README.en.md">English</a>
</p>

**Helping everyone access, understand, and use public weather radar observations.**

*Public radar, directly to you.*

Radiust aims to connect people to radar sources through open tools running on their own devices. It prioritizes official sources while also supporting reliable, traceable third-party products. Data is acquired, cached, and processed locally by default; core observation features require neither a Radiust account nor a data relay operated by Radiust.

The project currently provides a CLI, a Python SDK, and the reusable Rust `radiust-core` library. The Python console command and SDK share the Rust Engine. **The Desktop application for the general public has not yet been implemented.** Viewing observations, playing time sequences, choosing default sources, switching sources manually, and falling back when a source fails are planned Desktop features. The current focus is observations, with data interfaces intended to support future forecasting models. Cross-source radar mosaics are also deferred.

<p align="center">
  <a href="PHILOSOPHY.md">Project philosophy</a> · <a href="ROADMAP.md">Roadmap</a><br>
  <a href="DATA_SOURCES.md">Data source policy</a> · <a href="TRADEMARKS.md">Brand usage rules</a>
</p>

The current version is **0.1.5 alpha**.

[Features](#features) · [Install](#install) · [Usage](#usage) · [Project and licensing](#project-and-licensing) · [Dev](#dev)

## Features

- Radar acquisition from multiple sources: 25 catalog entries, with source, product, station, and latest-frame queries; availability and access requirements are listed in [Data sources](DATA_SOURCES.md).
- Terminal previews: view original source images, rule-generated gray codes, or dBZ values from supported products.
- Numeric reflectivity: Taiwan CWA `tw/grid` and RainViewer have [online acquisition and numeric readback records](DATA_SOURCES.md#历史获取与科学验证记录); RDCAP single-station decoding is implemented, while online capabilities remain unaccepted for all three countries.
- Data export: save raw artifacts, PNG, NetCDF, GeoTIFF, or Zarr with source, time, and processing records; scientific formats depend on product and geometry support.
- Local data read/write: read numeric files and convert explicitly declared gray-code images to dBZ; [gray conversion](docs/gray-dbz.md) does not establish a validated physical palette.
- Batches and caching: query and acquire in batches, reuse cached data, skip complete existing output, and cancel operations.
- Python SDK: synchronous, asynchronous, and batch interfaces share the Rust Engine with the CLI.

## Install

### Rust CLI

The native CLI needs no Python. Cargo compiles it from source, requiring Rust **1.92+**, CMake, a C/C++ compiler, and build tools:

```bash
cargo install --locked --version 0.1.5 radiust-cli
radiust --version
```

The Cargo package is `radiust-cli`; its executable is `radiust`. Cargo's `~/.cargo/bin` directory must be on PATH. Rust projects can use the matching core library with `cargo add radiust-core@0.1.5`. On macOS, use `xcode-select --install` for compiler tools and `brew install cmake` for CMake; see the [source build guide](docs/installation.md#从源码构建或使用-cargo-安装) and [release record](docs/releases.md) for complete prerequisites and v0.1.5 verification details.

### Python SDK

Use CPython **3.10–3.13**. Prebuilt `0.1.5` wheels cover macOS Apple Silicon (macOS 11+) and Linux x86_64 (glibc ≥ 2.28); no Rust or CMake installation is required:

```bash
python3 -m venv .venv
source .venv/bin/activate
python -m pip install "radiust==0.1.5"
python -m radiust --version
radiust --help
```

The Python package provides both the `radiust` command and the SDK. Install `"radiust[science]==0.1.5"` for NumPy/xarray conversion; see the [installation guide](docs/installation.md#python-包) for GeoTIFF, Zarr, and other optional dependencies. If you also install the standalone CLI, use `python -m radiust` to select the version in your virtual environment.

### Standalone prebuilt CLI

Download the prebuilt program without installing Python or Rust. The current `v0.1.0` preview targets macOS 11 and later. Download, verify, and install into your user directory:

```bash
mkdir -p "$HOME/.local/bin"
workdir="$(mktemp -d)"
cd "$workdir"
curl -fL -O https://github.com/chorust/radiust/releases/download/v0.1.0/radiust-v0.1.0-macos-arm64.tar.gz
curl -fL -O https://github.com/chorust/radiust/releases/download/v0.1.0/radiust-v0.1.0-macos-arm64.sha256
shasum -a 256 -c radiust-v0.1.0-macos-arm64.sha256
tar -xzf radiust-v0.1.0-macos-arm64.tar.gz
install -m 755 radiust "$HOME/.local/bin/radiust"
export PATH="$HOME/.local/bin:$PATH"
radiust --help
```

Only extract and install after verification succeeds. If a new terminal cannot find `radiust`, add the `export PATH` line to `~/.zshrc`. This preview is not notarized by Apple and may trigger Gatekeeper on first launch; see the [installation guide](docs/installation.md#macos-apple-silicon-预编译版本预览) for details and scope.

This older `v0.1.0` preview does not include all changes in `0.1.5`. Install the current version through PyPI or Cargo using the commands above.

### Install from source

Prepare the source build tools above, then run from the repository root:

```bash
git clone https://github.com/chorust/radiust.git
cd radiust
cargo install --locked --path crates/radiust-cli
```

For the Python package, use `python -m pip install "."`; use `python -m pip install ".[science]"` for optional scientific dependencies. Future versions use the [GitHub tag publishing workflow](docs/releases.md) to build, verify, and upload packages. See the [installation guide](docs/installation.md) for actual publication status by platform.

## Usage

### Config

Create `config.yaml` in the directory where you run commands to configure networking, caching, and default output:

```yaml
runtime:
  allow_network: true
  request_timeout: 30  # Per-request timeout in seconds

cache:
  enabled: true
  dir: ~/.cache/radiust

storage:
  output: ./data

output:
  format: netcdf
  grid: native
```

The CLI and SDK automatically read `config.yaml` from the current directory. Shared settings can go in `~/.config/radiust/config.yaml`; the current directory's file overrides matching fields, followed by `RADIUST_` environment variables. To select a file explicitly and inspect effective settings:

```bash
radiust --conf ./config.yaml config show
radiust --conf ./config.yaml discover rainviewer --latest
```

`allow_network: true` permits upstream requests. For temporary access, run `export RADIUST_RUNTIME__ALLOW_NETWORK=true` in the current terminal. Downloads use configured output and format defaults when `--output` and `--format` are omitted. See the [full example](config/example.yaml) and [configuration guide](docs/installation.md#配置) for more options.

### Query

- **`list`: inspect the source catalog**

  Inspect the source catalog:

  ```bash
  radiust list sources
  ```

- **`discover`: query the latest frames**

  After enabling networking in your configuration, query the latest data:

  ```bash
  radiust discover rainviewer --latest --max-age 3600
  ```

  `--max-age 3600` selects data from the last hour. Reports use UTC. `latest` means the newest frame the source provides, which can still be stale. Credentials, browser dependencies, and source configuration requirements are listed in [Data sources](DATA_SOURCES.md).

- **`cat`: terminal previews and local data**

  ```bash
  # Preview RainViewer's original tiles
  radiust cat rainviewer --latest --raw

  # Preview reflectivity from Taiwan CWA's numeric grid
  radiust cat tw --product grid --latest --dbz
  ```

  Interactive terminals select a preview renderer automatically. For redirected output, scripts, or metadata summaries, add `--renderer text`. `--raw`, `--gray`, and `--dbz` are separate modes; none is inferred from image appearance.

  Inspect an existing numeric file:

  ```bash
  radiust cat --file ./reflectivity.nc --dbz --variable reflectivity --renderer text
  ```

- **`download`: save images or numeric data**

  ```bash
  # PNG image and metadata
  radiust download rainviewer --latest --format png --output ./data

  # Numeric reflectivity with raw artifacts from the same acquisition
  radiust download tw --product grid --latest --format netcdf --raw --output ./data

  # Save only the original source data without scientific decoding
  radiust download tw-http --latest --station CV1_3600 --raw-only --output ./raw
  ```

  Download reports list actual output paths. A repeated download of a frame with complete output reports `skipped`. Numeric products also accept `--format geotiff` or `--format zarr`, subject to each source's scientific and geometry limits. Add `--json` for structured reports in automation.

See the [CLI guide](docs/cli.md) for local gray codes, frame selection, batch discovery, time ranges, cache, and configuration. File and manifest layout is documented in [Output maintenance](docs/output-maintenance.md).

### SDK

Connect to RainViewer, discover the latest frames, fetch a numeric field, and download NetCDF:

```python
from datetime import timedelta

import radiust

query = radiust.Query("rainviewer", latest=True, max_age=timedelta(hours=1))

with radiust.Client(config={"runtime": {"allow_network": True}}) as client:
    refs = client.discover(query)
    field = client.fetch(query)  # Acquire and decode into memory
    report = client.download(
        query, output="./data", format="netcdf", raw=True,
    )
```

`fetch()` returns a Rust-owned `RadarField`/`RadarDataset`; `download()` commits output. With the `science` extra installed, call `radiust.to_xarray(field)` for xarray conversion. See the [SDK guide](docs/python-sdk.md) for async entry points, batches, cancellation, local `read_dbz()`, and `write()`.

## Project and licensing

Radiust prioritizes official sources and supports reliable, traceable third-party products. The current focus is observations. Desktop viewing and playback, source selection and fallback, forecasting, and cross-source mosaics are planned work.

[Philosophy](PHILOSOPHY.md) · [Roadmap and Desktop acceptance goals](ROADMAP.md) · [Architecture](docs/architecture.md) · [Spec roadmap](.specify/memory/roadmap.md)

The software uses the standard [Apache-2.0 license](LICENSE), allowing commercial use. Upstream access, attribution, commercial use, and redistribution conditions are recorded separately by source. Unconfirmed terms remain unconfirmed; the software license does not replace data permissions. See [Brand rules](TRADEMARKS.md) for name and identity usage.

Linked project documents and most technical guides are currently in Chinese.

## Dev

Install development dependencies and run checks from the repository root:

```bash
uv sync --group dev
.venv/bin/ruff check python tests
cargo fmt --all -- --check
cargo test --workspace
.venv/bin/pytest -q
```

Default tests do not access the public network. For a sample workflow without networking, see the [offline quickstart](specs/001-radiust-v1-migration/quickstart.md). That guide includes historical migration steps; use this README for current installation. Live/provider, real-terminal, remote-storage, and platform acceptance are recorded separately; offline tests do not establish those results. See [Migration](docs/migration.md) and `migration/blockers/` for remaining limits.
