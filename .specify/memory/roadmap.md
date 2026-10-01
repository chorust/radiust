<!--
SYNC IMPACT REPORT
==================
Version change: 1.0.0 → 1.1.0
Bump rationale: MINOR — 新增已写规格 004 及其范围、依赖、决策和在线验收证据缺口。

Changes this revision:
  - Added spec 004 — RDCAP 台湾、日本、菲律宾单站雷达支持 [specced].
  - Recorded C-13–C-15 from 004 spec/plan/research/contracts.
  - Recorded 004 capability/contract dependencies on 001–003; their complete
    acceptance is not a prerequisite for independent 004 implementation.
  - Annotated Q-08's historical no-004+ statement; added Q-09 for native HTTP
    acceptance evidence. Existing Q-01–Q-08 remain open.
  - Preserved the initial Sync Impact Report and 001–003 entries/statuses.

Specs affected: 004 (new roadmap entry); 001–003 retained without restatus.
Open questions added/resolved: added Q-09; Q-08 annotated; resolved none.

Notes: Evidence snapshot 2026-10-01. This amendment records the existing 004
artifacts; 0/58 implementation tasks are checked. Browser samples and research
replay do not establish native adapter or per-country live acceptance. No new
ADR/PRD, product test, network verification, implementation, release or archive
operation is part of this amendment. The earlier report below is preserved as
initialization history. The placeholder constitution is not ratified here.
-->

<!-- Historical initialization report — 1.0.0; retained verbatim. -->
<!--
SYNC IMPACT REPORT
==================
Version change: none → 1.0.0
Bump rationale: INITIAL — 将既有项目 artifacts 汇入首份项目级 roadmap。

Changes this revision:
  - Added spec 001 — radiust v1 独立雷达数据获取与全量迁移 [in-progress]
  - Added spec 002 — CLI 行为修复与终端体验优化 [in-progress]
  - Added spec 003 — Rust 核心迁移与性能优化 [in-progress]
  - Recorded evidence-backed dependencies: 002 depends on 001 capabilities;
    003 depends on 001 and 002 contracts/capabilities, not their complete acceptance.
  - Recorded C-01–C-12 and decision evolution without changing source artifacts.

Specs affected: 001, 002, 003 (roadmap entries only)
Open questions added/resolved: added Q-01–Q-08; resolved none

Notes: Evidence snapshot 2026-09-30. The user confirmed the harvested
content and initialization as version 1.0.0 before this project roadmap was
written. This ratifies the roadmap ledger only, not the placeholder constitution
or unresolved source contracts. The loader's initial roadmap_exists=true
referred to the extension's own example project; absolute project-path overrides
confirmed the real target was absent. Existing spec/plan/tasks/docs/constitution
are preserved. No product tests or external provider checks were run for this
documentation work. Statuses describe the current worktree and recorded evidence,
not a release, merge or deployment.
-->

# radiust — Spec Roadmap

本台账统一记录 radiust 的项目目标、范围、关键决策、依赖与生命周期。依据为截至 **2026-09-30** 的仓库 artifacts；不新增交付日期、未写规格或未经证实的依赖。已写规格的历史设计与后续迁移决定均保留出处。

**2026-10-01 增补（1.1.0）：** 下文 2026-09-30 快照及三项生命周期判断为初始化记录。此次登记已完成规格与设计、尚未实施的 004；项目台账现有四项，001–003 保持原状态与验收缺口。004 仅扩展近期三国单站支持；不重定义已有迁移、性能或发布门槛。

治理入口：[constitution](constitution.md) 目前仍为占位模板，不能当作已批准原则；未发现项目 `docs/adr/`、配置 globs 匹配的 PRD 或额外项目交接记忆。下列决定来自已有 spec/plan/研究和验收记录，不冒称正式 ADR 或新批准的宪章。

Status legend (lifecycle): **undecided** · **needs-info** · **planned** · **specced** · **in-progress** · **implemented** · **verified** · **deferred** · **abandoned**。

状态判定口径：spec/checklist 通过只证明规格就绪；实施任务及运行记录证明实施进展；必需范围仍有未关闭项时保留 `in-progress`。`implemented` 需要范围内实现闭合，`verified` 还需要对应验收证据。来源台账中的 `blocked`、任务勾选和 roadmap 生命周期分别解释，不将局部延期升级为整个 spec 的 `deferred`，也不以测试 skip 判定验收通过。三项生命周期为基于证据的判断；不确定的关系和契约归入 Open Questions。

---

## Vision & End States

- **独立的数据工具与完整迁移交接。** 从旧项目剥离雷达来源能力，提供统一目录、查询、获取、处理、预览和输出，不依赖旧业务数据库、队列或监控；全部旧实现路径，包括未启用/注释项及两条历史巴西线索，均有迁移去向或明确接受的例外。完整 v1 验收后才进入旧库归档决策。依据：[原始计划](../../plan.md)、[001 US1/US6、SC-001–SC-011](../../specs/001-radiust-v1-migration/spec.md)。
- **可追溯的科学数据与可靠保存。** 保留有效时间、物理量/单位、质量、原生网格与处理历史；原图或旧灰度显示通过不代表科学验证通过。正式 NetCDF4、GeoTIFF、PNG、Zarr v2 和 raw 输出以完整 manifest 为完成边界，支持重跑、修复、覆盖与取消；本地和远端能力分别验收。依据：[001 数据与输出契约](../../specs/001-radiust-v1-migration/contracts/encoding.md)、[storage](../../specs/001-radiust-v1-migration/contracts/storage.md)。
- **可理解、可自动化的 CLI。** 缺省 latest、原图单帧预览、逐目标全量状态、可读人类报告与兼容机器报告；legacy 显示按来源/产品证据启用，窄终端、管道、取消和错误仍保留完整信息。依据：[002 US1–US5](../../specs/002-cli-experience/spec.md)。
- **一个可复用的原生核心。** 无 Python 的 macOS arm64 CLI 和 Python 薄绑定共用 Rust Engine，科学 Python 转换按需进行；同机重复测量验证启动、发现、首帧与资源目标。可嵌入核心为未来桌面端留下边界，本轮不交付桌面应用。依据：[003 spec](../../specs/003-rust-core-performance/spec.md)、[根 migration.md](../../migration.md)。

