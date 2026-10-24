# Implementation Plan: 位置驱动的雷达发现与局部投影预览

**Branch**: `main`（实际 Git 分支；逻辑 feature 为 `006-location-radar-view`） | **Date**: 2026-10-10 | **Spec**: [spec.md](spec.md)

**Input**: `specs/006-location-radar-view/spec.md`；用户要求以旧 `radar-icu` 参考投影和站表，使用两个子代理只读收集。

**Status**: 已实施；[53 项任务清单](tasks.md)及 SC-001–008 核对完成。Core/CLI/Python SDK 局部视图、投影、站表空间索引和 PT 网页显示几何均已实现。2026-10-17 固定输入三入口回放及 T027/T036 通过；2026-10-24 安装后裸 `cat` 为个人用途 `local_view` 选择并渲染 RainViewer 当前 dBZ 帧，报告显示署名链接。测试点观测覆盖仍 unknown，其他地区未据此验收。

## Summary

裸 `cat` 读取用户位置，以有限候选发现一个适用的最新产品，获取并转换成以用户为中心、距离可读的局部图。设计重点是 WGS84 椭球 AEQD 的逐像元反查，以及区分实体站址、查询选择器、产品域和真实观测覆盖的站表。

[投影设计](projection.md)说明网格与采样，[站表设计](station-catalog.md)说明空间元信息与候选流程，[整理附件](station-inventory.md)列出 48 个 RDCAP 站、47 条 SIDARMA 历史站址、10 个区域/产品选择器和 26 个产品条目。它们不构成可用覆盖声明。

按用户提供的 Tropical Globe 页面取得 1,867 条完整 CSV，已完成现有 95 条站址候选的[交叉核对](tropicalglobe-crosscheck.md)：88 条有 5 km 内候选，7 条没有；共址、迁站与名称冲突保留待核，未修改生产坐标。

[旧项目对照](legacy-reference.md)固定 `caiyun-radar-icu@54972deb`：其投影为 Mercator 裁剪/缩放，704 项目录混合产品与裁剪框；当前 SIDARMA 47 项逐字段一致。设计吸收源像元边缘/中心注册和方向证据，保留 bbox 变体及旧 skip，不复制旧流程。PT overlay 的像元 CRS 另有潜在冲突，先通过独立几何 gate。

## Technical Context

**Language/Version**: 复用 Rust 1.92 / edition 2024，Clap 原生 CLI、PyO3 0.29 与 Python 薄 SDK。

**Primary Dependencies**: 复用 Tokio、serde、image、既有调度/limits；`geographiclib-rs` 0.2.7（`default-features=false`）提供椭球测地线正反解。不增加生产 PROJ/SQLite 站点库/Python 投影流水线。

**Storage**: 复用 catalog 与来源资源、已有 raw cache/receipt。空间索引是派生只读视图；不新增数据服务或长期位置日志。正式 projected writer 不在本轮。

**Testing**: 离线独立坐标参考、数值/raw 采样、站表/筛选、CLI/SDK 合同和预算/取消回放已运行。安装后 wheel 的禁网 smoke、RainViewer 显式 live 视图、三入口同帧回放及 2026-10-24 个人用途默认裸 `cat` 当前链路均通过；完整记录见 `validation-results/006-local-view-offline.json`、`validation-results/006-local-view-live.json` 和 `validation-results/006-local-view-packaging.json`。该 live 准入限 `local_view` 个人用途，覆盖仍按证据声明为 unknown。

**Target Platform**: 沿用现有 CLI/Core/SDK 构建支持；不新增 Desktop 平台承诺。

**Project Type**: 共享 Rust Core + CLI + 薄 SDK。

**Performance Goals**: 输出 N 个像元的逐点映射与采样，查轴沿用已有实现；每行/块临时坐标，不保存完整二维映射。默认 50 km/1 km 为 101×101；源数据分辨率单列，不承诺凭重采样达到 1 km 真实分辨率。

**Constraints**: `0<radius≤500 km`、单产品、latest、最多 8 个目标、一个 `runtime.discovery_deadline` 与共享 operation/budget，保留网络 opt-in。覆盖、几何、数值和在线/权限证据各自判断。

**Scale/Scope**: 全部现有材料及旧站表对照，首先完成 RDCAP/RainViewer 数值分支，再验证 PT 原图注册/当前链路。默认真实启用须逐产品准入；TW datum、SG 源球面投影、旧 704 项或外部全球站批量导入不在首轮转换范围。

