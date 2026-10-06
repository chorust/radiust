# Radiust

<p align="center">
  <img src="https://raw.githubusercontent.com/chorust/radiust/main/assets/branding/radiust-mark.png" width="200" alt="Radiust 项目标识">
</p>

[简体中文](README.md) · [English](README.en.md)

**让普通人能够方便地获取、理解和使用公共气象雷达观测。**

*Public radar, directly to you.*

Radiust 希望让用户通过自己设备上的开放工具连接雷达来源，优先采用官方来源，也支持可靠、可追溯的第三方产品。数据默认在本地获取、缓存和处理，核心观测功能无需 Radiust 账号或 Radiust 数据中转服务。

当前提供 CLI、Python SDK 和可复用的 Rust `radiust-core`；Python console command 和 SDK 共用 Rust Engine。面向普通公众的 **Desktop 尚未实现**，其查看、时间序列播放、多源默认选择、手动切换与故障回退属于后续路线。当前先做好观测，为未来预测模型保留数据接口；跨来源雷达拼图留待后续。

[项目理念](PHILOSOPHY.md) · [发展路线与 Desktop 验收目标](ROADMAP.md) · [数据来源政策](DATA_SOURCES.md) · [品牌使用规则](TRADEMARKS.md)

当前版本为 **0.1.2 alpha**。