## Constraints & Decisions

- **C-01 — 独立且有界的产品边界。** 一次运行是有界任务，周期运行交给外部调度。HTTP 服务、常驻调度器、分布式队列、全屏 TUI、watch/动画、桌面 UI/IPC/Swift 绑定未纳入现有 specs。旧生产 API、配置和业务集成不承诺兼容。依据：[001 Assumptions](../../specs/001-radiust-v1-migration/spec.md)、[002 Assumptions](../../specs/002-cli-experience/spec.md)、[003 FR-013](../../specs/003-rust-core-performance/spec.md)。
- **C-02 — 架构决策有明确演进。** 历史设计是 Python 科学/来源/编排 + Rust I/O（[根 plan](../../plan.md)、[001 plan](../../specs/001-radiust-v1-migration/plan.md)）；003 明确变更为 Rust 独占主要业务，CLI/PyO3 共用 Engine，Python 保留薄绑定及显式 xarray/Zarr 互操作（[003 core/SDK](../../specs/003-rust-core-performance/contracts/core-sdk.md)）。002 的 Python Click/worker 布局是迁移前设计，不作为当前模块归属要求；保留其行为与证据基线，不删除原设计。
- **C-03 — 范围计数区分历史迁移与原生目录。** 001 盘点为 24 条 HEAD 实现路径 + 2 条历史线索，不能由文件数推断逻辑来源数；003 冻结 24 个内置 ID、26 个发现目标，明确排除未登记的 `br_cptec`/`br_sipam` 于本轮原生覆盖率。历史来源仍在 001 交接范围，未来是否登记原生目录待确认（Q-03）。`jma` 是原始计划示例，未纳入已有迁移清单。依据：[001 inventory](../../specs/001-radiust-v1-migration/source-inventory.md)、[003 research §2](../../specs/003-rust-core-performance/research.md)。
- **C-04 — 时间与身份依赖事实。** 使用有时区 UTC、`[start,end)` 区间、精确 at、分组 latest 与单帧消歧；不能用获取时间代替有效时间。身份区分 logical/revision/processing/output，秘密、签名 URL、临时路径和并发设置不影响稳定身份。语义变化提升 decoder/resource/encoder 版本。依据：[001 data model](../../specs/001-radiust-v1-migration/data-model.md)、[003 persistence](../../specs/003-rust-core-performance/contracts/persistence.md)。
- **C-05 — 科学与显示证据分开。** 连续科学值/质量保持 `float32`/`uint16`，缺测、站外、未知颜色、恢复、插值、检测阈值和已知无雨可区分；native grid 优先，regrid 必须显式且有可靠几何，dBZ bilinear 在线性反射率域执行。合法 raw、时间绑定、palette/参考值、几何与独立读回缺一时，只声明已验证能力。质量位文档存在冲突，待确认 Q-05。依据：[001 FR-008–013](../../specs/001-radiust-v1-migration/spec.md)、[003 FR-004](../../specs/003-rust-core-performance/spec.md)。
- **C-06 — cache、临时资料与正式输出分别管理。** cache 可回收，raw 是正式用户数据；租约/所有权覆盖实际 worker 生命周期，取消的迟到结果不提交。正式成果以 SHA-256、完整身份及 manifest-last 验证，raw-only 不 decode/mosaic/regrid，补 raw 必须同 revision。旧 v1 manifest 和锁协议是兼容基线；新旧缓存默认隔离，同根并发不得未经证明开放。依据：[001 storage](../../specs/001-radiust-v1-migration/contracts/storage.md)、[003 persistence](../../specs/003-rust-core-performance/contracts/persistence.md)。
- **C-07 — 远端存储按 provider 验收。** 本地、AWS S3、至少一个 S3-compatible、Aliyun OSS 分别留证；远端 immutable generation + 最后 pointer/manifest、取消 fence、未知提交结果核对，v1 外部保证单 writer，不承诺跨机器 exactly-once。内存或 loopback MinIO 合同不代替真实 AWS/OSS 支持证明。用户已延期真实矩阵的记录不等于需求被删除。依据：[001 research R04/R09](../../specs/001-radiust-v1-migration/research.md)、[release-readiness](../../migration/release-readiness.md)、[003 T041](../../specs/003-rust-core-performance/tasks.md)。
- **C-08 — 网络和资源预算统一。** 默认禁公网，live/provider 验证显式 opt-in，不读取旧凭据；TLS、跳转主机限制、秘密与控制字符安全展示保持既有保护。请求/host/帧/CPU、字节、像素、临时盘分别有界，暂时性错误只有统一重试预算。默认请求/host/帧/decode 并发 16/4/2/2，003 另设发现 worker 4；全量发现共用 300 秒预算，超时/取消保留已完成和未开始状态。依据：[001 research R10](../../specs/001-radiust-v1-migration/research.md)、[002 CLI](../../specs/002-cli-experience/contracts/cli.md)、[003 CLI](../../specs/003-rust-core-performance/contracts/cli.md)。
- **C-09 — CLI 默认行为有明确变更。** 001 要求显式时间且 source cat 默认科学预览；002 FR-001/006 与 CLI contract 改为 CLI 缺省 latest、cat 默认原图，只有 `--legacy-display` 启用验证规则，`--decoded` 选择科学路径。SDK 时间选择与文件多时次消歧不因此改变。002 Assumptions/研究中的“默认 legacy”与明确 FR/contract 冲突，列 Q-04，不据此授权自动转换。依据：[001 CLI](../../specs/001-radiust-v1-migration/contracts/cli.md)、[002 CLI](../../specs/002-cli-experience/contracts/cli.md)、[002 spec](../../specs/002-cli-experience/spec.md)。
- **C-10 — Python 与输出兼容变化必须明示。** 003 明确允许 `fetch` 默认返回绑定对象、显式 `to_xarray()`、Python source entry point 转为编译期 Rust adapter；wheel console 转发原生 CLI。NetCDF4/GeoTIFF/PNG/Zarr v2 的原生命令不依赖 Python 编码；原有 JSON/退出码、科学语义和正式输出身份继续作为契约。003 CLI contract 明示 download 缺省格式改用 `output.format`、显式 flag 优先，以及保留批量 partial result。普通配置层级出现冲突，不能从实现文档推断兼容变更已获批准（Q-05）。依据：[003 core/SDK](../../specs/003-rust-core-performance/contracts/core-sdk.md)、[CLI](../../specs/003-rust-core-performance/contracts/cli.md)、[docs SDK](../../docs/python-sdk.md)。
- **C-11 — 平台声明受证据约束。** 001 原始矩阵为 CPython 3.10–3.13、Linux x86_64/aarch64、macOS arm64/x86_64；003 首轮发布和性能门槛明确仅 macOS arm64，不把其他平台列为本轮首发阻塞。安装后的 wheel、原生二进制、格式读回和运行资源定位必须独立检查；macOS 11 tag/deployment target 不代表实际 macOS 11 主机验证（Q-07）。依据：[001 plan](../../specs/001-radiust-v1-migration/plan.md)、[003 plan](../../specs/003-rust-core-performance/plan.md)、[安装文档](../../docs/installation.md)。
- **C-12 — 验收边界不能由任务勾选替代。** 001 M0–M4 是一个 spec 内的门槛，不另造五个 spec；每个非例外来源需要真实样本和科学契约，例外须有接受记录。002 已记录 TH cmp1 批次豁免、RainViewer/Windy 新格式留待新算法、BMKG 等待新可达来源、OpenSnow/WU 移出 legacy-display 范围；这些任务可按已确认范围关闭，未验证路径仍 blocked。003 性能按同机同输入每场景至少 30 次核对语义、p95、进程树 RSS/请求/tmp；001 的六类 canonical 来源基准仍独立开放。依据：[001 delivery gates](../../specs/001-radiust-v1-migration/plan.md)、[002 来源批次 tasks](../../specs/002-cli-experience/tasks.md)、[003 SC-003–006](../../specs/003-rust-core-performance/spec.md)。