## Constitution Check

[constitution](../../.specify/memory/constitution.md)仍为占位。设计依据为 [AGENTS.md](../../AGENTS.md)、[PHILOSOPHY](../../PHILOSOPHY.md)、用户确认的位置流程及既有契约，不从模板推导审批或示例测试要求。

| Gate | Phase 0 前 | Phase 1 后 |
| --- | --- | --- |
| 本地流程、复用 Engine | 满足方向 | 满足：内部目标调度/fetch/decode/CPU/取消共用 |
| 几何与数值能力分开 | 必须明确 | 满足：数值与 georeferenced raw 分支，PT 注册待核、unknown 拒绝投影 |
| 不以站距/外框伪造覆盖 | 现有站表有风险 | 满足：产品域/coverage/missing 分开，unknown 只可有界探测 |
| 网络、凭据与 TLS 条件 | 继承当前 opt-in | 满足：只用已配置许可，不自动开 insecure_tls |
| 时间、身份与质量保留 | 继承已有契约 | 满足：源时间与 raw identity 不变，派生参数版本化 |
| 不扩大历史验收 | 001–005 gate 独立 | 满足：目录整理/006 设计不关闭旧规格或 live 状态 |
| 范围有界且不复制流水线 | 需明确 | 满足：latest 单产品，8 候选，同一期限/预算，循环可取消，复用已有资源镜像 |

“满足”只表示设计符合约束；产品和来源尚未验收。

## Project Structure

### Documentation

```text
specs/006-location-radar-view/
├── spec.md
├── plan.md
├── research.md
├── projection.md
├── station-catalog.md
├── station-inventory.md
├── tropicalglobe-crosscheck.md
├── legacy-reference.md
├── data-model.md
├── quickstart.md
├── tasks.md
├── contracts/cli-sdk.md
└── checklists/requirements.md
```

### 涉及的现有代码与材料

| 范围 | 复用或修改位置 | 原因/兼容边界 |
| --- | --- | --- |
| 配置/模型 | `crates/radiust-core/src/config.rs`, `model.rs` | 同步严格 key/serde/Default/校验/origins；新增 optional location 与 typed local request/result；旧 Query 空 source 含义不变 |
| 源空间目录 | `crates/radiust-core/src/source/catalog.rs`、`id_sidarma.rs`、`id.rs`、`pt.rs` 及既有资源 | metadata.spatial 和内部库存只读视图；PT 通过 gate 后最小 include_str loader；不改 expand_targets(all) |
| 位置流程 | `crates/radiust-core/src/local_view.rs`、现有 `engine.rs` | 共用目标调度、访问检查、来源获取、期限和 CPU permit |
| 坐标/采样 | `crates/radiust-core/src/projection.rs`、现有 `grid.rs` | 椭球 AEQD 二维映射；提取查轴/采样，区分边缘/中心注册和域外质量，保留老路径 |
| 原图定位/预算 | `crates/radiust-core/src/preview.rs`、`raster.rs`、`limits.rs` 与来源资源 | geometry/data_window/shape 绑定同次 receipt；复用 RasterMemoryBudget/lease 和 CPU permit，不猜图像坐标 |
| 原生解码接点 | `crates/radiust-core/src/science.rs`、`science/rdcap.rs` 及既有图像读入 | 复用原 decoder；在已检查 shape 的分配点传入同一 operation raster budget，避免分阶段重新创建计数器 |
| CLI | `crates/radiust-cli/src/lib.rs`、`report.rs`、`human.rs`、`terminal.rs` | 裸 cat 新路由；现有 source/file 非 local 语义保留；renderer 控制尺寸不改变网格 |
| SDK/exports | `crates/radiust-core/src/python_types.rs`、`python.rs`、`crates/radiust-python/src/`、`python/radiust/rust_client.py`、`client.py`、`_bridge.py`、`models.py`、`__init__.py` | typed PyEngine awaitable 与现有注册；同步/异步委托一份 Core 结果，保留 operation gate/取消；xarray 明确转换 |
| 目录/打包 | `python/radiust/resources/`、`crates/radiust-core/resources/builtin/`、`scripts/release/prepare.py` | 已有同步流程识别 include_str；保持 canonical/mirror 字节一致、旧目录 schema 与规则身份 |
| 回归/证据 | `crates/radiust-core/tests/`、`crates/radiust-cli/tests/`、`tests/contract/`、`tests/integration/` | 独立投影参考、目标筛选、旧 CLI、质量/取消、安装后 wheel 检查 |
| 活动文档 | `docs/cli.md`、`docs/python-sdk.md`、`docs/architecture.md`、`DATA_SOURCES.md` | 实施后更新实际能力、修订后几何与来源证据；若改 README 则同步中英文 |

