---

description: "Rust 核心迁移与性能优化的依赖有序实施任务"
---

# Tasks: Rust 核心迁移与性能优化

**Input**: `specs/003-rust-core-performance/` 的 [spec.md](spec.md)、[plan.md](plan.md)、[research.md](research.md)、[data-model.md](data-model.md)、[contracts/](contracts/) 和 [quickstart.md](quickstart.md)。

**Scope**: 首发 macOS arm64；原生 CLI 与可复用 Rust core，Python 只保留薄绑定及可选 xarray/Zarr 适配。科学通过状态仍以 `migration/` 台账为准。本文件仅列待实施工作，勾选须有运行证据。

**Tests**: 规格的每个用户故事都定义了独立测试，且计划要求旧格式读回、故障注入和性能验收；因此列出有意义的合同、集成和跨工具验证任务。不要用仅复述实现的测试替代端到端检查。

## Format: `[ID] [P?] [Story] Description`

- `[P]` 仅表示前置门槛完成后可与同阶段其他不同文件任务并行；共享文件、需要上一任务结果的任务不标记。
- `[USn]` 对应 [spec.md](spec.md) 的用户故事。每行给出将编辑或创建的项目相对路径。
- 任务 ID 为建议执行顺序；同一阶段的 `[P]` 可按依赖表并行，不代表其检查已通过。

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: 在入口或存储变更前固定旧契约与正式基线，建立原生工作区骨架。

- [X] T001 在 `scripts/validation/capture_migration_contracts.py` 与 `tests/fixtures/rust-migration/cli/python-baseline/` 固定现有七组命令的 JSON、退出码、选项和敏感信息脱敏 golden；单源空列表、聚合发现、dry-run、停止/中断行为分别留样，并与 Rust CLI 快照分目录保存。
  - 完成证据：固定 8 个真实禁网 CLI 命令 golden，以及 4 个隔离的确定性边界样本（成功 dry-run、单源空结果、含 partial result 的 stop、操作中断）；使用临时 HOME/XDG 配置、清除凭据环境变量并递归脱敏敏感字段、URL/Bearer 文本。3 项合同测试通过，连续双跑的 12 个样本与 index 完全一致。
- [X] T002 [P] 在 `scripts/validation/capture_source_inventory.py` 与 `tests/fixtures/rust-migration/sources.json` 固定 catalog 的 24 个 ID、26 个内置发现目标、产品/站点、退役状态、可用 fixture 的帧身份与 SHA-256；将 `br_cptec`、`br_sipam` 标为非内置而不计入覆盖率。
- [X] T003 在 `scripts/validation/benchmark_rust_migration.py` 与 `validation-results/rust-migration-baseline.json` 实现并采集迁移前同机 30 次冷/热 `list`、离线 `discover all`、四延迟来源、`cat` 首帧及批量下载的 p50/p95、进程树 RSS、请求数和临时盘峰值；记录机器/版本/样本指纹，禁止把旧三次冒烟当基线。
  - 完成证据：在 macOS 26.6.2 arm64 / CPython 3.12.7 上，以 Git HEAD `34e4cb42ed278e30048f10d9f391608c8d906550` 导出的旧 Python 树（SHA-256 `df172292c6d3de3fe402c0bca940623c3e15006198199d786fb800165ed663e7`）对比静态 arm64 release CLI（SHA-256 `22a1c1841072f19cd8970f9872ad5b0e423f69a950023947baff2177a287c7ae`）；四个 CLI 场景与七个 loopback 场景各完成 30 对，逐对语义指纹和正确性通过。旧 Python 独立基线见 `validation-results/rust-migration-baseline.json`，完整对照见 `validation-results/rust-migration-comparison.json`。Loopback 回放只访问 127.0.0.1，四来源请求逐项计数，报告标注 provider RTT 与 OS page-cache 未测限制。
- [X] T004 在 `Cargo.toml`、`crates/radiust-cli/Cargo.toml` 和 `crates/radiust-cli/src/main.rs` 加入最小独立 `radiust` 二进制 crate；它仅调用 core，不链接 PyO3，先确保无 Python 的 macOS arm64 构建入口可执行。
- [X] T005 在 `Cargo.toml`、`rust-toolchain.toml` 和 `Cargo.lock` 对齐可验证的 Rust 最低版本与 1.92 工具链，锁定新增依赖；保留 `cargo fmt --check` 与 `cargo test --workspace` 可运行。
- [X] T006 [P] 在 `tests/fixtures/rust-migration/output/` 与 `tests/fixtures/rust-migration/cache/` 固定合法旧 v1 manifest、缓存 `entries` 索引、原始对象及损坏/冲突样本，记录旧 Python 读回结果；fixture 不含凭据或不获许可资料。
  - 完成证据：`capture_legacy_storage_fixtures.py` 固定合法、字节损坏和有效字节但身份冲突的 v1 输出，以及 Python `entries` SQLite 表、原始对象和 ETag/过期时间；`readback-python.json` 记录旧 Python 的完整性结果。隔离复制索引后由旧 `CacheStore` 重新读回并验证大小/SHA/租约；两个 persistence fixture 合同通过。

**Checkpoint**: 旧行为、来源身份、持久化样本和性能基线可重复；原生 crate 骨架可构建。T003 必须在实际入口迁移前完成。

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: 建立所有故事共用的数据、身份、资源和安全边界；完成前不得切换旧缓存或正式输出写路径。

- [X] T007 在 `crates/radiust-core/src/model.rs` 定义 Query、DiscoveryTarget、FrameRef、RawFrame/Artifact、RadarField/Dataset、Preview、报告和终态枚举，包含 UTC、质量位、缺测及身份验证规则；对齐 [data-model.md](data-model.md)。
  - 完成证据：模型校验验证有时区 UTC、规范 logical_id、查询和报告的状态/目标一致性、Dataset 字段名/时间/grid 一致性，以及值/质量长度、非有限值与质量位关系；Rust 回归覆盖无标记 NaN 拒绝、带标记缺测接受和 Inf 拒绝。PyO3 Dataset 构造器也调用同一校验，Python 绑定合同确认不一致时间拒绝。`cargo test --workspace` 317 passed、1 ignored；绑定/输出格式合同 17 passed。
- [X] T008 [P] 在 `crates/radiust-core/src/errors.rs` 与 `error_contract.rs` 定义稳定的 code/stage/retryable、安全消息和部分结果表示；取消、参数错误、来源限制和远端提交结果未知不得被包装成成功。
  - 完成证据：`error_contract.rs` 统一 code/stage/retryable 与脱敏消息；参数错误映射到不可重试的 `invalid_query/validate`，取消/限额/来源错误保留失败语义；批量报告逐帧保留部分终态，`commit_outcome_unknown` 明确为 commit 阶段不可盲目重试。Rust core 单测 202 passed。
- [X] T009 在 `crates/radiust-core/src/identity.rs` 实现旧规范 JSON、UTC 时间和 locator 排除规则；用 `tests/fixtures/rust-migration/output/` 与 `python/radiust/identity.py` 的 golden 比对 logical/revision/processing/output ID，语义变化提升处理版本。
- [X] T010 [P] 在 `crates/radiust-core/src/config.rs` 实现默认值、YAML/环境/显式覆盖的旧优先级、未知键/重复键/凭据对校验及脱敏；预留 `runtime.discovery_workers=4`，网络默认关闭。
- [X] T011 在 `crates/radiust-core/src/transport/http.rs`、`crates/radiust-core/src/transport/ftp.rs` 与 `crates/radiust-core/src/limits.rs` 补齐 GET/HEAD/POST、共享连接/主机限额、跳转后主机检查、重试/超时、流式大小限额和取消；来源不得绕过网络 opt-in。
  - 完成证据：HTTP/FTP 均使用共享 request/host budget 和取消令牌；HTTP 合同覆盖 HEAD 重定向与网络门控、POST 编码和临时 503 重试、GET 跨源敏感头清理、同次请求合并、流式 SHA/限额/原子发布、跨源重定向拒绝及已写入部分文件的取消清理。`http_metadata` 12 项与 FTP/runtime lifecycle 合同通过；公网仍默认关闭。
- [X] T012 在 `crates/radiust-core/src/source/mod.rs` 与 `crates/radiust-core/src/source/catalog.rs` 建立编译期 Source 接口、24 项静态目录展开和能力/退役状态；内建目录加载无需 Python、网络或可选浏览器。
- [X] T013 在 `crates/radiust-core/src/engine.rs` 与 `crates/radiust-core/src/runtime.rs` 实现可复用 Engine 的操作上下文、进度事件、取消令牌、共享限额和资源收尾；CLI 与绑定只能经此入口执行业务操作。
  - 完成证据：Engine 的 discover/fetch/decode/regrid/download/write 入口通过 RAII `OperationContext` 发布 started/progress/completed/failed/cancelled，bounded broadcast 的事件只含操作 ID/类型/阶段/计数；订阅者缺席不影响业务。一次 Engine 上连续 discovery 事件 ID 不同，失败与预取消均有安全终态。一个共享 `RequestBudget`/取消 token 驱动 HTTP/FTP、发现、下载和提交；decode/encode 共用 CPU semaphore，local/remote commit 共用提交 gate，limits 从 Engine config 派生。跨入口 Python facade 调用同一 Rust Engine；暂存文件与租约由 drop/取消清理合同覆盖。`runtime_events` 5 项、`runtime_lifecycle` 6 项及 workspace transport/commit 生命周期合同通过，完整 `cargo test --workspace --offline --locked` 通过。
  - 当前进展：新增有界 broadcast 事件 API，Engine 的 discover/fetch/decode/regrid/download 入口都会发布生命周期事件；细粒度计数目前覆盖发现目标与单帧 artifact 获取。事件只含操作 ID/类型、阶段和计数，不泄露 locator/凭据。PyO3 Engine 与 `CoreEngineSession.subscribe_events()` 暴露非阻塞读取；公开 Python SDK 现将安全事件映射到 sync/async progress callback，并用 per-client reader/writer gate 隔离带回调操作，同时保留无回调并发。5 项 Rust 事件合同、3 项新增 SDK 进度/并发合同通过。各下载阶段实时计数、可复用 per-operation 取消令牌与所有资源守卫仍未完整覆盖，故保持开放。