- **C-13 — RDCAP 独立来源与计数扩展。** 004 新增 `rdcap` / `reflectivity`，公开站点为 `TWN/<站码>`、`JPN/<站码>`、`PHL/<站码>`；49 条记录去重为 48 个身份（13/20/15），保留 BALE 状态冲突，目录活跃不等于当前有资料。动态目录可增补，48 不是永久上限。实施后的冻结总快照由 24 来源/26 目标增至 25/74；C-03 和 003 的旧 24/26 验收基线继续保留，不能靠修改总数掩盖旧来源缺失。`tw`、`tw-http`、`ph` 不被替换。依据：[004 FR-001–006/019/023](../../specs/004-rdcap-single-station/spec.md)、[R02/R03/R10](../../specs/004-rdcap-single-station/research.md)、[CLI/SDK 兼容边界](../../specs/004-rdcap-single-station/contracts/cli-sdk.md)。
- **C-14 — RDCAP 科学解释有明确版本与边界。** 首版仅解读已验证的 CSR、数值变换及 EPSG:4326 左上角注册组合；保留负值/弱回波和当前帧几何。`9999` 的范围圆/站心角色是实测推断，版本化排除为 NaN + quality 65（missing | 新增 bit6 `source_annotation`），保留原始值和推断依据；旧 bits 0–5 不重定义，Q-05 的历史冲突仍开放。默认科学预览/PNG 共用 RDCAP 离散色标，低于 5 dBZ 透明但科学值不丢失；未验证头/标记模式拒绝科学解释。不推断高度、QC 或雨强。依据：[004 FR-012–017/022](../../specs/004-rdcap-single-station/spec.md)、[R07–R09](../../specs/004-rdcap-single-station/research.md)、[科学与持久化合同](../../specs/004-rdcap-single-station/contracts/science-persistence.md)。
- **C-15 — RDCAP 在线原始获取与离线解码分层验收。** 生产获取使用共用 Rust Engine 的原生 HTTP 和正常 TLS；每张票据一次文件 GET，同帧有界刷新，首读原始响应与确定性绑定保存后复用，票据不参与身份、不公开。不依赖人工 Orca、复制票据或新增浏览器回退。浏览器 CSV/重建 envelope 只证明内容与离线回放；T028/T053 要求三国各当前有资料站经标准入口取得原始响应并完成科学读回，任一国家缺证据保持该国 live 未验收和最终 gate 开放。不能以 skip/另一国家/离线通过补齐。依据：[004 FR-007–011/020–024、SC-003](../../specs/004-rdcap-single-station/spec.md)、[R05/R06/R10](../../specs/004-rdcap-single-station/research.md)、[T028/T053/T058](../../specs/004-rdcap-single-station/tasks.md)。

## Planned Specs

此节同时纳入已有 specs，标题沿用模板 ledger 名称。以下统计是现存 checkbox 的数量，不是产品完成率；未将历史页头 Draft、文档任务或登记 blocked 的任务当作整体完成。

