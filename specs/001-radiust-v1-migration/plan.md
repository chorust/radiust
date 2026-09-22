# Implementation Plan: radiust v1 独立雷达数据获取与全量迁移

**Branch**: `main` | **Date**: 2026-09-16 | **Spec**: [spec.md](spec.md)

**Input**: `specs/001-radiust-v1-migration/spec.md`；技术约束来自 [原始计划](../../plan.md)。

**Status**: Phase 0 研究与 Phase 1 设计完成，可生成实施任务。本文描述待实现工作，不声明产品、兼容矩阵或迁移验收已经完成。

Spec Kit setup 的 `BRANCH=001-radiust-v1-migration` 来自 feature.json 的目录名兜底；实际 Git 分支经检查为 `main`，本次未创建或切换分支。功能目录与 Git 分支互相独立。

## Summary

建立独立的 Python SDK/CLI，以 Rust 提供有界网络 I/O、原始缓存、临时资源、无损瓦片操作及 OSS/S3 传输，Python 提供来源发现、科学解码、xarray 模型、显式网格转换、文件编码与终端渲染。采用 PyO3+maturin mixed package；默认输出 h5netcdf NetCDF4，原生网格优先，所有正式输出以完整清单最后发布。

完整交付边界是旧项目全部来源迁移，不是单个框架 demo。初始盘点确认 19 个 scraper 实现文件、5 个 radar tile 实现，另保留 2 条历史实现线索；多路径不能简单计作独立来源数。先按 M0–M4 验证代表来源和可靠性，再关闭每个 inventory 条目。现有仓库仅有 VERSION/__version__ 包骨架与 setuptools 配置，没有可复用的完成流程。

## Technical Context

**Language/Version**: 保留 Python >=3.10，首版声明验证范围 CPython 3.10–3.13、常规 GIL；Rust edition 2024，工具链不低于 1.85 且满足所选 crates 的最高 MSRV。候选 PyO3/pyo3-async-runtimes 0.29 同 minor、maturin 1.15，M1 锁定精确版本。先按 CPython minor 构建，不启用 abi3。详见 [研究 R01/R02](research.md)。

**Primary Dependencies**: Rust：Tokio、reqwest/rustls、suppaftp、OpenDAL(S3/OSS)、SQLite 支持、PNG/image 无损解码、SHA-256/serde。Python 核心：NumPy、xarray 2025.6.1 兼容线、pyproj、Pillow、h5netcdf+h5py、Click、PyYAML；类型以 dataclass/枚举和显式校验实现。具体兼容候选范围与锁定规则见 research R03，不宣称本次已经联合解析。可选 geotiff=rasterio 1.4 线、zarr=2.18.3/numcodecs 0.13 线、playwright、recovery（SciPy/OpenCV headless）、scraping（BeautifulSoup/brotli/dateutil 等按盘点需要）。all 是这些 extras 并集；storage 为空兼容 extra，Rust OSS/S3 已随标准 wheel 构建。

**Storage**: 本地输出、Aliyun OSS、S3-compatible；远端 immutable generation + manifest-last；本地同文件系统 staging+rename+root lock。缓存 objects/mosaics/tmp+SQLite index，独立 lease 与 per-key lock；不依赖 Mongo/Kafka/Apollo/外部监控。

**Testing**: pytest、pytest-asyncio、离线真实 fixture、属性/golden-vector 测试、伪终端和存储故障注入；cargo test 检验 Rust transport/cache/tiles；基于 wheel 的最小安装/extra矩阵。live 和 provider 测试显式标记，默认禁止公网（允许本地重放服务）。真实终端目视验收单列。

**Target Platform**: Linux x86_64/aarch64、macOS arm64/x86_64；初始候选 wheel 目标 manylinux_2_28 与 macOS 11+。M1 证明实际 wheel 标签、依赖和系统下限，未通过不发布相应支持声明。Windows/musl/PyPy/free-threaded 不进入本次首版支持承诺，新增需单独验证。

**Project Type**: Python library + CLI，私有 Rust 扩展；没有 Web UI、HTTP API 或常驻服务。

**Performance Goals**: 对六类代表源分别记录冷/暖缓存吞吐、峰值 RSS、请求数、临时盘峰值；旧链可运行时做同条件基线比较。不给未经测量的加速比例。流式结果有界，不随输入帧总数累积全部数组；终端探测总预算 200ms。