- [X] T014 在 `crates/radiust-core/src/cache/index.rs` 与 `crates/radiust-core/src/cache/mod.rs` 设置旧 `entries` 与现有 Rust `objects` 索引的探测/隔离策略：兼容证明前使用独立根目录，禁止启动修复清理另一实现对象；记录旧条目读取、租约和回退策略。
- [X] T015 在 `crates/radiust-core/src/storage/local.rs` 对齐旧 Python advisory `flock` 协议和同根锁行为；与 `python/radiust/storage/local.py` 的进程并发合同验证前不得启用混合写入。
  - 已完成锁协议互通和释放/重入测试：Rust 使用同一个持久 `.radiust.lock` 文件及 `flock(LOCK_EX | LOCK_NB)`；实测 Python `fcntl.flock` 持锁时 Rust 不能获得锁。此项只提供互斥原语，正式 Rust 输出写入仍关闭，manifest/commit 的其余验收未完成。
- [X] T016 在 `crates/radiust-cli/src/lib.rs`、`crates/radiust-cli/src/main.rs` 与 `crates/radiust-cli/src/report.rs` 建立命令路由、v1 JSON envelope、stderr 进度/脱敏、安全错误和退出码映射骨架；业务规则仍由 Engine 提供。
  - 完成证据：七组原生命令均由 Rust 路由并通过 v1 报告/安全错误路径；TTY 上 discover、download 与 source cat 并行读取 Engine 的脱敏事件，在 stderr 显示操作阶段和计数，JSON stdout 保持单条完整 envelope；非 TTY 不输出进度，`--quiet` 保持安静。PTY 合同验证计数可见且不含凭据/locator；CLI crate 全量 59 项单元/进程合同通过，覆盖 JSON schema、错误阶段、部分结果退出码和取消 130。
- [X] T017 [P] 在 `crates/radiust-core/tests/replay_support.rs` 与 `tests/support/replay.py` 建立同一合法离线 fixture 的原生/旧版回放注入，包含可控慢源、悬挂、同主机、损坏和取消场景，默认不访问公网。
  - 完成证据：使用签入的 TW numeric-grid 原始 fixture；Python 旧 adapter 经内存 transport 发现/获取，生成旧版 raw manifest，再由 Rust PyO3 replay 解码并逐元素比对完整值数组、质量数组、shape 和 CRS。测试同时证明旧 `tw-legacy-v1` 与新 `tw-cwa-v2` discovery 身份相同；为保留这个兼容性，identity 对新 locator 中的新增解析元数据做 TW 专属规范化，Rust 仍在采集/解码时校验这些元数据。独立 Rust replay 合同覆盖 Python v1 manifest、损坏摘要拒绝、取消后的 staging 清理；Python transport 合同覆盖受控慢响应、同主机并发上限、悬挂取消及未注册 URL fail-closed。所有 fixture 默认无 socket/公网访问；`cargo test -p radiust-core --test replay_support --lib --offline --locked` 与 Python 回放/identity 定向合同通过，完整 workspace 与 Python 回归也通过。
- [X] T018 在 `crates/radiust-core/tests/foundation_compat.rs` 用旧 identity/cache/锁 fixture 验证 T007–T015 的跨实现边界；未通过时保持独立缓存根目录和只读旧成果，不开启新正式写路径。
  - 完成证据：3 项 `foundation_compat` 集成合同直接读取 Python 生成的 identity、v1 manifest 和 `entries` cache fixture，核对 frame/processing/output hash、正式成果完整性与缓存 SHA；另由 Rust 持有 `.radiust.lock` 并启动 Python `fcntl.flock` 子进程，确认互斥协议双实现一致。旧缓存根仍按 T014 隔离。`cargo test -p radiust-core --test foundation_compat --offline --locked` 3 项通过。

**Checkpoint**: Engine、目录、身份、配置、transport 和 CLI 骨架可离线运行；缓存/锁风险已由隔离或兼容合同覆盖。US2/US3 可使用 fixture Source 独立开发，不等待全部真实来源迁移。

---

## Phase 3: User Story 1 - 独立运行完整命令行工具 (Priority: P1) 🎯 MVP

**Goal**: 无 Python 的 macOS arm64 原生程序完成七组命令，覆盖全部 24 个内置来源的既有可用能力和四种正式输出，不升级未验证科学数据。

**Independent Test**: 在无 Python 的干净环境执行 `list/discover/download/cat/doctor/config/cache`，逐来源离线回放并对照旧报告、原始摘要、合法科学/显示样本和成果读回；退役来源不请求。

### Contract and integration tests

- [X] T019 [P] [US1] 在 `crates/radiust-cli/tests/cli_v1.rs` 添加七组命令的 JSON/退出码 golden 与无网络默认行为测试，保留单源 discover 精简 envelope 和旧特殊空结果边界。
  - 完成证据：Rust 原生 snapshots 固定 list、单源/聚合 discover、download dry-run 和 cat text；原生合同覆盖 doctor/config/cache JSON、网络默认关闭、空页 no_data/退役/缺凭据状态及退出码。`cargo test --locked --workspace` 的 16 项 cli_v1 与 6 项 multi-source CLI 测试全部通过。
- [X] T020 [P] [US1] 在 `crates/radiust-core/tests/source_matrix.rs` 添加 24 来源目录及合法样本的 discover/acquire/科学能力对照；blocked 路径只验声明限制，不伪造科学通过。
  - 离线合同通过：精确断言 24 个目录来源、26 个目标、24 个 native adapter、6 个 raw hook、两个已验收科学样本、退役和凭据/禁网预检；PH 的 HTTP parser/security 与浏览器 raw fallback 离线合同覆盖，无 provider 网络访问。Windy raw 路径已有 HTTP 与 CDP 两种实现，真实浏览器/provider 验收另行跟踪。
- [X] T021 [P] [US1] 在 `crates/radiust-core/tests/format_readback.rs` 添加 NetCDF4、GeoTIFF 三件组、PNG+sidecar、Zarr v2 的语义读回用例；用现有 Python 工具独立读回数值、quality、时间、CRS 与显示像素。
  - 完成证据：跨格式合同按 writer 拆分在 `tests/contract/test_rust_{png,netcdf,geotiff,zarr}_readback.py` 与 Rust `tests/netcdf.rs`、`tests/geotiff.rs`、`tests/zarr.rs` 中维护；Python 分别用 Pillow/JSON、xarray+netCDF4、Rasterio、xarray 独立核对 dtype/value、quality、时间、CRS/affine、orientation、nodata、provenance 和 Zarr chunk/codec。GeoTIFF fixture 覆盖 EPSG:3857 投影和 EPSG:3821 地理 CRS。工作区 Python/Rust 回归均通过。
- [X] T022 [P] [US1] 在持久化兼容测试中添加旧 v1 manifest 的 skip/repair/conflict、旧缓存读回/GC、同根锁及本地/远端 manifest-last 故障注入；不能只验证 mock 写入成功。
  - 完成证据：测试按职责拆分在 `storage::commit`、`cache_compat`、`cache_gc`、`storage::local`、`commit_faults` 和 `storage::remote_commit`，覆盖 Python v1 manifest skip/repair/conflict、旧 entries/object 路径和租约、GC/clear 保护、Rust/Python flock 互斥，以及本地/远端 manifest-last 故障。完整 `cargo test --workspace --offline --locked` 通过。远端 fault layer 验证协议，不替代 T041 的 AWS S3/OSS provider 验收。

### Core models, source adapters, processing and persistence

