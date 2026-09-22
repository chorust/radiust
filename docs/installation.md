# Installation

## 运行时安装

核心包要求 CPython 3.10–3.13。默认安装包含 Rust 扩展和 NetCDF writer：

```bash
python -m venv .venv
.venv/bin/python -m pip install radiust
```

可选能力按需安装：

```bash
.venv/bin/python -m pip install "radiust[geotiff]"   # rasterio
.venv/bin/python -m pip install "radiust[zarr]"      # zarr v2 + numcodecs
.venv/bin/python -m pip install "radiust[playwright]"
.venv/bin/python -m pip install "radiust[recovery,scraping]"
.venv/bin/python -m pip install "radiust[all]"
```

`storage` extra 当前为空，因为 OSS/S3 能力随标准 Rust wheel 发布，不需要另装 Python extra。`s3://` 和 `oss://` 输出使用 OpenDAL 的 Rust provider；HTTP(S) 输出目标仍会明确报错。远端配置示例：

```yaml
storage:
  output: ./data
  endpoint: https://s3.example.invalid
  region: us-east-1
  # access_key/secret_key 可省略，交给 provider credential chain
  anonymous: false
```

真实 provider 仍须使用隔离 prefix 和显式凭据执行验证矩阵；本地 loopback 兼容服务只证明提交协议和 Rust transport 可运行。

## 从源码验证

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

构建 wheel：

```bash
.venv/bin/maturin build --release --interpreter .venv/bin/python --out /tmp/radiust-wheel
```

构建产物应在源码树外安装后同时导入 `radiust` 和 `radiust._core`。本机已在 CPython 3.13 macOS arm64 对 core、各单独 extra 和 `all` 分别执行干净安装与读回；Linux x86_64 上 CPython 3.10–3.13 的 extra 矩阵已加入 wheel CI，尚待 CI 实际运行。其他 Python/平台组合只按已执行记录声明，见 [`packaging-matrix.json`](../validation-results/packaging-matrix.json)。

## 配置

配置优先级是显式 Python/CLI 参数、`--conf` 文件、`RADIUST_` 环境变量、包默认值。source 凭据支持成组配置并在 `config show` 中脱敏。示例文件不含真实凭据；生产环境建议使用环境变量或 provider credential chain，避免 secret 出现在 shell history。

SIDARMA、PAGASA 和 Weather Underground 的变量名、显式 opt-in 的在线命令、UK DataPoint 退役处理，以及当前没有可用凭据时的 skip 行为见 [`live-provider-tests.md`](live-provider-tests.md)。AWS S3、S3-compatible 和 Aliyun OSS 的真实矩阵按当前验收安排延期。
