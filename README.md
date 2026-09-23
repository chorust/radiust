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
radiust cat --file tests/fixtures/sources/th_royalrain/raw/takhli.png --renderer text
radiust cat --file tests/fixtures/sources/th/raw/kkn240Loop.gif --renderer text
radiust discover all --json
```

PNG/GIF preview uses original image pixels (first GIF frame); no scientific decoding is performed.
`discover all` 在默认禁止联网的配置下仍会返回完整目录状态，通常以退出码 5 表示所有目标均未成功获取实时资料；该结果不表示已完成实时来源验收。

实际输出目录和 manifest 的命名规则见 [docs/cli.md](docs/cli.md) 与 [docs/output-maintenance.md](docs/output-maintenance.md)。

## 数据源支持情况

以下是**截至 2026-09-21 的仓库内验证记录**，涵盖全部 26 个已注册来源（含 2 个历史来源）。所有来源均有适配器和离线测试，但**适配器存在、离线 fixture 通过或能够下载图片，不代表已经支持可信的科学数值解码**。“在线原始获取”描述有记录的实测结果，不保证此后上游持续可用；“科学数据”仅指有证据支持的解码能力。

| 数据源 ID | 提供方 / 数据 | 在线原始获取 | 科学数据支持 / 当前限制 |
| --- | --- | --- | --- |
| `au` | 澳大利亚 BoM 雷达 | 已实测（FTP） | 原始资料；色标与原生几何待验证 |
| `bmkg` | 印尼 BMKG 瓦片 | 上游 HTTP 403 | 原始获取受阻；科学解码未验收 |
| `br_cptec` | 巴西 CPTEC WMS（历史来源） | 历史帧已实测；最新帧返回上游错误 | 原始图像；物理色标与几何待验证 |
| `br_sipam` | 巴西 SIPAM（历史来源） | 已实测 | 原始图像；物理色标与像素定位待验证 |
| `ca` | 加拿大 ECCC | 已实测 | 原始资料；色标与原生几何待验证 |
| `cam` | 柬埔寨雷达 | 已实测 | 原始图像；科学解码与几何待验证 |
| `es` | 西班牙 AEMET | 已实测 | 原始资料；色标与原生几何待验证 |
| `fr` | 法国 Météo-France WMS | 已实测 | 原始图像；颜色到 dBZ 的映射待验证 |
| `id` | 印尼 BMKG 雷达 | 需有效授权，未完成在线验收 | 原始获取受限；科学解码未验收 |
| `id_sidarma` | 印尼 SIDARMA CMAX | 需授权；现有探测返回 HTTP 403 | 原始获取受限；科学解码未验收 |
| `kr` | 韩国 KMA | 已实测 | 原始资料；色标与原生几何待验证 |
| `my` | 马来西亚气象局 | 已实测 | 原始图像；时次、色标、几何与再利用许可待验证 |
| `nz` | 新西兰 MetService | 已实测 | 原始资料；色标与原生几何待验证 |
| `opensnow` | OpenSnow 瓦片 | 上游 HTTP 403；需授权访问 | 原始获取受阻；科学解码未验收 |
| `ph` | 菲律宾 PAGASA | 需外部凭据 / 浏览器获取条件 | 在线原始获取与科学解码未验收 |
| `pt` | 葡萄牙 IPMA | 已实测 | **支持有序降雨强度类别**；不提供逐像素数值雨强 |
| `rainviewer` | RainViewer 瓦片 | 已实测 | **支持经提供方色表验证的 dBZ 解码** |
| `sg` | 新加坡 NEA | 已实测并对照官方 API | **支持有序降雨强度类别**；不提供定量 mm/h |
| `th` | 泰国 TMD | 已实测 | 原始 GIF；时次、色标与几何待验证 |
| `th_royalrain` | 泰国 Royal Rainmaking | 曾实测；近期请求失败 | 原始资料；色标与原生几何待验证 |
| `tw` | 台湾 CWA | 已实测 | **`grid` 支持原生 TWD67 数值 dBZ**；`observation` PNG 仅原始获取，色标与像素定位待验证 |
| `tw-http` | 台湾 CWA HTTP 图片 | 已实测 | 原始图像；色标与原生几何待验证 |
| `uk` | 英国 Met Office DataPoint | 上游已停运（保留例外） | 不支持在线获取 |
| `vn` | 越南 Hymetnet CMAX | 已实测 | 原始资料；色标与原生几何待验证 |
| `windy` | Windy 瓦片 | 已实测 | 原始瓦片；物理色标及时次绑定待验证 |
| `wunderground` | Weather Underground 瓦片 | 需有效 API key，未完成在线验收 | 原始获取受限；科学解码及时次绑定未验收 |

对于仅标注“原始资料 / 原始图像”的来源，可使用 `--raw-only` 保留原始文件；不要将图像颜色直接解释为物理反射率或雨强。逐来源迁移状态、限制和实测证据见 [migration/sources/](migration/sources/)、[docs/migration.md](docs/migration.md) 与 [validation-results/](validation-results/)；台湾 CWA 数值网格的技术细节仍保留在 [docs/cwa-radar-products.md](docs/cwa-radar-products.md)。

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