**Constraints**: 默认并行帧 2、全局请求 16、单 host 4、decode worker 2，来源更严格限额优先；重试默认最多 3 次尝试，只有单一预算。初始大小、像素、超时与临时盘保护阈值见 research R10，须由真实样本验证，超限明确报错。quality、时间绑定、native grid、raw/output 所有权、单写者和 commit fence 为强约束。认证值不进入身份、日志、清单。

**Scale/Scope**: [source-inventory.md](source-inventory.md) 的全部 HEAD 实现与历史线索均有去向；每个非例外来源至少一个真实原始样本和统一 contract。支持 4 种编码、3 类目标存储、4 种终端模式。HTTP 服务、分布式队列、旧生产集成兼容、动画/watch、远端多写者/恰好一次、共享 Zarr append 均排除。

## Constitution Check

*GATE：研究前与设计后分别检查。*

`.specify/memory/constitution.md` 仍含项目/原则/版本占位符，未构成已批准宪章。不能将示例原则当强制规则，也不能宣称完整通过一份尚不存在的宪章。本次记录为“无可评估的已批准宪章条款”；用户授权的规划继续，后续若正式制定宪章须再次核对。

以下检查来源于已批准工作输入 spec/原始计划，不冒称宪章规则：

| 约束 | Phase 0 前 | Phase 1 后证据 |
| --- | --- | --- |
| 全量迁移，含注释/未注册来源 | PASS，范围明确 | PASS，24 条 HEAD 路径+2历史线索均在 inventory，M4不以demo替代 |
| Rust I/O / Python科学分层 | PASS | PASS，单向 facade、Source无全局客户端、decoder不联网 |
| 原生网格、真实物理值、未知色不当无雨 | PASS | PASS，data-model、encoding contract及科学验收矩阵 |
| cache 可删、raw/output 正式保存 | PASS | PASS，storage所有权、lease与root隔离 |
| manifest-last 与单写者 | PASS | PASS，local/remote提交状态机、commit fence及响应丢失核对 |
| 最小安装+extras、无旧生产依赖 | PASS | PASS，maturin wheel/核心NetCDF、懒加载与extra矩阵 |
| 默认离线回归与真实来源证据 | PASS | PASS，quickstart明确真实raw前置、offline replay与live分离 |
| 不伪称能力已验证 | PASS | PASS，候选版本/平台/来源状态分别列实施门槛 |

无未解释的设计违规，无阻塞用户澄清。进入实施前必须执行相应构建与样本验证；门槛失败修复或记录明确阻塞，不静默缩减原始需求。

## Project Structure

### Documentation (this feature)

```text
specs/001-radiust-v1-migration/
├── spec.md
├── plan.md
├── research.md
├── source-inventory.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── python-sdk.md
│   ├── cli.md
│   ├── storage.md
│   └── encoding.md
└── checklists/
    └── requirements.md
```

`tasks.md` 由下一阶段 `$speckit-tasks` 生成，本次不创建。根目录 plan.md 保留为原始设计输入，不能被本文件覆盖。

### Source Code (repository root, planned)

```text
Cargo.toml
Cargo.lock / rust-toolchain.toml
crates/radiust-core/
  Cargo.toml
  src/
    lib.rs / python.rs / runtime.rs / limits.rs
    transport/{mod.rs,http.rs,ftp.rs}
    cache/{mod.rs,index.rs,lease.rs}
    tiles/{mod.rs,palette.rs,mosaic.rs,crop.rs}
    storage/{mod.rs,local.rs,object.rs}
    temp.rs / digest.rs / errors.rs
  tests/
python/radiust/
  __init__.py / api.py / client.py / pipeline.py
  models.py / query.py / errors.py / config.py / registry.py
  _core.*                       # maturin 编译扩展
  sources/                      # 全部内置来源，静态描述与实现懒加载
  resources/                    # stations/products/palettes及版本
  decoders/                     # exact/nearest/recovery
  grids/                        # 几何、校验与显式重网格
  outputs/                      # encoders和科学元数据映射
  storage/                      # 输出组状态机及manifest
  rendering/                    # 共享RGBA/legend/title
  terminal/                     # capability/kitty/iterm2/ansi/text
  cli/                          # Click命令与报告映射
  py.typed
config/                         # 示例配置，无凭据
constraints/                    # 各Python版本与extra的已验证锁定输入
migration/                      # 机器可校验inventory、例外、迁移说明
scripts/validation/             # 离线重放、故障注入、基准和发布指南入口
tests/
  fixtures/{sources,protocols,scientific}/
  unit/ / contract/ / sources/ / integration/
  packaging/ / terminal/ / live/
.github/workflows/              # offline、wheel、显式live/provider作业
pyproject.toml
```