[Features](#features) · [Install](#install) · [Usage](#usage) · [项目与许可](#项目与许可) · [Dev](#dev)

## Features

- 多来源雷达获取：目录登记 25 个来源，可查询来源、产品、站点和最新时次；实际可用性及访问要求见[来源清单](DATA_SOURCES.md)。
- 终端预览：查看来源原图、规则生成的灰度编码或已支持产品的 dBZ 数值。
- 数值反射率：台湾 CWA `tw/grid` 和 RainViewer 有[线上获取与数值读回记录](DATA_SOURCES.md#历史获取与科学验证记录)；RDCAP 单站产品已实现数值解码，三国在线能力仍未验收。
- 数据导出：保存原始资料、PNG、NetCDF、GeoTIFF 或 Zarr，保留来源、时间和处理记录；科学格式取决于产品与几何支持。
- 本地资料读写：读取数值文件，将明确声明的灰度编码图转为 dBZ；[灰度转换](docs/gray-dbz.md)不等于物理色标已验证。
- 批量与缓存：批量查询和获取，复用缓存，跳过已有完整输出，支持取消操作。
- Python SDK：提供同步、异步和批量接口，与 CLI 共用 Rust Engine。

## Install

### Python 包：CLI 与 SDK

使用 CPython **3.10–3.13**。`0.1.2` 的预编译 wheel 覆盖 macOS Apple Silicon（macOS 11+）和 Linux x86_64（glibc ≥ 2.28），无需安装 Rust 或 CMake：

```bash
python3 -m venv .venv
source .venv/bin/activate
python -m pip install "radiust==0.1.2"
python -m radiust --version
radiust --help
```

Python 包同时提供 `radiust` 命令和 SDK。需要 NumPy／xarray 转换时，安装 `"radiust[science]==0.1.2"`；GeoTIFF、Zarr 及其他可选依赖见[安装说明](docs/installation.md#python-包)。同时安装独立 CLI 时，可用 `python -m radiust` 明确调用虚拟环境中的版本。

### 原生 CLI 与 Rust 库：crates.io

原生 CLI 无需 Python。Cargo 安装会从源码编译，需要 Rust **1.92+**、CMake、C/C++ 编译器及构建工具：

```bash
cargo install --locked --version 0.1.2 radiust-cli
radiust --version
```

Cargo 包名是 `radiust-cli`，可执行命令名是 `radiust`；`~/.cargo/bin` 需要在 PATH 中。Rust 项目可通过 `cargo add radiust-core@0.1.2` 使用同版本核心库。macOS 用户可用 `xcode-select --install` 准备编译工具，再用 `brew install cmake` 安装 CMake；完整前置依赖和 v0.1.2 发布验证记录见[安装说明](docs/installation.md#从源码构建或使用-cargo-安装)及[发布记录](docs/releases.md)。

### macOS Apple Silicon：旧版独立预编译 CLI


下载预编译程序即可使用，无需安装 Python 或 Rust。当前提供 `v0.1.0` 预览包，支持 macOS 11 及更新版本。下载、校验并安装到用户目录：

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

校验通过后再解压安装。若新终端找不到 `radiust`，将上述 `export PATH` 行加入 `~/.zshrc`。此预览包未经过 Apple 公证，首次运行可能出现 Gatekeeper 提示；处理方式及适用范围见[完整安装说明](docs/installation.md#macos-apple-silicon-预编译版本预览)。

这个 `v0.1.0` 旧预览包不包含 `0.1.2` 的全部变化；当前版本可通过上方 PyPI 或 Cargo 命令安装。

### 从源码安装

准备上述源码构建工具后，在仓库根目录运行：

```bash
git clone https://github.com/chorust/radiust.git
cd radiust
cargo install --locked --path crates/radiust-cli
```

Python 源码安装使用 `python -m pip install "."`，可选科学依赖使用 `python -m pip install ".[science]"`。后续版本由 [GitHub tag 发布流程](docs/releases.md)构建、验证并上传；各平台的实际发布状态见[安装说明](docs/installation.md)。

## Usage

### Config

在运行命令的目录创建 `config.yaml`，设置联网、缓存和默认输出：

```yaml
runtime:
  allow_network: true
  request_timeout: 30  # 单次请求超时，单位：秒

cache:
  enabled: true
  dir: ~/.cache/radiust

storage:
  output: ./data

output:
  format: netcdf
  grid: native
```

CLI 和 SDK 会自动读取当前目录的 `config.yaml`。通用配置可放在 `~/.config/radiust/config.yaml`；当前目录配置覆盖其中的同名字段，`RADIUST_` 环境变量再覆盖文件配置。也可显式指定文件并查看生效配置：

```bash
radiust --conf ./config.yaml config show
radiust --conf ./config.yaml discover rainviewer --latest
```

`allow_network: true` 允许连接上游；只想临时开启时，可在当前终端运行 `export RADIUST_RUNTIME__ALLOW_NETWORK=true`。省略 `--output` 和 `--format` 时，下载使用配置中的默认值。更多选项见[完整示例](config/example.yaml)和[配置说明](docs/installation.md#配置)。

### 1. 查询来源与最新时次

配置好联网权限后，查看来源目录并查询最新资料：

```bash
radiust list sources
radiust discover rainviewer --latest --max-age 3600
```

`--max-age 3600` 筛选最近一小时的资料。报告中的时间为 UTC；`latest` 表示来源最新提供的帧，仍可能过期。凭据、浏览器或其他来源配置要求见[数据来源文档](DATA_SOURCES.md)。

### 2. 在终端查看最新雷达

```bash
# 查看 RainViewer 原始瓦片预览
radiust cat rainviewer --latest --raw

# 查看台湾 CWA 数值网格的反射率
radiust cat tw --product grid --latest --dbz
```

交互终端默认自动选择预览方式；重定向、脚本或只需元数据时加 `--renderer text`。`--raw`、`--gray` 和 `--dbz` 是不同模式，不根据图像外观自动推断。

### 3. 保存图像或数值资料

```bash
# PNG 图像及元数据
radiust download rainviewer --latest --format png --output ./data

# 数值反射率，同时保留同次获取的原始资料
radiust download tw --product grid --latest --format netcdf --raw --output ./data

# 只保存来源原始资料，不请求科学解码
radiust download tw-http --latest --station CV1_3600 --raw-only --output ./raw
```

下载报告会列出实际输出路径。同一帧已有完整输出时，再次下载会报告 `skipped`。数值产品也可用 `--format geotiff` 或 `--format zarr`；不同来源的科学与几何限制仍适用。自动化处理加 `--json` 获取结构化报告。

查看已有数值文件：

```bash
radiust cat --file ./reflectivity.nc --dbz --variable reflectivity --renderer text
```

本地灰度编码、多帧选取、批量发现、时间范围、缓存与配置的完整用法见[CLI 文档](docs/cli.md)；输出文件和清单规则见[输出维护说明](docs/output-maintenance.md)。

### Python SDK

下面的示例连接 RainViewer，查询最新帧，读取数值场并下载 NetCDF：

```python
from datetime import timedelta

import radiust

query = radiust.Query("rainviewer", latest=True, max_age=timedelta(hours=1))

with radiust.Client(config={"runtime": {"allow_network": True}}) as client:
    refs = client.discover(query)
    field = client.fetch(query)  # 获取并解码到内存
    report = client.download(
        query, output="./data", format="netcdf", raw=True,
    )
```

`fetch()` 返回 Rust 持有的 `RadarField`／`RadarDataset`；`download()` 写入正式输出。安装 `science` 可选依赖后，用 `radiust.to_xarray(field)` 转为 xarray。异步入口、批量、取消、本地 `read_dbz()` 和 `write()` 见[SDK 文档](docs/python-sdk.md)。

## 项目与许可

Radiust 优先接入官方来源，也支持可靠、可追溯的第三方产品。当前先做好观测；Desktop 查看与播放、多源选择与回退、预测和跨来源拼图属于后续路线。

[项目理念](PHILOSOPHY.md) · [发展路线与 Desktop 验收目标](ROADMAP.md) · [架构](docs/architecture.md) · [规格路线](.specify/memory/roadmap.md)

软件采用标准 [Apache-2.0](LICENSE)，允许商业使用。上游数据的访问、署名、商用与再分发条件按来源另行记录，未确认项保持未确认；软件许可不替代数据授权。名称与标识的使用见[品牌规则](TRADEMARKS.md)。

## Dev

在仓库根目录安装开发依赖并运行检查：

```bash
uv sync --group dev
.venv/bin/ruff check python tests
cargo fmt --all -- --check
cargo test --workspace
.venv/bin/pytest -q
```

默认测试禁止公网。需要无网络样本试用时，见[离线 quickstart](specs/001-radiust-v1-migration/quickstart.md)。该文档包含历史迁移流程，当前安装方式以本文为准。live/provider、真实终端、远端存储与平台验收分别记录；离线测试通过不代表这些验收完成。迁移限制见[迁移说明](docs/migration.md)和 `migration/blockers/`。
