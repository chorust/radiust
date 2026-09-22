# radiust

`radiust` 是一个独立的雷达资料获取与处理工具包，Python 提供来源、科学数据和 CLI facade，Rust `radiust-core` 提供受限 I/O、临时文件、缓存和瓦片基础设施。

当前版本是 **0.1.0 alpha**。已经可审阅和离线运行的是一个完整的 fixture-backed MVP 闭环：目录查询、时间查询、获取、解码、NetCDF/PNG 输出、raw-manifest、批量、缓存、配置和文字预览。仓库中的 `my` 样本来自历史 PNG 输出，仍缺少可核验的上游 GIF、响应元数据和原生几何，因此不能把它当成已经完成的科学来源迁移。各来源的真实获取能力和科学解码能力分别记录在 `migration/`，不能仅根据目录中的 `needs_configuration` 推断获取接口尚未实现。

当前模块边界、端到端数据流及本地/远端提交流程见 [架构文档](docs/architecture.md)。

## 离线试用

在仓库根目录安装开发依赖：

```bash
uv sync --group dev
```

运行一个不访问公网的闭环：

```bash
radiust list sources --json
radiust discover my --at 2025-12-29T06:50:01Z --json
radiust download my --at 2025-12-29T06:50:01Z --output ./data --json
```

第二次使用相同参数会根据完成清单报告 `skipped`。保存原始资料时使用：

```bash
radiust download my --at 2025-12-29T06:50:01Z --output ./data --raw --json
radiust download my --at 2025-12-29T06:50:01Z --output ./data --raw-only --json
```

预览生成的文件时，管道环境要明确选择文字模式：

```bash
radiust cat --file ./data/path/to/frame.nc --renderer text
```

实际输出目录和 manifest 的命名规则见 [docs/cli.md](docs/cli.md) 与 [docs/output-maintenance.md](docs/output-maintenance.md)。

## 台湾 CWA 原生数值雷达

`tw` 的 `grid` 产品直接读取 CWA `O-A0059-001.json` 中的原始 dBZ 网格，可在允许公网访问后运行：

```bash
RADIUST_RUNTIME__ALLOW_NETWORK=true uv run radiust download tw --product grid --latest --output ./data --json
```

该产品输出 921×881 的 TWD67（EPSG:3821）原生经纬网格；`-99` 为无效数据、`-999` 为观测范围外或经质控移除的资料，两者均保存为 NaN 并通过 `quality` 区分。使用 `--grid geographic` 时需注意目标 EPSG:4326 与原生 TWD67 的基准面转换。默认的 `tw` `observation` 产品仍保留 3600×3600 PNG 供原始资料获取，其像素地理定位和物理色标尚未验证，不支持直接科学解码；`tw-http` 的图像也需独立验证。实测证据见 [validation-results/live-cli-tw-grid.json](validation-results/live-cli-tw-grid.json)。

CWA 官方图像产品编号、提供者声明的 PNG 范围/尺寸，以及数值网格的独立坐标系说明见 [docs/cwa-radar-products.md](docs/cwa-radar-products.md)。

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
    field = client.fetch(query)       # 不写正式 output
    report = client.download(query, output="./data", raw=True)
```

顶层 `fetch`、`afetch`、`fetch_many`、`iter_fetch`、`download` 和对应异步入口也可用。`fetch` 返回已经独立加载的 `RadarField`/`RadarDataset`；`download` 才进入编码和正式提交阶段。公共对象和错误类型见 [docs/python-sdk.md](docs/python-sdk.md)。

## 安装和可选能力

核心安装包含 NumPy、xarray、pyproj、Pillow、h5netcdf、h5py、Click 和 PyYAML，并默认支持 NetCDF。可按需要安装：

```bash
pip install radiust
pip install "radiust[geotiff]"
pip install "radiust[zarr]"
pip install "radiust[playwright]"
pip install "radiust[recovery,scraping]"
```

`radiust[storage]` 当前是有意保留的空 extra；标准 wheel 已随 Rust OpenDAL 编入 OSS/S3 远端提交能力。使用 `--output s3://...` 或 `oss://...` 时，配置 `storage.endpoint/region/access_key/secret_key/anonymous`，远端采用不可变 generation 和 manifest-last。AWS S3、S3-compatible 与 Aliyun OSS 的真实 provider 矩阵仍需单独验证，不能用 loopback 测试替代。完整的 CPython/Rust wheel 说明见 [docs/installation.md](docs/installation.md)。

## 开发和验证

```bash
.venv/bin/ruff check python tests
cargo fmt --all -- --check
cargo test --workspace
.venv/bin/pytest -q
```

默认测试禁止公网。live/provider/真实终端矩阵需要单独的凭据、环境和验收记录，不会因为 fixture 测试通过而自动标记完成。迁移状态和待补证据见 [docs/migration.md](docs/migration.md) 及 `migration/blockers/`。