**Structure Decision**: 沿用现有 Cargo workspace 与 python/radiust，增加领域模块而非重建多仓库。扩展名与 Python 私有 facade 固定；Rust 不导入 xarray。CLI → SDK/Client → pipeline → Source + Rust transport；Field → regrid/encoder/render；encoder → staged group → storage commit。只有 storage 编排发布正式 manifest，decoder 与 source 不各自提交。

## Phase 0 — Research outcomes

[research.md](research.md) 已记录 R01–R10 的 Decision/Rationale/Alternatives 与官方/本地证据，三个 luna_worker 完成独立信息收集后统一审阅：

- mixed maturin build，匹配的 PyO3 async bridge，Tokio/reqwest、suppaftp 与 OpenDAL；显式 cancellation/lease。
- h5netcdf NetCDF4、CF-1.8、Rasterio 1.4 兼容线、Zarr v2，每个 encoder 固定语义与版本。
- 19 scraper+5 tile 的初始路径清单，两个历史巴西来源线索；未把旧 output 认作可信 fixture，也未凭示例新增 JMA。
- Python 唯一 canonical identity；raw 完整性不改变 decoded 身份，补 raw 必须同 revision。
- commit fence 与未知提交结果核对，终端能力保守探测，资源保护默认与测量策略。

无设计层未决澄清。所有“尚未实测”事项都附到以下交付门槛，不把它们变成虚假的研究结论。

## Phase 1 — Design & contracts

- [data-model.md](data-model.md)：Query/FrameRef/receipt、科学对象、Grid、质量位、输出清单、结果、缓存与迁移状态。
- [Python SDK](contracts/python-sdk.md)：公共同步/异步入口、Client 生命周期、批量/流式结果、Source开发者协议、错误族。
- [CLI](contracts/cli.md)：命令参数、互斥/默认值、报告schema、退出码、配置和终端契约。
- [Storage](contracts/storage.md)：identity、命名、raw补存、本地/远端提交、缓存lease、失败/取消恢复。
- [Encoding](contracts/encoding.md)：各格式能力、quality/CRS/时间/provenance、round-trip规则。
- [quickstart.md](quickstart.md)：从未来实现的wheel/真实样本出发的可运行验证步骤与预期，不包含业务实现代码。

设计后 gate 结果见上表。文档验证仅证明规划结构与覆盖，不替代应用测试。

## Delivery sequence and release gates

| 阶段 | 实施内容与依赖 | 出口证据 | 对应场景 |
| --- | --- | --- | --- |
| M0 | 冻结旧commit；逐路径产品/站点/认证/时次/网格盘点；真实raw与使用依据；历史路径去向 | inventory无未解释项，至少首批代表具备可追溯样本；其余缺样本项有负责人/阻塞记录，未冒称完成 | US6 |
| M1 | 依赖解析与mixed wheel；身份/Query/Grid/quality；Client/HTTP/基础缓存与lease；my代表；本地NetCDF及最小manifest；list/discover/download、cat ANSI/text、JSON错误 | 从wheel完成离线真实fixture获取与导出读回；sync/async相等；时间/颜色/资源/取消基础契约；最小安装 | US1、US2、US3、US4基础 |
| M2 | id_sidarma/rainviewer/au/ph/fr代表；tw S3 acquisition；tile无损/FTP/浏览器/recovery；显式regrid；Kitty/iTerm2 | 每类真实样本与缺tile/错palette/竞态/临时清理测试；每种终端真实验收 | US2、US4、US6代表 |
| M3 | GeoTIFF/PNG/Zarr完整编码；OSS/S3；full raw/raw-only/overwrite/冲突状态机；批量流式；缓存修复/GC竞争；配置doctor和provider认证 | 所有存储/取消故障矩阵、格式读回、batch失败不丢、资源上限；真实provider证据 | US3、US5 |
| M4 | 各家族余下所有实现迁移；迁移说明/例外；wheel及文档；六类基准与代表live smoke | 每个非例外源至少1真实raw contract通过；inventory闭合，例外接受；再进入旧库归档决策 | US6及全部SC |