| Spec | 生命周期判断 | 已勾选 / 现存任务 | 未勾选任务 | 主要未关闭门槛 |
| --- | --- | --- | --- | --- |
| 001 | `in-progress` | 148 / 154 | T011、T038、T053、T065、T084、T085 | 真实来源科学/时间/几何、存储 provider、v1 交接 |
| 002 | `in-progress` | 66 / 67 | T039 | PH 同帧合法 raw/旧 gray；部分显示路径仍 blocked |
| 003 | `in-progress` | 80 / 84 | T023、T034、T041、T082 | 来源/显示科学证据、真实存储、最终迁移 gate |
| 004 | `specced` | 0 / 58 | T001–T058 | 原生三国 HTTP 原始获取与在线科学读回；离线科学/四格式/入口回归待实施 |

003 现存编号跨度为 T001–T085，缺独立 T035 条目；其 NetCDF 内容出现在 T034 进展中。没有证据确认这是有意合并还是编号遗漏，列 Q-06，统计只按实际条目。

### 001 — radiust v1 独立雷达数据获取与全量迁移  [status: in-progress]

- **Description:** 将旧项目全部雷达来源迁为独立 SDK/CLI，建立统一查询、获取、科学语义、输出与迁移证据。从 framework 到全量来源交接均属于本 spec。
- **Outcome:** 001 SC-001–SC-011 闭合：全部旧实现有去向或已接受例外，每个非例外真实样本通过发现/获取/科学和几何验证；六类代表来源、格式/存储/取消/缓存/终端/安装均有证据，之后才能提出旧库归档。MVP 或盘点完整不足以满足该 outcome。
- **Scope (in):** US1–US6、FR-001–FR-037；24 条 HEAD 路径及两条历史巴西线索；目录/产品/站点、UTC latest/at/range、sync/async 与 batch/stream、科学质量与 native/显式 regrid、HTTP/FTP/tile/browser/recovery、四格式、本地/OSS/S3、正式 raw/raw-only、幂等 manifest/cache/config/doctor、四类终端模式、M0–M4 及来源/发布证据。
- **Scope (out):** 旧生产集成和旧 API/config 兼容；HTTP 服务、常驻/分布式队列、远端多写者/exactly-once、共享 Zarr append、动画/watch、未验证雨量守恒映射；不从 JMA 示例新增迁移来源，不自动执行发布或旧库归档。
- **Depends on:** 尚未发现对其他已编号 spec 的明确前置；外部证据前置是旧代码/合法 raw、产品科学/几何资料和 provider/运行环境。001 剩余目标是否需要依赖 003 完成当前原生路径的对应能力，现有 artifacts 未明示，待确认 Q-03，不登记反向硬依赖。
- **Governed by:** C-01、C-03–C-08、C-11–C-12；架构和入口的后续明确变化见 C-02、C-09–C-10。
- **Addresses:** [根 plan：目标、分阶段交付与最终边界](../../plan.md)。未发现 PRD。
- **Spec dir:** [specs/001-radiust-v1-migration/](../../specs/001-radiust-v1-migration/)。
- **Key decisions:** 历史计划采用 PyO3+maturin、Rust transport/cache/tile/OpenDAL + Python 科学；h5netcdf NetCDF4/CF-1.8、GeoTIFF 三件组、Zarr v2、严格身份/资源/提交。003 已明确迁移语言归属和基础依赖，科学/正式数据契约继续作为基线。
- **Status evidence:** [tasks](../../specs/001-radiust-v1-migration/tasks.md) 148 checked/6 open；[release-readiness：2026-09-30 recheck](../../migration/release-readiness.md) 明示 `ready_for_v1=false`，发布与旧库归档仍受阻；[迁移文档](../../docs/migration.md) 亦明确迁移未关闭。spec/plan 页头记录的是早期规划状态，不能覆盖后续证据。
- **Remaining evidence:** T011/T038 为 MY canonical raw/许可/有效时间/原生几何；T053/T065 为 SIDARMA 等代表及更广科学证据；T084/T085 为真实 AWS/S3-compatible/OSS 与完整存储验收。001 六类基准仍只有部分 canonical 输入；已接受 UK 退役例外、终端 CI/PTY 替代路径和真实 provider 延期分别留证，不能推广为全部来源例外或完整产品验收。
- **Notes:** 003 改写实现归属不等于取消本 spec 的全量迁移结果，也不将 003 的原生性能基准计作 001 缺失的六类真实来源基准。继续关闭现有任务，不在此重写 spec/plan/tasks。

### 002 — CLI 行为修复与终端体验优化  [status: in-progress]

