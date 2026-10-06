# Radiust

[简体中文](README.md) · [English](README.en.md)

**让普通人能够方便地获取、理解和使用公共气象雷达观测。**

*Public radar, directly to you.*（候选品牌标语）

Radiust 希望让用户通过自己设备上的开放工具连接雷达来源，优先采用官方来源，也支持可靠、可追溯的第三方产品。数据默认在本地获取、缓存和处理，核心观测功能无需 Radiust 账号或 Radiust 数据中转服务。

当前提供 CLI、Python SDK 和可复用的 Rust `radiust-core`；Python console command 和 SDK 共用 Rust Engine。面向普通公众的 **Desktop 尚未实现**，其查看、时间序列播放、多源默认选择、手动切换与故障回退属于后续路线。当前先做好观测，为未来预测模型保留数据接口；跨来源雷达拼图留待后续。

[项目理念](PHILOSOPHY.md) · [发展路线与 Desktop 验收目标](ROADMAP.md) · [数据来源政策](DATA_SOURCES.md) · [品牌使用规则](TRADEMARKS.md)

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
radiust cat --file tests/fixtures/sources/th/raw/kkn240Loop.gif --raw --renderer text
radiust cat --file tests/fixtures/gray-dbz/local/225-codes.png --gray --renderer text
radiust cat --file tests/fixtures/gray-dbz/local/225-codes.png --dbz --renderer text
radiust discover all --json
```

`--raw` shows source pixels, `--gray` declares a gray-code display, and `--dbz` strictly decodes a declared local gray image. The local rule accepts visible integer codes 0–224 and computes `dBZ = gray × 5/16`; transparent pixels are missing, opaque black is valid zero, and time/geolocation remain unknown. No mode is inferred from appearance. The old source-only `--legacy-display` flag remains as a compatibility alias for `--gray`.

本地多帧 GIF/WebP 必须显式选帧；来源流程先保留原始 artifact，再查看规则生成的 gray，只有通过来源证据的路径才能请求 dBZ：

```bash
radiust cat --file ./frames.gif --raw --frame-index 0
radiust cat --file ./frames.gif --dbz --frame-index 0
radiust cat nz --product rain --latest --raw
radiust cat nz --product rain --latest --gray
radiust cat nz --product rain --latest --dbz
```

数值文件可用 `radiust cat --file ./reflectivity.nc --dbz --variable reflectivity` 检查；Python SDK 可用 `read_dbz()` 读取，再通过 `write()` 保存新数值成果，无需为本地文件伪造 `FrameRef`。来源 `download --dbz --raw` 将同次获取的原始 artifact 附在解码成果中；`--raw-only` 与 `--dbz` 冲突。省略模式仍走原 generic/native 行为，其他科学变量保留真实单位。JSON 报告沿用 Envelope v1 并增加 `mode_schema_version: 1` 与 `mode_info`。

Source gray evidence, dBZ conversion limits, numeric formats, and validation status are documented in [docs/gray-dbz.md](docs/gray-dbz.md); CLI and SDK examples are in [docs/cli.md](docs/cli.md) and [docs/python-sdk.md](docs/python-sdk.md).
`discover all` 在默认禁止联网的配置下仍会返回完整目录状态，通常以退出码 5 表示所有目标均未成功获取实时资料；该结果不表示已完成实时来源验收。

实际输出目录和 manifest 的命名规则见 [docs/cli.md](docs/cli.md) 与 [docs/output-maintenance.md](docs/output-maintenance.md)。

## 数据源支持情况

当前原生目录登记 **25 个来源**，涉及亚洲、欧洲、大洋洲和北美的多个国家／地区。实际可用性和支持能力因来源、产品与访问条件而异；完整清单、获取状态和授权条件见[数据来源政策](DATA_SOURCES.md)。

| 能力 | 已有验证的例子 | 使用边界 |
| --- | --- | --- |
| 数值反射率 | 台湾 CWA `tw/grid`、RainViewer | 支持有证据的 dBZ 解码，不代表所有来源均可定量解释 |
| 降雨强度类别 | 新加坡 `sg`、葡萄牙 `pt` | 提供有序类别，不提供精确逐像素 mm/h |
| 原图／原始资料 | 其他来源按各自状态开放 | 可用 `--raw-only` 保存原始文件；未经验证的颜色不能直接解释为反射率或雨强 |

`rdcap` 的台湾、日本、菲律宾在线能力仍标为未验证；`uk` 已退役，两条巴西来源属于历史迁移记录，未注册到原生目录。详情及[历史验证快照](DATA_SOURCES.md#历史获取与科学验证记录)集中记录在来源文档中。

适配器存在或下载成功不等于科学解码已验收，也不等于取得商用或再分发授权。逐源技术证据见[迁移记录](migration/sources/)与[验证材料](validation-results/)。

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

### macOS Apple Silicon 原生 CLI（预览版）

macOS arm64 用户可下载独立 Rust CLI `radiust` v0.1.0（支持 macOS 11 及更新版本），无需安装 Python 或 Rust。当前为预览版，适用范围和校验步骤见[完整安装说明](docs/installation.md#macos-apple-silicon-预编译版本预览)及 [GitHub Release](https://github.com/chorust/radiust/releases/tag/v0.1.0)。该 CLI 与下方的 Python 包是两种独立安装方式。

### Python 包

基础安装包含 Rust 扩展；YAML 配置由 Rust 解析，因此运行时不依赖 PyYAML。科学互操作等能力按需安装：

```bash
pip install radiust
pip install "radiust[geotiff]"
pip install "radiust[zarr]"
pip install "radiust[playwright]"
pip install "radiust[recovery,scraping]"
```

`radiust[storage]` 当前是有意保留的空 extra；标准 wheel 已随 Rust OpenDAL 编入 OSS/S3 远端提交能力。使用 `--output s3://...` 或 `oss://...` 时，配置 `storage.endpoint/region/access_key/secret_key/anonymous`，远端采用不可变 generation 和 manifest-last。AWS S3、S3-compatible 与 Aliyun OSS 的真实 provider 矩阵仍需单独验证，不能用 loopback 测试替代。完整的 CPython/Rust wheel 说明见 [docs/installation.md](docs/installation.md)。

## 开放使用与许可

软件采用标准 [Apache-2.0](LICENSE)，欢迎商业使用。项目持续为公众提供开放、可独立使用的工具；软件许可不替代上游雷达数据授权，也不构成对下游产品的背书。数据访问、署名、商用和再分发条件逐源记录，未确认项保留为未确认，见[数据来源政策](DATA_SOURCES.md)。名称与未来视觉标识的使用见[品牌规则](TRADEMARKS.md)。

## 开发和验证

```bash
.venv/bin/ruff check python tests
cargo fmt --all -- --check
cargo test --workspace
.venv/bin/pytest -q
```

默认测试禁止公网。live/provider/真实终端矩阵需要单独的凭据、环境和验收记录，不会因为 fixture 测试通过而自动标记完成。迁移状态和待补证据见 [docs/migration.md](docs/migration.md) 及 `migration/blockers/`。