M0 样本收集可持续贯穿后续，但缺首个代表真实样本不能宣称该代表验收完成。M1 已包含最小可靠本地 manifest/清理，不能先写无完成标记的下载再等 M3 修补。M2 的 tw acquisition 需要复用 Rust 对象读取能力，不等 M3 正式远端输出。数据模型与身份测试先于来源批量迁移。

任务生成可将独立 encoder/terminal/provider/source family 分组并行，但公共模型、身份、context预算和提交契约必须先固定；同一文件写入避免并行冲突。每个来源至少形成 inventory、raw fixture、source adapter、contract、migration note 的闭环，不只创建占位类。

## Requirement coverage and validation mapping

| 要求 | 设计所有者 | 阶段与验证 |
| --- | --- | --- |
| FR-001、FR-002 | registry/resources/SDK/CLI | M1离线list、M4全目录 |
| FR-003、FR-004 | Query/Source discovery | M1精确/范围/时区/歧义，M2竞态 |
| FR-005、FR-006、FR-007 | identity/receipt/runtime | M1golden vectors、M2时次绑定和所有权 |
| FR-008、FR-009、FR-010 | models/decoders/resources | M1科学fixture、M2所有解码家族 |
| FR-011、FR-012、FR-013 | Rust tiles/Python grids | M2palette无损、几何与显式regrid |
| FR-014、FR-015 | SDK/Client/pipeline | M1sync/async、M3batch/stream/cancel |
| FR-016、FR-017 | outputs/rendering | M1NetCDF、M3完整格式矩阵 |
| FR-018、FR-019、FR-020、FR-021、FR-022 | storage/manifest | M1本地最小提交、M3全部目标与raw模式 |
| FR-023、FR-024 | Rust cache/leases | M1基础缓存、M3损坏/竞争/GC |
| FR-025 | config | M1默认/错误、M3凭据/来源/覆盖 |
| FR-026、FR-027 | Client/Rust runtime/commit fence | M1基础取消、M2重型限制、M3完整故障注入 |
| FR-028、FR-029、FR-030 | CLI/errors/logging | M1报告退出码、M3全部维护与脱敏 |
| FR-031、FR-032、FR-033 | rendering/terminal | M1ANSI/text、M2图片协议/TTY |
| FR-034 | packaging/extras | M1每矩阵wheel、M3extra、M4发布验证 |
| FR-035、FR-036、FR-037 | inventory/sources/migration | M0盘点、M2代表、M4全源/例外 |

SC-001/002 对应完整inventory与真实样本；SC-003/004 对应时间和科学一致性；SC-005 对应存储故障矩阵；SC-006/007 对应批量与缓存；SC-008/009 对应终端和安装；SC-010 对应配置与报告；SC-011 对应记录基准。每份验收证据必须包含命令、依赖版本、输入摘要、预期/实际与状态，不以“未运行”计为通过。

## Complexity Tracking

没有已批准宪章条款的违规需要豁免，因此无违规表。保留的复杂度来自用户明确需求：跨语言I/O、多个协议、科学质量/网格、正式raw、远端提交与全来源迁移；均有独立契约和阶段门槛。未引入服务端、队列、插件拆包或分布式写锁。

## Remaining implementation risks

- Python 3.10 与科学依赖兼容线需要 M1 真正解析并构建，候选范围不能代替锁文件；二进制extra不满足矩阵时明确该extra支持边界，不破坏核心。
- 来源停运/认证/时次绑定与真实raw不足必须逐项记录；未能证明物理语义的产物不能当合格科学数据发布。
- `fr/pt` luminance 与旧 fuzzy/recovery 可能含历史错误，不能逐字节保留为 canonical；参考像素与迁移说明必须先于替换结论。
- 远端提交响应丢失、CPU不可中断、浏览器下载需遵守统一所有权和commit fence；测试只证明其覆盖场景，不扩大为跨机器恰好一次承诺。
- 宪章仍未制定；若后续正式制定，重新执行 Constitution Check，不回填假定批准日期。