- **Description:** 在已有 SDK/CLI 上实现四项 TODO 和终端体验改进：原图预览、CLI 缺省 latest、全目录最新发现、人类可读报告/进度以及来源专属 legacy 灰度迁移。
- **Outcome:** US1–US5、SC-001–SC-010 按已记录范围满足：查询默认一致，每目标恰一个终态、单一完整 JSON；预算耗尽/取消后 5 秒内有界收尾；40/80/120 列和管道不丢关键内容；真实人类报告 review 及合法逐路径像素/alpha/尺寸/背景比对可复查，显示通过不升级科学状态。
- **Scope (in):** raw PNG/GIF 单帧与离线文件预览、GIF 首画面标识、显式 `--legacy-display`/`--decoded`、latest 规范化且 SDK 默认不变；discover all 目标账本/状态/300 秒整批预算；安全文本、stderr 进度、quiet/NO_COLOR/JSON；14 地区及 RainViewer/Windy/BMKG 三类纳入路径的规则/覆盖/证据。当前 legacy ledger 为 23 路径，15 passed/0 difference_pending/8 blocked。
- **Scope (out):** 科学真值推断、科学能力升级、raw 预览正式 output/regrid、watch/持续 dashboard/全屏 TUI、GIF 动画、多语言；OpenSnow/WU 不纳入本次 legacy-display，但独立采集迁移仍保留。all 不支持 at/range/product/station/base-time；显式多来源查询后来由 003 承担。
- **Depends on:** **001 的已有能力与证据基线。** 002 plan Summary/Phase 1 明确复用 SDK Query、FrameRef/RawFrame、获取、终端和资源预算；002 T032 明确使用 [001 source-inventory](../../specs/001-radiust-v1-migration/source-inventory.md)。这是能力复用依赖，不要求 001 M4 或全部真实 provider 先验收。
- **Governed by:** C-04–C-09、C-12；003 迁移后的实现归属见 C-02/C-10。
- **Addresses:** [TODO：CLI 问题及显示迁移](../../TODO.md)，不是 PRD。
- **Spec dir:** [specs/002-cli-experience/](../../specs/002-cli-experience/)。
- **Key decisions:** 历史计划复用 Click/Pillow/NumPy，独立 RawPreview、显示引擎/证据、聚合报告；隔离 worker + 父级共享限额保证硬预算，显示规则按版本/hash 验证。003 将执行位置迁到 Rust，保留这些行为/验收基线。
- **Status evidence:** [tasks](../../specs/002-cli-experience/tasks.md) 66 checked/1 open（T039）；[验收日志末尾用户旧快照回放与调查 follow-up](../../validation-results/002-cli-experience.md) 记录 15/0/8、PH 缺配对输入，用户已分别确认三份报告可在 30 秒读懂；[TODO](../../TODO.md) 保留灰度迁移未完成。部分回归曾因环境/旧 wheel 失败，不能把后续 003 回归泛化为此 spec 的全部原始验收通过。
- **Remaining evidence:** T039 名义批次 NZ/PH/SG，记录的实际缺口为 PH 无合法同帧 raw + old gray；不能重开已通过 NZ/SG，也不能因只有 PH 缺项而将该任务或整个 US5 标完成。其余 blocked 路径依据用户已确定范围保留，不自动启用转换；待确认事项见 Q-02/Q-04。
- **Notes:** [002 quickstart](../../specs/002-cli-experience/quickstart.md) 的“25 路径全 blocked”与早期计划不是当前证据快照。SC-008/checklist 仍写 5 类瓦片，与 FR-019/contract/任务中 OpenSnow/WU 排除冲突；本 roadmap 保留明确范围记录并列待确认，不修改原文。

### 003 — Rust 核心迁移与性能优化  [status: in-progress]

- **Description:** 把既有业务执行汇入可嵌入 Rust Engine，交付无需 Python 的 macOS arm64 原生 CLI；Python 保留薄绑定和显式科学互操作，并改善发现、下载与首帧性能。
- **Outcome:** SC-001–SC-008 全部具备证据：七组命令原生可用、24 内置 ID/26 目标的已有能力与身份保持、四格式/旧成果可读回，显示/科学不静默变化；每场景至少 30 次同机测量，list/discover-all p95 ≤旧版 50%，四延迟来源 ≤串行 2/3，cat 首帧改善 ≥20%，RSS 不增加且清理完整；基础/科学两种 wheel 安装和迁移示例成立。
- **Scope (in):** 24 内置来源的既有可用能力；Rust catalog/模型/identity/config/HTTP/FTP/可选 CDP/OCR、公平有界 discover all/指定多来源、并行下载、科学/legacy/raw 边界、原生四格式及本地文件读取/预览、cache/旧 v1 manifest/锁/本地和 S3/OSS commit、CLI/SDK 共用、默认绑定返回/显式 xarray/编译期来源扩展、macOS arm64 安装和配对性能证据。T083 清理 Python 业务和 T084/T085 格式预览/实际 ANSI 首帧测量是现有追加任务。
- **Scope (out):** 本轮桌面 UI、后台服务、IPC/Swift 绑定；其他平台首发承诺；未登记 BR 来源的原生覆盖；从 raw/display 推断新科学能力；运行时 Python source 插件及缺原生能力时的 Python pipeline 回退。
- **Depends on:** **001 与 002 的已有契约、能力和证据。** [003 core/SDK](../../specs/003-rust-core-performance/contracts/core-sdk.md) 明确保留 001 SDK 行为，[persistence](../../specs/003-rust-core-performance/contracts/persistence.md) 明确补充 001 storage/encoding；[003 CLI](../../specs/003-rust-core-performance/contracts/cli.md) 明确扩展 002 CLI 并保留其 preview/aggregate 行为。不要求 001/002 全部外部验收闭合后才迁移。
- **Governed by:** C-01–C-08、C-10–C-12；CLI 默认继承 C-09。
- **Addresses:** [根 migration.md：目标、迁移顺序、性能与完成条件](../../migration.md)。
- **Spec dir:** [specs/003-rust-core-performance/](../../specs/003-rust-core-performance/)。
- **Key decisions:** 单一 Rust Engine + 独立 CLI + PyO3；24/26 目录冻结，首轮 macOS arm64；发现 worker 4 与帧/request/host/CPU 独立限额；可选系统 Chromium/Tesseract；兼容 v1 manifest/旧 flock、隔离缓存；原生 NetCDF/HDF5 静态构建、GeoTIFF/Zarr 独立读回；可选 Python science 与文档化破坏性 API/插件变化。
- **Status evidence:** [tasks](../../specs/003-rust-core-performance/tasks.md) 80 checked/84 现存条目、4 open；[验收报告 continuation/streaming retry 更新](../../validation-results/rust-migration-acceptance.md) 和 [根 migration.md](../../migration.md) 明示整体 incomplete；目录/原生闭环、性能与 wheel 局部 gate 已完成，不能提升为 implemented/verified。
- **Remaining evidence:** T023 为来源/瓦片与历史显示配对证据；T034 为更广来源科学/geometry/datum/projection（含未支持的 TW EPSG:3821→EPSG:4326）；T041 的 RemoteStore 协议和 loopback MinIO 已有实现/合同，但真实 AWS/S3-compatible/OSS 尚未验收；T082 受上述必需项阻塞。实际 macOS 11 主机/GitHub source-build 未运行是单列平台覆盖限制，不把本机 macOS 26 上的 wheel 审计改写为全平台通过。
- **Notes:** 最新记录显示 loopback/离线 SC-003–006 的 30 对测量通过；ANSI 首帧测的是 PTY 收齐输出，不是终端 paint 或公网 RTT。这不关闭来源与真实 provider gate。旧性能数字、早期“23 adapter/PH 缺实现/基线未完成”由验收日志后续明确 supersedes，保留它们的历史性质，不重复当作当前新缺口。