- [ ] T023 [US1] 在 `crates/radiust-core/src/source/legacy.rs` 与 `crates/radiust-core/src/source/tiles.rs` 移植合法图片/瓦片来源共有的发现、原始获取、版本化显示规则和完整性约束；raw-only 不触发科学解码或猜测瓦片。
  - 当前进展：RainViewer（512×512）、Windy 和 BMKG（256×256）接入固定 2×2 raw PNG 拼图；只有完整 FrameRef identity/locator 通过对应来源适配器验证后才允许拼接，并逐项校验四个 artifact 名称、PNG 类型、文件路径、receipt 大小/SHA、尺寸与资源预算。离线集成合同逐像素验证 RainViewer/Windy retained fixtures 并覆盖缺块、重复坐标、伪造 locator、错误摘要、越界链接和超限拒绝；BMKG 合成 256×256 tile 的像素顺序及伪造 locator 拒绝单测通过。`source_tile_preview` 7 项合同还显式验证 WU 来源不能借用 RainViewer artifact 猜测拼图布局；这只是 fail-closed 回归，不证明 WU 的真实布局。新增 `blocked_legacy_display_paths_keep_original_pixels` 对 8 条证据受阻路径逐一断言不应用未验收规则、不缩放且原样保留像素；RainViewer、TH/cmp1、TW-HTTP 和 Windy 使用现有 raw fixture，BMKG、ID-SIDARMA、PH 和数值 TW grid 使用合成 RGBA 仅验证 fail-closed 分支。该回归不替代路径级兼容证据。source-cat 单图路径在显示前验证 image media type、receipt 形状/大小/SHA；`preview_semantics` 7 项、source-cat 4 项和 `cat_first_frame` 9 项合同通过。Native legacy display 现在对 15 条 source-matched 历史路径逐像素验证。BMKG 没有保留成功的上游响应，合成测试不证明 provider 可用；WU/OpenSnow 布局及另外 8 条证据缺失路径仍未验收，任务保持开放。
  - 公开资料候选（不构成 parity 验收）：台湾 CWA 的 [雷达合成回波图](https://data.gov.tw/dataset/75125) 与 [QPESUMS 数值网格](https://data.gov.tw/dataset/76629) 提供潜在的台湾来源数据；前者 bulk 下载需注册，后者大文件需会员权限。目前没有证明这些目录产品与旧 `tw-http`/`tw/grid` 实际路由、同帧旧输出及许可范围之间的对应关系。RainViewer 官方 [Weather Maps API](https://www.rainviewer.com/api/weather-maps-api.html) 记录的当前接口为近两小时、10 分钟帧和 PNG 瓦片，不能代替旧 WebP/历史灰度配对样本。
- [X] T024 [P] [US1] 在 `crates/radiust-core/src/source/au.rs`、`ca.rs`、`nz.rs` 移植 AU FTP 与 CA/NZ 图片适配器，逐项保持查询、修订、原始字节和能力状态。
- [X] T025 [P] [US1] 在 `crates/radiust-core/src/source/es.rs`、`fr.rs`、`pt.rs` 移植 ES/FR/PT 图片与瓦片适配器，保持产品/站点与色标规则证据。
- [X] T026 [P] [US1] 在 `crates/radiust-core/src/source/id.rs`、`id_sidarma.rs`、`bmkg.rs` 移植 ID/SIDARMA/BMKG 的发现、凭据限制和原始获取；不得读取旧仓库密钥。
- [X] T027 [P] [US1] 在 `crates/radiust-core/src/source/kr.rs`、`my.rs`、`sg.rs` 移植 KR/MY/SG，保留 MY 两站和各来源缺测/显示边界。
- [X] T028 [P] [US1] 在 `crates/radiust-core/src/source/th.rs`、`th_royalrain.rs`、`tw.rs` 移植 TH/RoyalRain/TW；OCR 仅作为 TH 适用路径的系统 Tesseract 可选依赖，保留 TW 两产品。
- [X] T029 [P] [US1] 在 `crates/radiust-core/src/source/tw_http.rs`、`vn.rs`、`cam.rs` 移植 `tw-http`、VN、CAM 的请求规则、帧身份和原始摘要校验。
- [X] T030 [P] [US1] 在 `crates/radiust-core/src/source/opensnow.rs`、`uk.rs`、`wunderground.rs` 移植 OpenSnow、退役 UK、WU 目录/凭据行为；UK 不允许获取请求。
- [X] T031 [P] [US1] 在 `crates/radiust-core/src/source/ph.rs`、`windy.rs` 与 `crates/radiust-core/src/source/browser.rs` 移植浏览器来源，按需驱动系统 Chromium 的隔离临时 profile；覆盖 cookie/CSRF、版本不符、超时和子进程清理。
  - 完成证据：PH/Windy 浏览器 fallback 使用专属临时 profile、随机 loopback CDP 端口、精确 host allow-list、共享 request/host budget、取消/超时 fence 和有界原始响应读取；CSRF selector 安全编码，token header 只注入精确同源目标请求。fake-CDP 合同覆盖版本拒绝、CSRF/header、响应体读取、deadline/cancellation、子进程终止与 profile 清理；完整 Rust workspace 测试通过。
  - 运行证据：在受限 sandbox 外执行 `RADIUST_TEST_ALLOW_LIVE=1 RADIUST_TEST_REAL_BROWSER=1 uv run --offline --no-sync pytest -q tests/live/test_ph_browser_replay.py`，本机 headless Chromium 成功完成真实 CDP handshake；关闭后私有 profile 清理检查通过（1 passed）。测试仅打开 about:blank 并用 loopback CDP，没有访问 provider。PH token/保留原始样本仍在 [US1 offline matrix](../../validation-results/rust-migration-us1-matrix.md) 中明确标为未验收来源证据，不由浏览器 runtime 测试推定通过。
- [X] T032 [P] [US1] 在 `crates/radiust-core/src/source/rainviewer.rs` 移植 RainViewer 的发现、瓦片获取和已验证科学解码，不把其他瓦片来源自动升级为科学可用。
- [X] T033 [US1] 在 `crates/radiust-core/src/source/catalog.rs` 将 T024–T032 的全部 24 个 adapter 接入编译期目录，核对 26 个内置目标、retired/凭据预检及合法离线样本覆盖；排除未登记的 BR adapter。
  - 完成证据：`source_matrix` 将 Rust registry 的 24 个 ID 与编译期 catalog 对齐，并逐项比对 26 个 target；离线合同覆盖 UK retired 优先级、4 个来源凭据预检、21 个其他来源网络禁用、带凭据时秘密不进入报告，以及 inventory 中所有保留 raw artifact fixture 的本地 SHA/manifest replay。没有获准样本或上游错误的来源保持显式 blocked，不制造 fixture；BR 来源不在 catalog。`cargo test -p radiust-core --offline --locked --test source_matrix` 8 项通过。PH/Windy 浏览器 fallback 的实现和运行合同仍由 T031 跟踪。
- [ ] T034 [US1] 在 `crates/radiust-core/src/science.rs`、`crates/radiust-core/src/grid.rs` 与 `crates/radiust-core/src/tiles/` 迁移经验证的解码、`float32`/`uint16` 质量、缺测/透明、nearest/bilinear 重网格和 legacy 显示规则；未验收来源显式拒绝科学输出。
  - 当前进展：Rust legacy display 实现与 Pillow 12.3.0 8-bit separable bicubic 一致的 22-bit 定点系数、两次 pass clipping 与 signed rounding；ID 配对样本逐像素验证 4084×4084→461×461 输出。OpenCV 4.14 NS inpaint 已以依赖-free Rust 实现并保留上游许可证声明。Native comparison 现为 23 条路径中 15 passed、0 difference_pending、8 blocked；15 条已匹配旧样本（含 ES/PT/TW/ID、French 和 OpenCV NS 路径）均逐像素一致。另 8 条因缺少同帧/合法旧输出或 provider artifact 而保持 blocked；完整科学来源 dtype/缺测和其余跨 CRS 转换仍未完成，T034 保持开放。详见 `validation-results/legacy-display.json`。
  - 性能进展：ID 大图 legacy 转换的 Pillow bicubic 两个独立 pass 与最近色 palette 距离扫描改为最多 4 个 Rayon worker；palette 最近索引从 `usize` 收窄到 `u8`，zero-color 分类不再分配未使用的索引数组。固定 4084×4084 ID fixture 仍与旧输出逐像素一致；debug 集成测试优化前单次 8.42 s，优化后 5 次为 4.52/4.50/4.56/4.56/4.59 s，中位数 4.56 s（约快 45.8%，优化前只有单次样本，不作为 p95）。详见 `validation-results/rust-legacy-display-resize.json`。这只覆盖本地显示转换，不代表来源网络吞吐；跨 CRS 投影与完整科学来源覆盖仍未完成。
  - 当前进展：已实现规则网格 nearest/bilinear 与 quality/dBZ 语义，并接入有界 Engine/PyO3。新增 Rust legacy display catalog、指纹/证据校验及 fail-closed 算子执行；15 条 source-matched 本地历史路径现逐像素通过，OpenCV NS 算子已移植且不增加 OpenCV crate/system dependency。另 8 条旧显示路径因缺少可验收样本继续 fail-closed；其余 datum/projection 转换和来源科学解码仍待实现，故任务开放。
  - 坐标转换进展：`Engine::regrid()` 现支持 EPSG:4326↔EPSG:3857 Web Mercator，并在无额外二维暂存数组的情况下逐轴映射目标坐标；PROJ 官方 (2°, 49°) 参考点、双向最近邻、quality 保留、投影范围拒绝及 Engine 集成合同通过。未知投影和 TW EPSG:3821→EPSG:4326 仍 fail-closed；这项通用坐标合同不替代缺失来源匹配样本或 datum transformation 验收。
  - 当前进展：新增 NetCDF4 f32 数据/u16 quality/CF 时间、坐标、CRS、affine 和 provenance 写入及同目录原子发布；本地读取按 `[time,y,x]` 在读取大数组前完成时间消歧，并限帧/像素/临时字节。Rust 往返/多时次测试与 Python xarray/netCDF4 独立读回通过，CLI 可预览选定字段。NetCDF crate 已启用静态构建；macOS 11/15 arm64 wheel 和原生 CLI 均不再链接 Homebrew NetCDF/HDF5。OpenCV inpaint、跨 CRS 投影和完整科学来源覆盖仍未完成，故任务保持开放。
- [X] T036 [P] [US1] 在 `crates/radiust-core/src/output/geotiff.rs` 实现 GeoTIFF 数据/quality/provenance 三件组与可读回 CRS/仿射标签；验证 PROJ `proj.db`、格网及原生依赖随包定位。
  - 完成证据：纯 Rust `tiff` writer 以事务组写 float32 数据、uint16 quality 与 provenance JSON；规则 EPSG:4326/EPSG:3821 网格保留 CRS，非规则 EPSG:4326 仅在球面 Web Mercator 投影后成为规则网格时写成 EPSG:3857，其余拒绝。Rust 测试覆盖朝向、nodata、质量、覆盖替换与 CRS；独立 Rasterio 合同读回 EPSG:3857/EPSG:3821、仿射、Deflate 和 provenance。CLI writer 不依赖 GDAL/PROJ `proj.db`，Rasterio 验证进程清除外部 PROJ/GDAL 路径后使用自身运行时数据完成读回。
- [X] T037 [P] [US1] 在 `crates/radiust-core/src/output/png.rs` 实现 PNG 与 render sidecar 的旧显示语义、尺寸、透明/缺测及来源规则溯源。
  - 完成证据：五项 writer 单测覆盖默认 palette 插值、轴向翻转、质量/缺测透明、Python v1 sidecar 字段及非法选项；RainViewer 合法原始 fixture 已从 Engine 下载/解码并经本地 manifest-last 输出 PNG 与 sidecar，重复执行 skip；Pillow 独立读回 Rust 生成的小图 fixture。其他格式及跨来源科学读回不在本任务范围。
- [X] T038 [P] [US1] 在 `crates/radiust-core/src/output/zarr.rs` 实现无需 Python 的 Zarr v2 写入，固定旧 dtype/chunk/codec 与 consolidated metadata；用 xarray 读回，不以 v3 子集支持的存在代替互操作验证。
  - 完成证据：原生 writer 支持 field/dataset、同目录 staging 与原子目录发布，保持 float32/uint16、512 上限的数据 chunk、legacy Blosc/LZ4 level 5 + byte shuffle、scalar time/CRS 和 consolidated v2 metadata；坐标 chunk 与旧 xarray writer 一样覆盖整条轴。`cargo test --locked -p radiust-core --test zarr` 3 项通过，Python `tests/contract/test_rust_zarr_readback.py` 以 xarray 独立解码 fixture 并核对值、quality、亚秒时间、CRS、dtype、chunk、codec 及 consolidated metadata。Rust serializer 的内部 V2 字段会在发布前规范化移除；覆盖写会原子替换已有 Zarr 目录。
- [X] T039 [US1] 在 `crates/radiust-core/src/cache/index.rs`、`crates/radiust-core/src/cache/mod.rs` 完成旧 `entries` 布局、对象路径、lease、修复、GC 与清理兼容；通过跨版本测试前继续隔离根目录。
  - 完成证据：CacheIndex 只读探测旧 `entries` 布局并在原表 CRUD，不创建 Rust `objects` 表；兼容 Python key-hash `.bin` 路径与带 nonce 的 lease。缓存读回验证 size/SHA；修复、GC、clear 仅触碰索引确认且字节匹配的缓存对象，外部路径、未索引对象和无关临时文件保持不变。基于旧 SQLite/object fixture 的 `cache_compat` 3 项与现有 cache GC 5 项通过。Python/Rust 并发读写尚未验收，缓存根目录继续隔离。
- [X] T040 [US1] 在 `crates/radiust-core/src/storage/local.rs`、`crates/radiust-core/src/storage/manifest.rs` 与 `commit.rs` 实现旧 v1 清单校验、同身份 skip/repair、路径冲突及本地暂存和最终 manifest 发布；同路径锁可跨旧/新入口互斥。
  - 完成证据：Rust 读取 Python 生成的 `tests/fixtures/rust-migration/output/legacy-v1/` 并验证 skip；Python `inspect_manifest()` 独立读回该 fixture。9 项本地提交测试覆盖 skip、损坏修复、冲突保护、overwrite/supersedes、raw-complete 补全、越界/符号链接/路径重叠拒绝和撤回 manifest 后故障回滚；`.radiust.lock` 仍与 Python `flock(LOCK_EX | LOCK_NB)` 共用。SDK 离线 TW 官方 grid fixture 另覆盖 PNG sidecar、NetCDF repair、overwrite generation、corruption repair 与 template conflict；冲突通过 `output_conflict` 稳定错误码返回。
- [ ] T041 [US1] 在 `crates/radiust-core/src/storage/object.rs`、`crates/radiust-core/src/storage/commit.rs` 实现 S3/OSS generation、逐件 SHA-256 读回、最终 pointer manifest、取消 fence 与提交结果未知复核；仅 mock 通过不得声明真实 provider 已验证。
  - 当前进展：新增 `RemoteStore` 并接入 Engine raw-only/解码输出；S3/OSS URI 解析、endpoint/region/凭据配置、immutable generation 上传、逐件流式 SHA/readback、manifest-last、取消 fence、pointer 响应复核和专用 unknown-result 错误已实现。内存对象层覆盖 9 项协议/故障合同。2026-09-29 检查到 AWS credential files/profile、provider URI、provider credentials 与显式 provider opt-in 均未配置；完整 Python 回归中的 3 项真实 provider 测试因此仍按策略跳过。2026-09-30 为 `ObjectStore::read` 增加最多 3 次、短退避的幂等读取重试，并用 loopback S3 HTTP 合同验证暂时性 503 后成功读取；写入与条件发布不做透明重试。此前一次 loopback MinIO 运行因 PUT close/读取请求的间歇 HTTP send 错误未通过；本次用隔离 MinIO server、专用 bucket/config 重跑 `s3_compatible_endpoint_streams_and_reads_back_a_large_artifact`，3 MiB 流式上传/逐字节读回、manifest-last 不覆盖、RemoteStore 事务提交及对象清理全部通过（1 passed）。该 opt-in 测试仍只接受 loopback/test bucket；本地 MinIO 成功不替代真实 AWS S3/阿里云 OSS provider 验收。没有扫描或写入真实远端，真实 provider 验收未完成，任务保持开放。
  - 本轮补强：`read_to_path` / `read_to_path_cancellable` 对暂时性读取错误最多尝试 3 次，每次独立 staging，失败后清理；取消、资源限制、本地 I/O 与永久 provider 错误不重试。`RemoteStore` 的保留对象拷贝、上传后逐件校验及已有 manifest 完整性检查均接入取消感知读回。对象存储故障注入 `--test object_storage` 16 passed/1 ignored；`remote_commit` 模块 10 passed，含 pointer 写入中途收到取消后仍正确返回已提交结果。真实 AWS/OSS provider acceptance 仍开放。
- [X] T042 [US1] 在 `crates/radiust-core/src/output/mod.rs` 与 `crates/radiust-core/src/engine.rs` 串接已验收科学/原图到四种格式及正式成果提交，确保 `fetch` 不提交、raw-only 不解码，处理版本变化不错误跳过旧成果。
  - 完成证据：RainViewer 验收 fixture 已通过 `Engine` 的 PNG+sidecar、NetCDF4、GeoTIFF 三件组与 Zarr v2 下载入口；新增 `validated_science_download_commits_all_four_formats_with_complete_manifests` 端到端合同逐格式检查正式 manifest、artifact size/SHA-256、manifest-last 完整性及 PNG 像素尺寸、NetCDF field、GeoTIFF 数据组和 Zarr dtype/shape。`download_netcdf` 集成套件 11 项通过；已有 `changed_processing_spec_does_not_skip_an_older_decoded_output` 验证处理规格改变时不错误跳过，Rust SDK fetch 合同验证 fetch 不触发 download/commit，raw-only download 合同对无科学解码能力的 fixture 成功落 raw 成果。四种 writer 的独立 Python 读回仍有 fixture 验证。真实 S3/OSS provider、已提交 provider 输出由 Python 独立读回及未验收来源扩展仍分别由 T041/T079 与 source coverage 任务跟踪。
- [X] T043 [US1] 在 `crates/radiust-cli/src/commands/list.rs`、`discover.rs`、`download.rs` 将单来源目录/发现/下载及 dry-run 接到 Engine，保持旧选项、JSON 和退出含义；多来源并发与多帧并发分别留给 US2/US3。
  - 完成证据：`list`、单来源 `discover`、`download --dry-run` 与原生报告已接入 Rust catalog/Engine；CLI 保留来源/产品/站点/时间筛选、raw/raw-only、格式/输出模板、地理处理、cache/storage 参数、JSON 和退出语义。合同覆盖 list JSON/human parity、单来源禁网错误边界、dry-run 不获取/不写输出、配置与显式 format 优先级、unsupported format 的结构化退出，以及参数和凭据脱敏；`cargo test --workspace --offline --locked` 通过。格式默认采用 `output.format` 的已记录兼容差异由合同固定。T045 的 fixture-backed 无 Python 单源流和 24 来源离线矩阵已完成；真实 provider acquisition、未验收来源科学/格式扩展与真实云存储分别仍由 T023/T034、T041 跟踪。
- [X] T044 [US1] 在 `crates/radiust-cli/src/commands/cat.rs`、`doctor.rs`、`config.rs`、`cache.rs` 完成原生预览、来源专用诊断、配置脱敏与缓存管理，非 TTY/quiet/NO_COLOR 保持既有规则；首帧提速留给 US5。
  - 完成证据：doctor/config/cache、凭据脱敏、cache status/gc/clear、GC/clear dry-run（预览不改索引/文件；不存在缓存根时不创建目录；非预览清理需 `--yes`）、`config` 默认 action、`--quiet`/`--verbose`、TTY/NO_COLOR 已有合同。`doctor --network --source ID` 保持旧版 adapter-specific 状态并报告目录 availability，不发出探测；来源诊断按需报告 TH 的 Tesseract、PH 的 token 是否配置（不暴露值）和本机 Chromium 检测状态，以及 Windy HTTP/Playwright 与 Chromium 检测状态。`cat` 恢复 `--palette default`、`--vmin`/`--vmax`、`--width`/`--height`；raw 预览在读取/联网前拒绝科学渲染选项，PNG preview core 与 NetCDF cat 测试核对范围参数和像素行为。`cache_gc` 7 项、CLI `cli_v1` 34 项通过；浏览器 live/provider 合同继续由 T031 跟踪。
- [X] T045 [US1] 在 `crates/radiust-cli/tests/native_e2e.rs`、`crates/radiust-core/tests/source_matrix.rs` 运行七组命令、24 来源与格式/存储合同矩阵，按 [quickstart.md](quickstart.md) 检查无 Python 环境的完整单来源主流程；记录 blocked 科学/真实 provider 项而非勾选通过。
  - 完成证据：`native_e2e` 9 项原生进程合同覆盖 list、discover、download dry-run、local-file cat、doctor、config、cache；`source_matrix` 断言 24 个原生 adapter、26 个目标、禁网/凭据/退役边界，并对保留 fixture 的 19 帧/26 件原始 artifact 校验身份和 SHA 后完成本地 raw manifest 提交/读回。新增 `native_single_source_discover_acquire_decode_and_commit_runs_without_python_or_provider_io` 将 Rust 发现、fixture 获取、科学解码、NetCDF manifest-last 提交、artifact SHA/大小核对及独立字段读回串成单源流程；fixture 仅本地字节，不代表真实 provider。`download_netcdf` 11 项合同通过，其中 RainViewer 四种格式和 TW grid NetCDF 均验证完整 manifest/读回；wheel 四格式 Python 独立读回另见 T079。矩阵与真实 provider/科学 blocked 边界记录于 [rust-migration-us1-matrix.md](../../validation-results/rust-migration-us1-matrix.md)；真实 S3/OSS 仍由 T041 验收，8 条 legacy display blocked 与来源科学/瓦片 evidence 分别由 T023/T034 跟踪。

**Checkpoint**: US1 可独立演示无 Python 原生 CLI；24 个来源均有可查询目录及原有合法能力，旧成果可读回。US2/US3 只改变吞吐与批量调度，不承担补齐 US1 的基本功能。

---

## Phase 4: User Story 2 - 一次快速发现多个来源 (Priority: P1)

**Goal**: `discover all` 与显式多来源共用公平有界调度，所有目标有稳定完整报告，慢/失败来源不阻塞其他来源。

**Independent Test**: 四个固定延迟来源、同主机目标、失败与悬挂目标的离线回放，验证目标覆盖、状态计数、任务上限、超时/中断和参数前置校验。

### Contract and integration tests

- [X] T046 [P] [US2] 在 `crates/radiust-core/tests/discovery_scheduler.rs` 增加四延迟源、同主机争用、重复上游元数据、部分失败/悬挂与公平性的可控回放，断言活动目标≤配置及排序/计数完整。
- [X] T047 [P] [US2] 在 `crates/radiust-cli/tests/discover_multi.rs` 增加 `discover all`/`discover au vn` 的 JSON v1、`query.sources`、无数据/缺凭据/退役、非法组合前置退出 2 和 Ctrl-C 130 合同。

### Implementation

- [X] T048 [US2] 在 `crates/radiust-core/src/config.rs` 加入正整数 `runtime.discovery_workers` 默认 4 与环境/文件/显式覆盖验证；保持现有请求/主机/解码/帧默认值及整批 deadline。
- [X] T049 [US2] 在 `crates/radiust-core/src/discovery.rs` 实现按来源轮转、有界多目标调度及同次元数据请求合并；请求与重定向后主机仍经共享预算，结果按来源/产品/站点稳定排序。
- [X] T050 [US2] 在 `crates/radiust-core/src/discovery.rs` 实现整批单调截止时间与取消状态机：已完成结果保留、运行中 timeout/cancelled、未派发 not_started，迟到结果不可覆盖终态。
- [X] T051 [US2] 在 `crates/radiust-cli/src/commands/discover.rs` 加入 `SOURCE...` 解析及 all/显式列表聚合报告；只允许 latest/max-age，重复来源、all 混用和单源专用条件在网络前退出 2，保留单源旧 envelope。
- [X] T052 [US2] 在 `crates/radiust-cli/src/report.rs` 实现聚合状态 counts、脱敏逐目标错误、单一完整 JSON、退出 0/3/4/5/130 及中断后终端/临时资源收尾。
- [X] T053 [US2] 在 `crates/radiust-cli/tests/discover_multi.rs` 与 `crates/radiust-core/tests/discovery_scheduler.rs` 对照 26 个内置目标及四延迟回放完成独立验收，记录 p95、请求数与并发峰值供 SC-004 最终对照。
  - 完成证据：26 个内置目标的离线 CLI 报告合同通过；固定四延迟源 30 轮对照最近一次 p95 为并发 40.33 ms、串行 delay control 89.18 ms，活跃目标峰值 4，状态完整且跨 seed 语义稳定。该延迟模型没有 HTTP/provider 请求；provider-backed SC-004 仍按 acceptance gate 标为 partial。完整多次结果见 `validation-results/rust-migration-discovery.json`。
  - 完成证据：2026-09-28 显式运行 30 轮四延迟 fixture 验收并写入 `validation-results/rust-migration-discovery.json`。串行延迟控制 p50/p95 为 86.18/88.09 ms；4-worker Engine scheduler 为 38.64/39.87 ms，p95 为串行控制的 45.27%，观测并发峰值为 4；每个目标均有一个终态且跨 seed 语义结果稳定。此 fixture 不产生 provider 请求，不能替代真实来源延迟/请求计数基线。
  - 重复验证：2026-09-29 显式重跑同一 30 轮 ignored 验收，串行 p50/p95 为 84.16/85.12 ms，4-worker scheduler 为 37.64/39.04 ms，p95 为串行对照的 45.87%，最大活动目标仍为 4；语义结果稳定、全部目标有终态。该回合请求数为 0（in-process delay adapter），真实 provider 请求/响应时间仍留 T003/T078。

**Checkpoint**: US2 用 fixture Source 即可独立验收调度；完整内置来源矩阵可在 US1 接入后再运行，不将 p95 单次结果当最终门槛。

---

## Phase 5: User Story 3 - 高效且安全地批量下载 (Priority: P1)

**Goal**: 多帧有界并行，保留输入顺序与已提交成果，停止/中断/同路径冲突不会留下半成品。

**Independent Test**: 多帧合法离线资料、一个失败帧、同一路径和远端最后发布故障注入，比较逐帧状态、正式成果、临时目录和共享资源峰值。

### Contract and integration tests

- [X] T054 [P] [US3] 在 `crates/radiust-core/tests/download_batch.rs` 增加 collect/continue/stop/raise、dry-run、输入顺序、共享限额和取消时未派发状态的离线合同测试。
- [X] T055 [P] [US3] 在 `crates/radiust-core/tests/commit_faults.rs` 增加同路径并发、本地锁、S3/OSS 最后 manifest 失败/响应丢失、旧成果 skip/repair 与 staged 文件清理验证。
  - 完成证据：`commit_faults` 5 项覆盖同路径并发、本地锁、旧成果 skip/损坏修复及 staged copy 清理；`storage::remote_commit::tests` 9 项覆盖 generation manifest 写失败、artifact 读回失败/损坏、pointer 写前/写后响应丢失、取消 fence、旧 generation 修复、raw 补全和同 pointer 并发。失败或 fence 前取消会清理未发布 generation 对象，pointer 结果未知不会误删可能已发布的 generation。`cargo test --workspace --offline --locked` 全部通过。真实 provider 验收继续由 T041 跟踪。

### Implementation

- [X] T056 [US3] 在 `crates/radiust-core/src/download.rs` 实现按 `frame_concurrency` 派发的帧级任务、获取/解码/提交共享资源限额和输入索引结果收集；不把已有 `fetch_many` 并发误用于旧串行 download 状态。
  - 完成证据：raw-only 与 PNG/NetCDF/GeoTIFF/Zarr 的获取、解码和编码按 frame concurrency 有界运行；解码/编码共享 Engine CPU worker，正式提交通过 Engine 共享的单提交 worker 离开异步执行器并避免同 Engine 调用争抢非阻塞 flock；结果按输入索引收集，Stop 保留已提交前缀。worker 上限、Stop/Collect 和四格式读回/下载合同通过。独立 Engine/进程之间的锁语义与进程树 RSS 由 T055/T058/T061 验收。
- [X] T057 [US3] 在 `crates/radiust-core/src/download.rs` 实现 collect/continue 与 stop/raise 的停止派发、运行中取消、已完成结果保留及逐帧安全错误；dry-run 只产生 planned，不获取或提交。
  - 完成证据：raw 获取 stop/raise 会取消失败边界之后的活动任务、阻止启动队列项，并按输入顺序保留已提交前缀；PNG 有序准备在 stop 后取消后续准备并阻止其提交。新增门闩 fixture 覆盖慢前缀、失败帧、活动取消、排队未启动、临时文件清理与 manifest 前缀；workspace `download_batch` 6 项通过。Tokio 已启动的 `spawn_blocking` 解码 CPU 无法强制中止，但取消后结果不会提交。
- [X] T058 [US3] 在 `crates/radiust-core/src/storage/local.rs` 与 `crates/radiust-core/src/storage/commit.rs` 将同路径写入串行化并对最终 manifest 设提交 fence；远端结果未知须读回确认或显式报 unknown，不盲目覆盖重试。
  - 完成证据：LocalStore 按规范化输出根目录共享进程内 gate，随后通过与旧 Python store 兼容的持久 `.radiust.lock`/`flock` 跨进程串行化，并以 manifest-last 发布；RootLock 子进程冲突测试与 `commit_faults` 同路径并发合同验证完整成果与唯一 generation。RemoteStore 按 pointer key 串行同进程提交，发布前复核旧 pointer；pointer 写后响应丢失时读回确认，否则明确返回不可重试的 `CommitOutcomeUnknown`，不盲目重试。`commit_faults` 5 项及 `remote_commit` 9 项故障合同通过（`cargo test --workspace --offline --locked`）。真实 S3/OSS 与跨进程远端 CAS 验证仍由 T041 跟踪。
- [X] T059 [US3] 在 `crates/radiust-core/src/transport/http.rs` 与 `crates/radiust-core/src/storage/object.rs` 将大 artifact 的获取/提交改为有界流式落盘和边写边 SHA-256，避免整帧重复分配；验证峰值 RSS/临时盘不超过基线。
  - 完成证据：HTTP artifact 的 `get_to_path` 分块写入临时文件并边写边算 SHA-256；`ObjectStore::write_path_cancellable` 以 64 KiB chunk 从 staged 文件流式上传并增量计数/哈希，取消或超限会中止未完成写入。`object_storage` 的流式、摘要、超限和取消合同通过。`scripts/validation/benchmark_large_artifact.py` 以 127.0.0.1 本地 fixture 对比旧 Python `HTTPTransport.get_sync` 缓冲路径和 Rust HTTP+OpenDAL 文件 sink：10 对、每次 4×8 MiB，所有请求、文件 SHA-256 和清理检查通过。wait4 直接子进程峰值 RSS max Python/Rust 为 113,950,720/19,152,896 bytes；worker 内观测临时盘 max 两侧均为 33,554,432 bytes（p50 Python/Rust 为 25,165,824/33,554,432 bytes），退出后两侧临时目录为空。Rust/Python p50 为 0.189/0.324 s；Rust p95 为 0.439 s，受单次启动/调度抖动影响而高于 Python 0.335 s。本任务只据内存/临时盘和完整性判据验收，不声称 p95 吞吐改善。详细结果见 `validation-results/rust-migration-large-artifact.json`。该合成传输/文件 sink 对照不替代 T003/T078 的历史 CLI 批量采集基线。
- [X] T060 [US3] 在 `crates/radiust-cli/src/main.rs` 与 `crates/radiust-cli/src/report.rs` 接入有序批量报告、部分成功退出码和中断 130；对旧 CLI 丢弃 `BatchError.partial_result` 的差异写入 `docs/cli.md`。
  - 完成证据：原生入口将发现和 raw-only/PNG/NetCDF/GeoTIFF/Zarr 执行结果合并为输入顺序逐帧报告，部分成功退出 4，中断保留已完成帧并退出 130；新增中断 partial report 合同测试，现有报告测试覆盖发现失败与下载成功的混合结果。Python Click 入口仍会丢弃异常中的 `BatchError.partial_result`，差异已写入 `docs/cli.md`。
- [X] T061 [US3] 在 `crates/radiust-core/tests/download_batch.rs`、`crates/radiust-core/tests/commit_faults.rs` 与 `validation-results/rust-migration-batch.json` 完成并发/故障独立验收，记录成果校验、进程树 RSS 和遗留临时文件/子进程为零。
  - 完成证据：loopback-only 手动基准完成 30 轮 × 16 帧、共 480 个本地 HTTP 请求；观测并发始终为 4（配置上限 4），每轮核对输入顺序、manifest/artifact SHA-256，下载与提交 p50/p95 为 0.6633/0.6872 秒。采样进程树峰值 RSS 23,511,040 bytes；隔离 TMPDIR 峰值 1,217,254 bytes/52 个文件，退出后目录为空、进程组无残留。`commit_faults` 的 5 项故障/并发合同及 workspace `download_batch` 常规合同通过。该小型 64 KiB artifact 样本验证批量并发、顺序和清理；T059 另行验证大 artifact 的 buffered/streamed 内存与暂存盘对照。

**Checkpoint**: US3 可由 fixture Source 独立验证并发与提交，不需要等待全部真实来源；正式成果完整性优先于吞吐。

---

## Phase 6: User Story 4 - Python 科学工作流按需转换 (Priority: P2)

**Goal**: 基础 Python 包通过绑定使用核心且不强制加载 xarray，显式转换保持科学语义，旧 SDK/插件用户有明确迁移路径。

**Independent Test**: 在源码树外分别安装基础 wheel 与可选科学依赖，调用同一合法离线资料的发现/获取/下载和 `to_xarray()`，核对值、质量、坐标、时间及命令行为。

### Contract and integration tests

- [X] T062 [P] [US4] 在 `tests/contract/test_rust_binding_sdk.py` 增加绑定对象生命周期、同步/异步错误、fetch 不正式提交、部分结果和可选 `to_xarray()` 的科学读回合同。
  - 完成证据：绑定 FrameRef/Report/Field 在所有者释放后仍可读；sync/async public fetch 的字段在 Client 关闭后仍可读取 value、quality、metadata，调用轨迹仅包含 discover/fetch_raw/decode_science，不触发 download/commit。sync/async 歧义、网络权限错误、stop 部分结果、可选 xarray 值/坐标/时间/缺测读回均有合同；该文件 17 项通过，Ruff 通过。
- [X] T063 [P] [US4] 在 `tests/packaging/test_installed_wheel.py` 加入无 xarray 基础安装与可选科学安装的源码树外 smoke，验证 wheel 控制台命令与原生 CLI 的 JSON/退出码一致。
  - 完成证据：macOS arm64 CPython 3.12 的修复 wheel 在源码树外 venv 分别以 core 和 zarr+science extra 安装；基础模式断言未安装 xarray/NumPy，顶层 RadarField/RadarDataset 为 binding；可选模式执行 xarray 数据、quality 和坐标读回。两种模式均验证 console 与 Rust CLI JSON/退出码一致，分别 2 项通过。wheel 经 delocate 内置 NetCDF/HDF5 等 dylib。

### Implementation

- [X] T064 [US4] 在 `crates/radiust-core/src/python.rs` 暴露 Engine/Query/FrameRef/RadarField/Dataset/Report 的薄绑定、异步/同步生命周期及稳定错误映射；大数组由核心持有至显式转换。
  - 验证：PyEngine 持有共享 `Arc<Engine>` 复用 transport/runtime，提供 terminal `cancel()`/`cancelled`；Query、FrameRef、RadarField、Dataset、DiscoveryReport、RawFrame、fetch/download reports 及 raw-only、PNG、NetCDF、GeoTIFF、Zarr v2 下载均已绑定，大数组保留在 Rust 侧至显式转换。CPython 3.12 arm64 extension wheel 可构建并安装；13 项 Python binding 合同通过，网络权限/下载 dry-run/partial report 等错误边界有测试。
- [X] T065 [US4] 在 `python/radiust/client.py`、`python/radiust/api.py` 与 `python/radiust/_bridge.py` 将发现/获取/下载和批量便捷调用改为统一 Rust Engine；`fetch` 默认返回绑定对象，保留一帧歧义与 on_error 语义。
  - 完成证据：公共 Client/AsyncClient 与便捷 API 复用 CoreEngineSession；discover、raw acquire、科学解码、批量获取及 raw-only/PNG/NetCDF/GeoTIFF/Zarr 下载经 Rust Engine 执行。批量 SDK 新增 `Engine::fetch_many_decoded` 与 PyO3 `fetch_many_decoded`，在 Rust 完成有界获取、共享解码 worker 调度和有序逐帧状态/错误报告；Python 仅映射报告对象。缓存、manifest-last、S3/OSS、处理参数、本地四格式 writer 和进度事件沿用 Rust Engine 路径。`tests/contract/test_batch_sdk.py` 与 `tests/integration/test_pipeline_progress.py` 覆盖 sync/async 离线科学批量读回、输入顺序、部分结果及禁止 Python 再次解码。内存写入远端、非空 `encoder_options` 和来源/几何覆盖仍不支持，不回退 Python pipeline。
- [X] T066 [US4] 在 `python/radiust/science_adapters.py` 与 `python/radiust/outputs/zarr.py` 实现显式 `to_xarray()`、可选 Python Zarr 操作和缺依赖提示；转换保持 `float32`、`uint16`、坐标、UTC 时间与缺测。
  - 完成证据：Rust `RadarField`/`RadarDataset` 只在显式 `to_xarray()` 时复制为 NumPy/xarray，并携带时间、坐标、CRS、provenance 与 uint16 quality；Python Zarr writer 显式固定 v2 和 consolidated metadata。Rust binding readback、xarray 数据集、zarr 读回及缺失 xarray/Zarr 依赖提示合同通过。
- [X] T067 [US4] 在 `pyproject.toml`、`python/radiust/cli/main.py` 与 `python/radiust/__main__.py` 将基础依赖缩为绑定所需、xarray/Zarr 移入可选依赖，并让 wheel console entry 转发到原生命令实现；移除运行时 `radiust.sources` 加载路径。
  - 完成证据：基础依赖仅保留 PyYAML，science/GeoTIFF/Zarr 等进入 extras；`radiust` console 与 `python -m radiust` 调用 PyO3 `cli_main` 并转发到 radiust-cli `run_args`；registry 不再读取 `radiust.sources` entry point。macOS wheel CI 使用 delocate 将 NetCDF/HDF5 dylib 修复进 wheel，基础/可选安装 smoke 已通过。
- [X] T068 [US4] 在 `docs/python-sdk.md` 与 `docs/source-development.md` 写出旧默认 xarray 对象、Python entry-point 插件到显式转换/编译期 Rust Source 的迁移示例，说明破坏性行为及重新构建步骤。
- [X] T069 [US4] 在 `tests/contract/test_rust_binding_sdk.py` 与 `tests/packaging/test_installed_wheel.py` 完成基础/可选依赖双环境独立验收，记录 Python wheel 在干净 macOS arm64 环境的实际运行结果。
  - 完成证据：本机 CPython 3.12.7 macOS arm64 修复 wheel 在源码树外 venv 安装/运行；core 与 zarr+science 模式均通过。当前本机 wheel 标签为 `macosx_15_0_arm64`，实测只证明 macOS 15+ arm64；更低系统版本须以对应 CI runner 构建/标签验证。

**Checkpoint**: US4 的 Python API 直接使用与原生 CLI 相同的核心行为；旧插件和默认返回值变化已清楚记录，不提供隐式 Python 回退。

---

## Phase 7: User Story 5 - 从已有资料快速预览首帧 (Priority: P2)

**Goal**: 原图与本地文件的选定帧尽快显示，保留 raw/legacy/decoded 区分和歧义处理。

**Independent Test**: 相同代表性原图和多时次 NetCDF 的离线回放，核对首帧身份/像素/终端状态，并测同机 30 次 p95。

### Contract and integration tests

- [X] T070 [P] [US5] 在 `crates/radiust-cli/tests/cat_first_frame.rs` 与原生 CLI 合同中加入唯一 raw、legacy 规则、本地多时次/歧义、非 TTY text、取消及终端恢复的可见结果测试。
  - 完成证据：source-cat 唯一帧只 fetch 一次；French `FRCOMP` 与 ID historical raw/gray 配对样本分别通过已验证 legacy 规则逐像素匹配冻结灰度 fixture；本地多时次 NetCDF 与歧义、非 TTY text/JSON/quiet、文件模式 legacy 来源身份边界均有进程测试。CLI 单测验证取消 envelope/exit 130、发现与获取中断后的临时文件清理；新增 Unix PTY 子进程验证 TTY SGR 恢复、`NO_COLOR` 无转义序列且不切换 alternate-screen/cursor 模式。核心 legacy 合同 4 项、CLI 单元与集成合同共 54 项及 Python console/module 转发回归 10 项通过。依赖 OpenCV 的其余历史显示路径以及 science/grid 全来源覆盖仍由 T034 跟踪并保持 fail-closed。
- [X] T071 [P] [US5] 在 `crates/radiust-core/tests/preview_semantics.rs` 加入原图/legacy 逐像素、未知时间、缺科学证据拒绝和只选择所需帧的读取计数测试。
  - 完成证据：AU 原始 PNG 与固定 legacy 灰度成品分别逐像素校验 RGBA SHA-256；仅时间戳文件名不生成 FrameRef 身份；无验收科学能力的来源拒绝解码；本地注入 adapter 的两帧 `latest` 只 fetch 一次且身份匹配。新增 source artifact receipt 合同验证 image media type、尺寸和 SHA-256 一致才允许预览；`preview_semantics` 7 项通过。当前无 legacy 转换 API 或多时次 NetCDF reader，因此测试验证成品预览与 fetch 次数，不声称验证转换像素或数组读取次数。

### Implementation

- [X] T072 [US5] 在 `crates/radiust-core/src/preview.rs` 使原图路径只获取/解码唯一选中的显示帧，不启动科学解码或预取无关帧；保留来源/产品/时间身份和 raw/legacy 模式。
  - 完成证据：source `cat` 在候选不唯一时于获取前退出；唯一候选仅获取其完整 raw artifact set，单图直接预览、RainViewer/Windy 只拼接通过 identity 与 receipt 验证的固定瓦片，预览保留原 FrameRef identity 和 `Raw` 模式，未调用科学解码或预取。Rust 合同证明 bytes/file 超过 `max_temp_bytes` 时在图像解码前拒绝，CLI 普通图片有合同验证会遵守配置值，来源图片也将该值传入 preview limits。
- [X] T073 [US5] 在 `crates/radiust-core/src/output/netcdf.rs` 实现本地文件按显式时间/变量只读取选定帧；多帧未消歧时在读取大数组前报告歧义。
  - 完成证据：读取器要求明确变量；`[time,y,x]` 按 `--at` 先查时间坐标，再对 field/quality 发起切片读取；未指定且多时次时，在尝试读取字段数组前返回歧义。多时次切片和 string 类型大数组歧义测试通过，并接入 Rust CLI NetCDF `cat`。
- [X] T074 [US5] 在 `crates/radiust-cli/src/commands/cat.rs` 与 `crates/radiust-cli/src/terminal.rs` 优化首帧显示路径，保留图形/text、非 TTY、NO_COLOR、quiet 与取消后终端恢复规则。
  - 完成证据：本地 PNG 和选时 NetCDF `cat` 在读完/选定帧后才渲染；`auto` 只在 TTY 且未设置 `NO_COLOR` 时发 ANSI，`text` 与管道输出保持文字；全局 `--quiet` 不隐藏预览、错误或 JSON。来源发现/获取取消在渲染前返回中断报告，ANSI renderer 每行恢复 SGR 状态且不切换 raw/alternate-screen 模式。CLI 合同覆盖 quiet、非 TTY、JSON 和取消清理；PTY smoke 验证 `NO_COLOR` 有无时的实际输出。
  - 限制：NetCDF C 读取是同步调用，不能在读取内部协作式取消；该路径在输出前完成本地读取，也不更改终端模式。来源发现/获取本身的延迟由全场景 T078 跟踪。
- [X] T075 [US5] 在 `crates/radiust-cli/tests/cat_first_frame.rs`、`crates/radiust-core/tests/preview_semantics.rs` 与 `validation-results/rust-migration-preview.json` 完成语义与同机首帧 p95 独立验收，记录较旧版至少降低 20% 或未达标原因。
  - 完成证据：同一 macOS arm64 主机上对 256×256 不透明 Windy PNG (`9052bae7ccf8af41e8046c0c2f1abe69f30803861bded455424ce9c9d3acef09`) 做 30 对受控 80×24 PTY ANSI 渲染。Python/Rust 48×24 终端格首帧输出 p95 为 311.49/32.64 ms，Rust 降低 89.52%（9.54×）；30 对全部正确且 RGB/glyph 像素摘要相同（`be7259c7427031760f32fa32da3578d4e5623470d6a1c3baf55f37e16c650ee5`）。首帧定义为 PTY reader 收齐 1,152 格，测量不含终端模拟器实际绘制延迟。详见 `validation-results/rust-migration-preview.json`。

**Checkpoint**: US5 的身份、像素和科学边界仍正确，且有重复测量的首帧延迟证据。

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: 关闭跨故事的文档、发布、安全、性能与科学证据门槛。

- [X] T076 [P] 在 `docs/cli.md`、`docs/installation.md` 与 `docs/architecture.md` 更新原生安装、七组命令、多来源发现配置、状态/退出码和未来桌面复用边界；明确已批准与未批准的行为变化。
- [X] T077 [P] 在 `docs/output-maintenance.md`、`docs/migration.md` 与 `migration/release-readiness.md` 记录缓存隔离/同根策略、旧 v1 成果读回、来源科学 blocked、真实 provider 未验收项及处理版本升级规则。
- [X] T078 在 `scripts/validation/benchmark_rust_migration.py` 与 `validation-results/rust-migration-comparison.json` 运行相同 macOS arm64/样本/冷热配置的每场景≥30 次新旧对照，输出 p50/p95、进程树 RSS、请求数、临时盘峰值并逐项判定 SC-003～SC-006；未达标先定位热点，不修改正确性门槛。
  - 完成证据：当前 release CLI 与 Git HEAD 旧 Python 树对四个 CLI 和七个 loopback 场景均完成 30 对，`baseline_complete=true`。SC-003 list/discover-all p95 比 0.0725/0.0398；SC-004 四来源 cold/warm 比 0.1935/0.1927，请求与结果合同通过、native 峰值并发 4；SC-005 TTY ANSI 首帧比 0.1048，见 T075；SC-006 进程树 RSS 峰值 Python/native 为 214,335,488/12,304,384 bytes，临时盘峰值 12,288/0 bytes，并发不超过 4、loopback 暂存文件全部清理。逐样本和环境指纹见 `validation-results/rust-migration-comparison.json`。公开 provider RTT 不在 loopback 测量范围内。
- [X] T079 在 `.github/workflows/wheels.yml`、`scripts/validation/check_macos_release.py` 与 `validation-results/rust-migration-packaging.json` 完成原生 CLI/wheel 干净 macOS arm64 安装、四格式读回、动态库/`proj.db` 等资源定位检查；不得依赖开发机 Homebrew 路径。
  - 完成证据：最终 macOS 11 arm64 wheel SHA-256 `94f4a11b90dda7f90fb591eb83a735c6e6483171fa04278bf942faa821ac3b90` 经 `check_macos_release.py` clean install 与 exact-wheel 四格式独立读回；24-source CLI smoke、Mach-O 系统依赖、`proj.db` 隔离 venv 定位均通过，报告状态为 passed。完整 wheel 内容合同与 Python 回归也针对该 wheel 通过。
  - 平台范围：wheel tag 与 Mach-O deployment target 为 macOS 11 arm64；执行验证的主机为 macOS 26.6.2 arm64。实际 macOS 11 主机与 GitHub source-build workflow 未运行，作为未覆盖平台记录，不代表本地 wheel 检查失败。
- [X] T080 在 `crates/radiust-core/src/config.rs`、`crates/radiust-core/tests/runtime_lifecycle.rs`、`http_metadata.rs`、`ftp.rs`、`object_storage.rs`、`discovery_scheduler.rs`、`download_batch.rs` 与 `crates/radiust-cli/tests/cli_v1.rs` 复核禁网、跳转后主机限额、凭据/控制字符脱敏、大小/时间限制及 Chromium/Tesseract 缺席路径；取消/限额测试验证临时文件清理。
- [X] T081 在 `validation-results/rust-migration-acceptance.md` 按 [quickstart.md](quickstart.md) 汇总 SC-001～SC-008 的命令、版本、样本、结果与未运行/blocked 项；真实 S3/OSS 和缺科学证据来源不得由 mock 或目录覆盖推定通过。
- [ ] T082 在 `Cargo.toml`、`Cargo.lock`、`pyproject.toml` 与 `.github/workflows/offline.yml` 运行格式、lint、core/CLI 与 Python 离线回归、打包工作流；仅在检查通过且 T081 无未关闭的必需项时标记迁移完成。
  - 2026-09-30 当前源码复验：新增单源 Rust E2E、WU fail-closed 与 S3 暂时性读取重试合同后 `cargo test --workspace --offline --locked` 通过（core 267 passed/1 ignored；全部 CLI、integration 与 doc tests 无失败；对象存储合同 12 passed/1 个 opt-in ignored）。追加的重投影验证 `grid::tests` 9/9、`download_netcdf` 12/12 通过，覆盖 EPSG:4326↔EPSG:3857 Engine 采样并保留对不支持的 TW EPSG:3821→EPSG:4326 的拒绝。`cargo fmt --all -- --check`、`cargo clippy --workspace --offline --locked --all-targets`、`uv run --offline --no-sync ruff check python tests scripts/validation`、`git diff --check` 均通过，Clippy 保留既有 warnings。精确 macOS 11 arm64 wheel 的 Python 全量回归为 283 passed/24 个 opt-in live/provider skipped，clean-install/四格式 package audit 通过。用户确认目前没有来源匹配样本，也没有可用于真实 AWS/S3-compatible/OSS 验收的 endpoint/prefix；T045 已完成，T023/T034/T041 的来源科学/瓦片证据与真实 S3/OSS 尚未验收，实际 macOS 11 主机及 GitHub source-build workflow 也未运行，因此依任务条件 T082 保持开放。
  - 延续复验：当前 macOS 11 arm64 wheel SHA-256 `da67cacac7ddc278d706a272ab3c627a3c9c8074127a78daf9845c455962a728` 通过 `check_macos_release.py --with-format-readback`，包括隔离 extras 安装、native extension/24-source CLI、Mach-O、`proj.db` 与四格式读回（4 passed）；同 wheel 的 Python 全量回归 283 passed/24 skipped，独立 native GeoTIFF/Zarr CLI smoke 通过。`cargo fmt --all -- --check`、Ruff、Python compile 与 `git diff --check` 通过。此证据不关闭上述材料缺失的 T023/T034/T041，也不满足 T082 的未关闭必需项条件。
  - 本轮复验：`cargo test --workspace --offline --locked` 全部通过（core 268 passed/1 ignored，CLI、集成与 doc tests 无失败）；`cargo fmt --all -- --check`、`cargo clippy --workspace --offline --locked --all-targets`（有既有 warnings）及 `git diff --check` 通过。按最新 Rust core 源码构建的 macOS 11 arm64 wheel SHA-256 `e73409e0079ede97fda1b207a4d6c1aeb89f31583f8391c88c796e2c6b0d75c2` 通过 clean install/extension/24-source CLI/Mach-O/`proj.db` 与四格式独立读回（4 passed），完整记录见 `validation-results/rust-migration-post-retry-wheel-audit.json`。`PYTEST_RADIUST_WHEEL=<该 wheel> uv run --offline --no-sync pytest -q` 全套 283 passed/24 个 opt-in live/provider skipped。未显式指定 wheel 的一次本地 run 选中旧 `validation-results/wheel` 并失败；对当前 exact wheel 的测试通过。wheel 审计运行于 macOS 26.6.2 arm64；尚未在实际 macOS 11 主机运行。T023/T034 缺来源匹配证据、T041 缺真实 AWS/OSS endpoint，仍使 T082 final gate 保持开放。
- [X] T083 [US4] 按 FR-010/FR-013 与 plan 的 Python 薄绑定边界，清理或停止打包 `python/radiust/` 中仍承担来源发现/获取、科学解码、显示、存储、传输和输出业务的旧 Python 实现；保留 Rust CLI 转发、必要 SDK/绑定 facade、显式 xarray 转换及可选 Python Zarr 适配。迁移依赖旧实现的测试到 Rust 外部行为或绑定合同，并通过基础 wheel 内容审计确认旧业务模块不再随包发布（contradicts）。
  - 完成证据：Rust CLI 入口由 `radiust.cli.main` 转发；旧 provider adapters、Python discovery/pipeline/batch、display、transport、storage 与 PNG/NetCDF/GeoTIFF output 模块均已从 wheel 排除，源码中的 provider adapters/discovery/pipeline/batch 已删除。最终 macOS 11 wheel SHA-256 `94f4a11b90dda7f90fb591eb83a735c6e6483171fa04278bf942faa821ac3b90` 仅含 22 个 Python 模块，包括 Rust SDK/CLI facade、Rust catalog metadata facade、显式 xarray adapter 和可选 `outputs.zarr`。
  - 完成证据：针对这个 exact wheel 的 AST 审计检查 75 个 Python 测试文件，0 个文件、0 条直接 import 引用了 wheel 排除模块，0 个 AST parse error；wheel 安装合同与完整离线 Python 回归通过（283 passed、24 个 opt-in live/provider 跳过）。四格式 exact-wheel 独立读回另见 `validation-results/rust-migration-packaging.json`。源码中保留的少数兼容 DTO/geometry wrappers 不承担 CLI/provider/output 执行业务；未验收来源与真实 provider 门槛继续由 T023/T031/T034/T041 跟踪。

## Phase 9: Convergence

- [X] T084 [US5] 在 `crates/radiust-core/src/output/` 与 `crates/radiust-cli/src/lib.rs` 为 Rust 生成的 GeoTIFF 和 Zarr v2 本地成果实现无 Python 读取/预览；多变量或多时次资料须在读取大数组前要求明确选择，并保留值、quality、时间、CRS 与所选帧身份；在 `crates/radiust-cli/tests/cat_first_frame.rs` 和核心格式合同中覆盖读回、歧义、损坏及资源限制 per FR-003/US5/AC2。
  - 完成证据：GeoTIFF reader 核对 data/quality/provenance 三件组、CRS/仿射、时间/变量身份、sidecar 与文件/像元/帧预算；Zarr v2 reader 先解析 consolidated 元数据并在读取大数据 chunk 前拒绝未选择的多变量 store，恢复值、quality、坐标、CRS、affine、time 与 provenance，验证完整 data/quality/vector chunk、symlink 和字节/像元/帧预算。Zarr `time`/`crs` scalar 的 v2 零 fill 可不写 chunk；writer 明确写出 data/quality fill chunk。CLI `cat --file` 支持两种 Rust 成果的文本/图像预览与 selector 校验，覆盖歧义、错误变量/时间、缺 sidecar。`cargo test --workspace --offline --locked` 通过；GeoTIFF core 8/8、Zarr core 9/9、`cat_first_frame` 11/11。当前源码 macOS 11 arm64 wheel（SHA-256 `da67cacac7ddc278d706a272ab3c627a3c9c8074127a78daf9845c455962a728`）在隔离安装环境中通过 `cat` GeoTIFF/Zarr smoke、Mach-O 与四格式独立读回（4/4）；报告见 `validation-results/rust-migration-current-continuation-wheel-audit.json`。
- [X] T085 [US5] 修正 `scripts/validation/benchmark_rust_migration.py` 的 SC-005 场景，使配对基准测量实际预览 renderer 向受控 TTY 输出所选首帧的时间，而不是 `--renderer text` 的首条摘要；以同一图像、Python/Rust 各至少 30 次、核对身份和像素，并更新 `validation-results/rust-migration-comparison.json` 与 `rust-migration-preview.json`；只有达到 SC-005 的 20% p95 改善才报告通过 per SC-005/T075。
  - 完成证据：两侧均以 `--renderer ansi --width 48 --height 24` 输出至由 PTY 读取器驱动、窗口设为 80×24 的真实 TTY。样本记录首次完整 48×24 色格抵达的时间与逐格 foreground/background RGB 和 glyph；独立从固定 PNG source pixels 计算的预期网格、两端帧 SHA 与逐对语义指纹均一致。正式 30 对 p95 为 311.49 ms/32.64 ms，Rust 降低 89.52%，SC-005 通过。报告披露测的是 PTY 输出完成，不是终端模拟器 paint 时间；无公网上游请求。
