# Installation

## 从包仓库直接安装

截至 2026-10-07，`0.1.5` 已发布到 [PyPI](https://pypi.org/project/radiust/0.1.5/)、[radiust-core](https://crates.io/crates/radiust-core/0.1.5) 和 [radiust-cli](https://crates.io/crates/radiust-cli/0.1.5)，可直接安装当前 CLI、SDK 与核心库。本版本修复 RDCAP 匿名会话票据，提供仅对 RDCAP 生效的显式 TLS 校验例外，并去掉该配置的 warning。GitHub tag 工作流构建并验证两个平台的 crate 与八个 Python wheel，包由工作流发布，PyPI Trusted Publishing 已成功；验证详情见[发布流程与验证记录](releases.md)。

```bash
cargo install --locked --version 0.1.5 radiust-cli
radiust --help

python3 -m venv .venv
.venv/bin/python -m pip install --only-binary=:all: "radiust==0.1.5"
.venv/bin/python -m radiust --help
```

Cargo 包名是 `radiust-cli`，安装后的命令名是 `radiust`；Cargo 会从源码编译，需要 Rust 1.92+、CMake、C/C++ 编译器和构建工具。Rust 项目可通过 `cargo add radiust-core` 使用同版本核心库。Python 的预编译 wheel 无需 Rust 或 CMake；tag 发布矩阵覆盖 CPython 3.10–3.13、macOS arm64 与 Linux x86_64（glibc ≥ 2.28），其他环境可使用下方源码安装方式。科学转换可安装 `radiust[science]` 等 extra。

## 原生 Rust 命令行程序

原生 `radiust` 二进制不需要 Python。macOS Apple Silicon 用户可以下载下方的预编译版本；从仓库源码构建或使用 Cargo 安装则需要 Rust 1.92 或更新版本、CMake，以及 C/C++ 编译器和构建工具。

### macOS Apple Silicon 预编译版本（预览）

GitHub 提供 macOS arm64 预览版 `v0.1.0`，支持 macOS 11 及更新版本。它是独立 Rust CLI，不是 Python 包；发布资产和后续版本见 [GitHub Releases](https://github.com/chorust/radiust/releases)。下载后先检查 SHA-256，再安装到用户级 `~/.local/bin`：

```bash
mkdir -p "$HOME/.local/bin"
workdir="$(mktemp -d)"
cd "$workdir"
curl -fL -o radiust-v0.1.0-macos-arm64.tar.gz \
  https://github.com/chorust/radiust/releases/download/v0.1.0/radiust-v0.1.0-macos-arm64.tar.gz
curl -fL -o radiust-v0.1.0-macos-arm64.sha256 \
  https://github.com/chorust/radiust/releases/download/v0.1.0/radiust-v0.1.0-macos-arm64.sha256
shasum -a 256 -c radiust-v0.1.0-macos-arm64.sha256
tar -xzf radiust-v0.1.0-macos-arm64.tar.gz
install -m 755 radiust "$HOME/.local/bin/radiust"
export PATH="$HOME/.local/bin:$PATH"
radiust --help
```

`~/.local/bin` 若尚未加入 PATH，请将 `export PATH="$HOME/.local/bin:$PATH"` 加入 `~/.zshrc` 并重新打开终端。该预览构建未经过 Apple 公证；首次运行时 macOS 可能显示 Gatekeeper 提示。确认下载来源并通过 SHA-256 校验后，再决定是否允许运行。预览版仍有未完成的来源和科学能力验收，详见下方支持边界及[迁移记录](migration.md)。

### 从源码构建或使用 Cargo 安装

原生 CLI 与 Python 包的源码安装都需要 Rust 1.92 或更新版本、系统可执行文件 `cmake`、C/C++ 编译器及构建工具。工作区启用了 NetCDF 的 `static` feature，`netcdf-src` 与 `hdf5-metno-src` 的构建脚本会调用 CMake 编译 NetCDF/HDF5；pip 和 Rust 不会自动安装这些系统工具。

macOS 用户需要 Xcode Command Line Tools（提供编译器和 `make`）；安装 CMake 可使用 Homebrew：

```bash
xcode-select --install
brew install cmake
```

等待 Command Line Tools 安装完成后再构建；已安装时可跳过。确认 `cmake` 和编译工具在 PATH 中，再运行下方命令。其他平台也需要准备相应的 C/C++ 编译器、构建工具和 CMake。

```bash
cargo build --locked --release -p radiust-cli
./target/release/radiust --help
```

安装到 Cargo 默认的可执行文件目录（通常是 `$CARGO_HOME/bin`）：

```bash
cargo install --locked --path crates/radiust-cli
radiust --help
```

也可指定隔离安装根目录：

```bash
cargo install --locked --path crates/radiust-cli --root /tmp/radiust-native
/tmp/radiust-native/bin/radiust --help
```

`radiust-cli` 通过本工作区的路径依赖使用 `radiust-core`。命令集合与当前支持边界见[原生 CLI 文档](cli.md#原生-rust-cli)。RainViewer composite 和 TW grid 的 PNG+sidecar、NetCDF4、GeoTIFF 和 Zarr writer/local commit 已接入，四种格式均有独立 fixture 读回。GeoTIFF 编码为纯 Rust，不加载 GDAL/PROJ；NetCDF/HDF5 使用 Rust crate 的静态构建，因此原生 CLI 和 Python 扩展不依赖 Homebrew NetCDF/HDF5 dylib。macOS wheel 仍执行 Mach-O 依赖审计与 delocate 检查。

本轮原生迁移的正式目标平台是 macOS arm64；`0.1.2` 的 Linux x86_64 wheel 已在 manylinux_2_28 容器中构建，并在隔离容器内从公开 PyPI 安装验证，证明该包可以安装运行，不等于 Linux 的全部迁移或科学能力已经验收。原生 CLI 目前仍是部分实现，不能据此宣称 US1–US5、24 个来源或四种输出格式全部完成。

## Python 包

Python SDK 的 `Client`、`AsyncClient`、便捷函数和 `radiust` console script 通过 PyO3 共用 Rust Engine 与原生 CLI。基础 wheel 不要求 NumPy/xarray；科学数组只在显式调用 `to_xarray()` 时转成 Python 对象。

核心包要求 CPython 3.10–3.13。PyPI `0.1.5` 提供 macOS arm64（macOS 11+）和 Linux x86_64（glibc ≥ 2.28）共八个 wheel；这些环境可使用上方直接安装命令。当前没有发布 sdist，其他架构或 Python 版本不能据此视为支持。以下命令从源码安装，需要先准备 Rust 1.92 或更新版本、CMake、C/C++ 编译器和构建工具；macOS 安装步骤见[源码构建前置依赖](#从源码构建或使用-cargo-安装)。命令须在仓库根目录执行：

```bash
python -m venv .venv
.venv/bin/python -m pip install "."
```

科学读回与其他 Python 集成按需安装：

```bash
.venv/bin/python -m pip install ".[science]"  # NumPy/xarray 科学转换
.venv/bin/python -m pip install ".[geotiff]"   # rasterio
.venv/bin/python -m pip install ".[zarr]"      # zarr v2 + numcodecs
.venv/bin/python -m pip install ".[playwright]"
.venv/bin/python -m pip install ".[recovery,scraping]"
.venv/bin/python -m pip install ".[all]"
```

`storage` extra 当前为空；标准 Rust core 编译了 OpenDAL provider。Python `Client.download()` 和原生 CLI 接受 `s3://bucket/prefix`、`oss://bucket/prefix`，并要求显式开启 `runtime.allow_network`、通过 `storage` 配置 endpoint/region/凭据。远端 generation/pointer 提交和读回协议已有内存故障合同；真实 AWS S3/阿里云 OSS provider 尚未验收。

真实 provider 仍须使用隔离 prefix 和显式凭据执行验证矩阵；本地 loopback 兼容服务只证明提交协议和 Rust transport 可运行。

## 开发环境与源码检查

```bash
uv sync --group dev
.venv/bin/ruff check python tests
cargo fmt --all -- --check
cargo test --workspace
.venv/bin/pytest -q
```

GitHub Actions 使用精简的 `ci` 依赖组运行 lint 和自动化测试，避免为 CI 安装仅供独立 NetCDF/CF 手工验收使用的 `cfchecker`/`cfunits`。完整开发环境仍使用 `uv sync --group dev`；`dev` 保留上述工具。

## CF checker（macOS + Homebrew）

`cfchecker` 依赖系统的 UNIDATA UDUNITS-2 动态库。macOS 上可以安装 Homebrew formula，并在运行 checker 的 shell 中显式提供库和 XML 数据路径：

```bash
brew install udunits
export UDUNITS2_HOME="$(brew --prefix udunits)"
export DYLD_LIBRARY_PATH="$UDUNITS2_HOME/lib${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}"
export UDUNITS2_XML_PATH="$UDUNITS2_HOME/share/udunits/udunits2.xml"

.venv/bin/python -c 'from cfunits import Units; assert Units("m").equivalent(Units("m"))'
.venv/bin/cfchecks -v auto path/to/file.nc
```

`DYLD_LIBRARY_PATH` 必须指向包含 `libudunits2.dylib` 的目录，不能直接填写 dylib 文件。若希望新终端自动生效，把上述三个 `export` 放入 `~/.zshrc`。当前项目输出声明 CF-1.8，`cfchecker 4.1.0` 可以按该版本完成检查。

构建 Python wheel（此命令构建 `radiust._core` 扩展，不会安装独立 Rust CLI）：

```bash
.venv/bin/maturin build --release --interpreter .venv/bin/python --out /tmp/radiust-wheel
```

NetCDF/HDF5 已静态编入 macOS Rust 扩展，无需为它们从 Homebrew 复制 dylib。CI 仍使用 `delocate` 检查 wheel 中其他可能的动态依赖；本地可按同样方式检查构建产物：

```bash
.venv/bin/python -m pip install delocate
mkdir -p /tmp/radiust-wheel/repaired
.venv/bin/delocate-wheel --wheel-dir /tmp/radiust-wheel/repaired /tmp/radiust-wheel/*.whl
```

应在源码树外的干净虚拟环境分别安装基础 wheel 与所需 extra，验证 `radiust`、`radiust._core`、console CLI 和科学读回。本次发布与安装记录见 [`local-registry-publication.json`](../validation-results/local-registry-publication.json)，此前构建与验收记录见 [`packaging-matrix.json`](../validation-results/packaging-matrix.json)；不同 macOS 系统库构建出来的 wheel 最低系统版本可能不同，应以修复后 wheel 标签为准。

## 配置

原生 Rust CLI 和 Python SDK 使用 Rust Engine 配置模型。配置优先级是默认值、`~/.config/radiust/config.yaml`、当前工作目录的 `config.yaml`、`RADIUST_` 环境变量；自动文件不存在时跳过，同名字段由后面的层覆盖。`--conf` YAML 替代当前目录的配置层，保留全局文件作为默认配置。Python SDK 可通过 `Client(config=...)` 提供优先于环境变量的映射。source 凭据支持成组配置并在 `config show` 中脱敏。示例文件不含真实凭据；生产环境建议使用环境变量，避免 secret 出现在 shell history。

Python SDK 的 `load_config(overrides, path, environ)` 也由 Rust 读取和校验配置；返回的 `EffectiveConfig` 保留 `values`、`origins`、路径属性和脱敏视图。YAML 配置不需要安装 PyYAML。PyYAML 仅保留在 `ci`/`dev` 开发组中，用于读取仓库内的 YAML 工作流合同。

## 迁移与兼容性状态

Python SDK 的 fetch 结果现为 Rust 绑定对象，xarray 转换需显式调用；Python source entry point 不再自动加载。原生 CLI、Rust core 和 Python wheel 的覆盖仍是分阶段状态，未完成验收的来源、科学能力、远端存储和性能场景继续列为未完成，不应据此宣称 US1–US5 全部通过。没有获准静默改变科学值、质量标记、时间/地理语义或原图与科学解码的区别。

SIDARMA、PAGASA 和 Weather Underground 的变量名、显式 opt-in 的在线命令、UK DataPoint 退役处理，以及当前没有可用凭据时的 skip 行为见 [`live-provider-tests.md`](live-provider-tests.md)。AWS S3、S3-compatible 和 Aliyun OSS 的真实矩阵按当前验收安排延期。