### 004 — RDCAP 台湾、日本、菲律宾单站雷达支持  [status: specced]

- **Description:** 将中央气象署亚太雷达资料中心 RDCAP 的台湾、日本、菲律宾近期单站回波作为独立来源接入现有原生 CLI 和同步/异步 SDK，提供站点选择、真实时间线、自动原始获取、可信 dBZ 网格和现有输出工作流。
- **Outcome:** [004 SC-001–SC-008](../../specs/004-rdcap-single-station/spec.md) 全部留有可复查证据：48 唯一身份及冲突可见，秒/毫秒实际时间与 latest/at/range 匹配正确；三国各当前有资料站通过标准入口自动发现→原始获取→解码→至少一个科学成果独立读回；三国样本数值误差 ≤0.01 dBZ、几何误差 ≤原生格距×1e-6，花莲八参考点符合基准；标记/缺测/弱回波不混淆，四格式与重复复用、异常/取消/部分失败和 CLI/同步/异步等价通过；TW/TW-HTTP/PH 保持既有行为。离线通过不能替代任何国家的在线门槛。
- **Scope (in):** US1–US4、FR-001–FR-024；`rdcap` / `reflectivity`、三国国家/站码、48 站去重快照与动态目录、目录/实时状态分离、近期实际 UTC latest/精确 at/半开 range；单读票据同帧有界刷新、原始 JSON 响应和确定性 binding、manifest 离线重放；受限 CSR→f32 dBZ/u16 quality、EPSG:4326 原生注册/显式 geographic regrid、版本化 annotation bit6 和离散色标；raw/raw-only、科学预览及 PNG/NetCDF/GeoTIFF/Zarr，完整逐目标报告、批量/部分结果、CLI/同步/异步共用 Engine、身份/cache/幂等/安全模板/预算/取消；三国内容/离线/live 分层证据、macOS arm64 安装后 CLI/wheel 检查与旧来源回归。
- **Scope (out):** 其他国家、国家拼图、长期历史归档、体扫、仰角/扫描高度、风场、QC 或定量雨强声明；替换或重新解释 `tw`/`tw-http`/`ph`，新 Python 来源业务 pipeline、生产浏览器回退或人工维持 Orca；新增平台支持或延迟 SLO；重新宣布 001/003 未验收远端存储通过、发布或旧库归档。
- **Depends on:** **001、002、003 的已有契约与能力（均为 `in-progress`）。** 001 的科学/质量、身份、raw/manifest/cache/输出基线；002 的 latest、单帧预览/消歧、完整报告和预算收尾；003 的共用 Rust Engine、原生 adapter/transport/writer、薄 Python 绑定及 macOS arm64 基线。依据：[004 Assumptions](../../specs/004-rdcap-single-station/spec.md)、[plan Technical Context/Constitution Check](../../specs/004-rdcap-single-station/plan.md)、[R01/R05/R06/R10](../../specs/004-rdcap-single-station/research.md)。这是能力/契约复用，不要求 001–003 全部外部验收先完成；外部前置为合法当前资料、正常 TLS 的上游可达性及独立读回环境（Q-09）。
- **Governed by:** C-01–C-02、C-04–C-12、C-13–C-15；C-03 的历史计数按 C-13 区分新旧范围。未发现正式 ADR，宪章仍为未批准占位模板。
- **Addresses:** [RDCAP 分析报告](../../docs/rdcap-single-station-analysis.md)及 [004 Input/用户场景](../../specs/004-rdcap-single-station/spec.md)。未发现 PRD，不将研究报告冒称 PRD。
- **Spec dir:** [specs/004-rdcap-single-station/](../../specs/004-rdcap-single-station/)。
- **Key decisions:** 一个独立来源/产品，公开 country/code 与安全输出分量分开；动态目录 hook 缺省不影响其他来源；原生 HTTP 单次 GET、最多三张票据且刷新仍匹配原 key，实际请求计入共享期限；原始响应不重编码，绑定确定性且无秘密；CSR 科学解释、几何、annotation/palette/decoder 均版本化，未验证模式拒绝；新增薄 discover_report/replay_raw_manifest 及可选安全错误 code，公开机器 schema/既有退出码保持。
- **Status evidence:** [requirements checklist](../../specs/004-rdcap-single-station/checklists/requirements.md) 16/16 证明规格质量；[plan](../../specs/004-rdcap-single-station/plan.md) Phase 0/1 设计完成，[tasks](../../specs/004-rdcap-single-station/tasks.md) 0 checked/58 open 且明示所有任务未实施。因此登记 `specced`，已有研究样本不作为 adapter 实施或 live 验收证据。spec/plan/tasks 的 `main` 页头为早期分支记录，当前 prerequisites 解析为 `004-rdcap-single-station`，不据分支名判断实施已开始。
- **Remaining evidence:** T018 目录/时间回放；T027 raw 离线完整性；T028 三国标准入口 live raw；T041 离线科学/四输出；T052 离线异常/入口矩阵；T053 三国在线科学读回；T055/T056 安装/旧来源回归；T057 逐国能力实证更新；T058 全部范围最终 gate。浏览器外完整文件获取曾超时，正常 TLS 原生路径尚未验收（Q-09），不能预填通过。
- **Notes:** 按 [tasks Dependencies & Execution Order](../../specs/004-rdcap-single-station/tasks.md) 推进：目录→raw 离线接口，科学可用已有内容样本独立进行；T028 受限不阻止离线工作，但 T053/T058 必须保留开放。实施实际开始后再按证据转为 `in-progress`，规格/设计/研究完成不足以提前迁移状态。