**Structure Decision**: 使用现有 SourceCatalog/metadata 和内部数据加载位置，不新增全球 StationRegistry。新空间索引仅组织现有来源材料；投影与获取全部由 Rust Core 实施，CLI/SDK 只负责输入与展示。

## Phase 0 — 已完成设计研究

[research.md](research.md)已说明 AEQD、测地线依赖、二维反查、源 CRS/注册准入、站表语义、入口兼容及 Tropical Globe 外部目录核对决定；R09 汇总两个子代理的旧项目证据，R10 核对当前 Engine/配置/资源/PyO3 接点，R11 明确 PT 注册的补证路径。技术选择已确定；来源证据缺口有明确检查和拒绝规则，保留实施 gate。

## Phase 1 — 已生成设计

1. [data-model](data-model.md)：请求、目录/候选和局部结果。
2. [projection](projection.md)：中心、半径、网格、源反查与质量。
3. [station-catalog](station-catalog.md)、[整理附件](station-inventory.md)、[外部交叉核对](tropicalglobe-crosscheck.md)与[旧项目参考](legacy-reference.md)：材料清单、逐字段对照、差异及证据。
4. [CLI/SDK contract](contracts/cli-sdk.md)：裸 cat、显式 local、旧行为和报告。
5. [quickstart](quickstart.md)：当前离线实现、安装包 smoke 与尚开放的 live 验证流程。

### 后续实施顺序与 gate

| 顺序 | 交付边界 | 进入下一步前的证据 |
| --- | --- | --- |
| 1 | Core 请求/配置预检、椭球 AEQD、逐点采样与结果 | SC-002/004；默认 101×101；非对称中心/边缘注册、quality、checked shape 与取消/预算；旧 regrid 回归 |
| 2 | metadata/既有库存的空间索引、固定排序与最多八目标的调度 | SC-003、FR-004–008；所有材料有类型/出处/unknown；源约束不越界，目录刷新只一次，deadline 不重置；缺测不触发回退 |
| 3 | 裸 cat/显式 local 与 typed PyO3、同步/异步薄 SDK | SC-001/005/006/007；旧 source/file 路由保留，同一结果/错误/时间/质量，取消后无迟到发布 |
| 4 | PT 原图注册验证及最小资源读取/投影分支 | R11 官方实际 CRS、窗口/内部控制点/shape/receipt gate；证据不够则 unavailable，不新增科学解码 |
| 5 | 至少一个适用真实产品完成标准入口默认准入 | SC-008；正常访问、当前帧、来源/用途、覆盖或其未知状态、独立几何控制点与投影记录；分别更新来源/能力/验收材料 |

五步实现与验收均已完成：离线 Core/CLI/SDK 与站表索引、PT 网页显示几何受限分支、独立投影参考和目录检查、T027/T036 三入口综合 gate，以及个人用途 RainViewer 默认产品的当前链路（SC-008）。SC-001–008 的结果与边界汇总见[验证清单](quickstart.md)和[53 项任务清单](tasks.md)。RainViewer 观测覆盖仍 unknown；未知地区、商用/高流量集成与数据再分发不因本次准入而扩大。

## 影响与验收边界

- 旧显示/gray/dBZ 值、原始缓存和旧文件不因局部中心变更而重写；派生局部图用独立处理 identity。
- 现有 Grid 为一维 x/y，可容纳目标 AEQD 规则网格；坐标映射必须二维逐点，不能误用现有分轴变换。
- 现有 native science 只分派 RainViewer/TW grid/RDCAP；PT/SG 的资源能力声明不等于通用 decoder 已实现。
- PT 网页显示几何已有 2026-10-10 官方脚本快照与 receipt 绑定实现；独立像素 tie points 缺失，不凭网页 overlay 几何声称物理覆盖或定量能力。
- 第一阶段到 cat/内存结果为止，不让未支持局部 CRS 的正式 writer 静默接受新网格。
- 入场产品的覆盖、授权和 live 证据仍需补齐；首版可开放范围按实际通过产品决定，不能预设整个台湾、日本或菲律宾已覆盖。