## Open Questions

以下 `needs-info` 是问题状态，不替代三项已能确定的 `in-progress` 生命周期；补齐证据或确认后才能修订相应关系/决定。

- **Q-01 — needs-info：项目治理。** constitution 仍未填写；正式原则、ratification 和 ADR/PRD 归属未建立。现有 specs 一致说明不能评估已批准宪章。需要独立制定/确认后再核对，本次不补写或追认。
- **Q-02 — needs-info：未验收材料与例外闭合。** 001 MY/代表源科学证据、真实存储与六类 canonical 基准，002 PH 配对样本，003 T023/T034/T041 都有明确缺口；尚未有全部补齐或全范围例外接受记录。需要来源/产品级合法输入、时间/物理/几何说明、真实 provider 矩阵或明确接受的范围调整；所有权人和日期未知，不编造。保留已有局部延期/豁免，不将它们扩大。
- **Q-03 — needs-info：历史迁移与原生目录的后续关系。** 001 包含两条 BR 历史迁移去向，003 明确不把它们算入 24 内置。是否未来注册原生 adapter、以何验收收口 001，或是否明确由 003 的哪些能力承担 001 剩余目标，当前无正式映射。保留范围差异；不写 001→003 反向硬依赖或“003 取代全部 001/002”。
- **Q-04 — needs-info：002 内部显示契约残留。** FR-001/022 与 contracts 明确 raw 保留原图、显式 legacy；Assumptions/部分研究/tasks checkpoint 仍说默认/自动 legacy。SC-008/checklist 写 5 类瓦片，FR-019/contract/任务已排除 OpenSnow/WU 只纳入 3 类。明确后续是否只修正文案，不能依较宽旧句恢复被排除范围或自动转换。已有用户范围收敛见 [002 日志](../../validation-results/002-cli-experience.md)。
- **Q-05 — needs-info：配置优先级与质量位冲突。** 001 FR-025/CLI 为显式 > 文件 > 环境 > 默认；003 要求保留旧优先级，但 docs SDK/CLI/installation 写显式 > 环境 > 文件 > 默认。没有明确兼容变更批准记录，需确认规范。003 data-model 把 bit 5 写作 recovered，而 001 与 docs SDK 为 bit 3 recovered、bit 5 below_detection；需按既有文件/golden 核对后确认，不能由 roadmap 重定义编码。
- **Q-06 — needs-info：artifact 漂移与任务编号。** docs architecture 图写 23 adapter、正文 24；共享缺口写 renderer 仅 auto/text，而 docs CLI 已列五种；docs output-maintenance 仍写拒绝旧 entries、远端未串接，而 003 tasks/更新记录已有兼容 reader/RemoteStore；docs source-development 仍说旧 Python pipeline 可用，而 docs SDK/003 T083 说明已移除。003 缺独立 T035；001/002 quickstart 和 spec/plan 页头残留早期状态。需要逐项确认和单独维护，不在本次改 originals 或用旧句降级已明确完成的局部 gate。
- **Q-07 — needs-info：平台支持声明。** 001 多平台矩阵与 003 首发 macOS arm64 的作用范围已分别记录；实际 macOS 11 host 和 GitHub source-build 未运行。是否需在首发支持声明前新增这些证据或限定支持版本，现有 artifacts 未给出统一结论。保留 wheel tag、实际运行主机与验收任务范围的区别。
- **Q-08 — undecided：后续独立 specs 与排期。** 未来桌面界面、RainViewer/Windy 新匹配算法、BMKG 新可达来源、更多 CWA 产品只出现为意图/线索或排除范围；没有独立 spec 编号、批准 scope/outcome、依赖或交付日期。暂不创建 004+ 条目；是否独立规划及顺序待用户后续决定。
  - **2026-10-01 增补：** 上述“暂不创建 004+”为 2026-09-30 历史快照，已由本次登记的独立 RDCAP 004 补充；004 并非上述桌面/新匹配算法/BMKG/更多 CWA 产品意图的自动实现。原列未来主题的独立范围、顺序及排期仍未定，不新增其他编号或日期。
- **Q-09 — needs-info（验收证据）：RDCAP 原生三国 HTTP 完整链路。** [004 R05](../../specs/004-rdcap-single-station/research.md) 记录浏览器三国文件/独立索引成功、浏览器外完整文件超时，尚无共用 Engine 正常 TLS 自动获取并科学读回的三国证据。由 T028/T053 留存逐国日期、build、帧身份、原始摘要与读回结果后核对关闭；若受限保留该国 live 未验收及 T058 开放。这是已定原生方案的执行/证据风险，不是未决定的技术选择；所有权人和完成日期未确定。

## Cross-Cutting Notes

### 已证实的依赖与关系

| 使用方 | 依赖能力 / 关系 | 明确依据 | 不作出的推断 |
| --- | --- | --- | --- |
| 002 | 001 的 SDK/RawFrame/目录/终端/限额和旧盘点 | 002 plan Summary、Phase 1；T032 引用 001 inventory | 不要求 001 全量迁移或存储验收先完成 |
| 003 | 001 的 SDK、identity、存储与编码兼容基线 | 003 core/SDK 与 persistence 明确补充 001 contracts | 不免除 001 真实来源/交接义务 |
| 003 | 002 的 CLI latest/raw/legacy、aggregate 与安全行为 | 003 CLI 开头与 Preview/output 明确扩展 002 contract | 不要求 PH 配对样本先到齐；不把 002 自动标 verified |
| 003 与 001/002 | 明确迁移实现归属，保留行为/科学证据 | 003 spec Assumptions、core/SDK Compatibility | 不是全范围 supersedes/abandoned；反向依赖待 Q-03 |
| 004 | 001/002 的数据/持久化/CLI 合同与 003 Rust Engine/transport/writer/绑定能力 | 004 Assumptions、plan Technical Context、R01/R05/R06/R10 | 不要求 001–003 完整验收先闭合；不替代旧来源或升级未验收 provider |

这里的能力依赖给出 `001 → 002`、`001 → 003`、`002 → 003` 的关系，没有证据支持额外跨 spec 硬前置。源码目录变化、编号先后和未提交状态不单独作为依赖依据。

**2026-10-01 增补：** 004 明确复用上述能力，新增 `001/002/003 → 004` 的能力/契约关系；所有使用方按实际接口与证据推进，不新增“依赖 spec 必须先整体 verified”的硬门槛。

### 阶段和验收推进依据

- **001：** 沿已有 M0 盘点 → M1 最小 SDK/本地科学输出和 ANSI/text → M2 六类复杂获取/终端 → M3 完整输出/可靠性 → M4 全量来源/交接 gates；这是该 spec 的原始门槛，不代表目前已逐阶段通过。外部证据缺失时继续独立工作，保留缺项。
- **002：** 基础安全/时间规范 → raw/latest/报告 → 显示规则与合法逐路径比对、all 调度 → 文档/自动和人类验收；PH 材料缺失不阻止其他已授权工作，也不算整体完成。范围收敛按现存接受记录。
- **003：** 冻结旧契约/30 次基线 → 独立 core → 来源/有界调度 → 处理/预览/格式 → 持久化/CLI/绑定/发布检查；最新证据显示大量局部 gates 已过，当前由 T023/T034/T041 和依赖它们的 T082 收口，无证据时不标完成。

- **004：** 目录/时间→原始保存/离线绑定→科学/四格式→批量/CLI/同步异步→安装与旧来源回归；离线可独立推进，T028/T053 的三国正常 TLS 在线链路分别留证，T058 依赖全部必需门槛。当前尚未实施（0/58）。

### 读取范围与证据索引

完整读取项目 `specs/` 下 **32 份 Markdown**（三项 spec/plan/tasks、研究、模型、quickstart、全部 Markdown contracts/checklists 及 001 inventory）、根 [plan.md](../../plan.md) 和 `docs/` 下全部 **10 份 Markdown**；补充根 migration/README/TODO、项目 memory/config 和相关验收记录。`.specify/extensions/roadmap/specs` 与其 `.specify/memory/roadmap.md` 是扩展工具自身的示例/规格，不纳入 radiust 的项目 ledger。

**2026-10-01 增补读取范围：** 004 spec/plan/research/tasks、科学与持久化合同、目录/SDK/协议相关合同及 requirements checklist；研究报告与实测目录为既有来源指针。本次不重新验收 001–003，不把初始化时的文件数量当作当前仓库总量。

| 材料 | 用途 |
| --- | --- |
| [001 artifacts](../../specs/001-radiust-v1-migration/) | 全量迁移目标、M0–M4、科学/存储/SDK/CLI 原始基线及任务 |
| [002 artifacts](../../specs/002-cli-experience/) | CLI 行为、显示规则/证据、all deadline、安全布局与验收 |
| [003 artifacts](../../specs/003-rust-core-performance/) | 原生核心范围、明确兼容变更、性能/持久化/平台 gate 与任务 |
| [004 artifacts](../../specs/004-rdcap-single-station/)、[RDCAP 分析](../../docs/rdcap-single-station-analysis.md)、[研究资料](../../validation-results/rdcap-analysis/) | 三国近期单站范围、独立来源/动态目录、原始绑定/科学/显示设计与 58 项未实施任务；研究样本不替代 live 验收 |
| [原始计划](../../plan.md)、[原生迁移计划](../../migration.md) | 产品目标与架构/交付决策演进 |
| [architecture](../../docs/architecture.md)、[cli](../../docs/cli.md)、[python-sdk](../../docs/python-sdk.md) | 当前实现边界、命令与 SDK；与原契约冲突处列 Q-05/Q-06 |
| [installation](../../docs/installation.md)、[source-development](../../docs/source-development.md)、[output-maintenance](../../docs/output-maintenance.md) | 包/平台、来源接入、缓存与正式输出维护边界 |
| [migration](../../docs/migration.md)、[live-provider-tests](../../docs/live-provider-tests.md) | 迁移未闭合、延期 provider、凭据/退役及 opt-in 状态 |
| [tmd-source](../../docs/tmd-source.md)、[cwa-radar-products](../../docs/cwa-radar-products.md) | TMD printed-time/OCR、过期事实；TW numeric/image 不同产品与几何证据 |
| [v1 release-readiness](../../migration/release-readiness.md)、[002 实施日志](../../validation-results/002-cli-experience.md)、[003 acceptance](../../validation-results/rust-migration-acceptance.md)、[003 US1 matrix](../../validation-results/rust-migration-us1-matrix.md) | 生命周期和缺项依据，优先采用文中明确 supersedes 的后续记录 |
| [README](../../README.md)、[TODO](../../TODO.md)、[constitution](constitution.md) | 补充历史状态/已确认行为与治理缺口；不据陈旧 checkbox 或占位符伪造状态 |

Roadmap 的确认只批准这份项目台账；来源真实性、例外接受、平台/provider 验收、发布或旧库归档仍需各自证据。本次不运行产品、性能、网络、CI 或发布动作，不修改已有 spec、plan、tasks、docs 或 constitution。

---

**Version**: 1.1.0 | **Ratified**: 2026-09-30 | **Last Amended**: 2026-10-01
