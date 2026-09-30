# Rust 核心迁移与性能优化计划

> 状态：实施中，US1–US5 尚未完成。现有来源的科学验收状态仍以 [docs/migration.md](docs/migration.md) 和 `migration/` 台账为准。本文规划的是代码归属与性能迁移，不会把可获取的原图自动认定为经过验证的科学数据。

## 目标和交付边界

- 交付独立运行的 macOS arm64 `radiust` Rust CLI。`radiust-core` 同时供 CLI、PyO3 绑定和后续桌面应用使用；本轮不制作桌面 UI、守护进程、IPC 或 Swift 绑定。设计参考 [Omastorm 的引擎与界面分层](https://github.com/wesleygrimes/omastorm/blob/main/DESIGN.md)，但当前交付物是可嵌入的 Rust 库。
- 把现有 CLI 功能、24 个内置来源、获取和科学处理、预览、缓存与正式输出的实现迁到 Rust。Python 包保留薄绑定和可选的 xarray/Zarr 适配；原生 CLI 的 `--format zarr` 仍由 Rust 实现，无需 Python。
- Python `fetch` 默认返回 Rust 绑定的数据场/数据集；需要 xarray 时显式调用 `to_xarray()`。旧 Python `radiust.sources` entry-point 插件接口退出，新增来源实现编译期 Rust `Source` trait 并编译进 CLI。提供 API 迁移说明。
- 首轮发布和性能验收只针对 macOS arm64。现有其他平台测试可以继续运行，但不作为首轮发布门槛。

## 当前事实与迁移顺序

Rust CLI 已原生实现 `list`、`discover`、`download`、`cat`、`doctor`、`config` 和 `cache` 七组命令；Python console 与 `python -m radiust` 转发到该 CLI。Rust registry 有 24 个 source ID、26 个 discovery target 和 24 个编译期 adapter。PH HTTP timeline、Windy latest HTTP discovery 与 raw tile 获取已迁移；headless Chromium/loopback CDP runtime 已本机验证，但测试只打开 about:blank，没有访问 provider，因此 PH/Windy provider artifact 与 credentialed 验收仍缺。未验收的 science 与几何能力继续 fail-closed。

多来源发现按 `runtime.discovery_workers` 有界打乱派发，最终报告保持确定顺序；HTTP/FTP 获取以有界缓冲落盘并增量摘要。RainViewer composite 与 TW grid 的 Rust decoder 有数值/quality fixture 对照，当前正式解码输出支持 PNG、NetCDF4、GeoTIFF、Zarr v2，并通过本地 manifest-last 提交；四格式均有独立读回。`cat SOURCE` 只获取唯一选中帧，本地 NetCDF `cat --file` 支持变量和时间选择，TTY 原图预览保持 raw/legacy 与科学解码边界。原生 `download` 已恢复 variable/grid/bbox/resolution/resampling 参数；RainViewer EPSG:4326 到 geographic EPSG:4326 可用 Rust 有界重网格并记录 processing identity。核心重网格现支持 EPSG:4326↔EPSG:3857 Web Mercator 坐标转换；其他 datum/projection 转换（包括 TW EPSG:3821 到 EPSG:4326）仍明确拒绝，不会静默输出原网格。

Rust 本地输出通过旧兼容 `flock`、共享进程 gate 和 manifest-last fence；远端 generation/pointer 提交具备逐件校验、故障清理、取消 fence 和结果未知复核。内存对象层测试通过，但真实 AWS S3/OSS provider 与远端跨进程 CAS 尚未验收。Python `Client`/`AsyncClient` 复用 Rust Engine；绑定 `RadarField`/`RadarDataset`、发现/获取/下载和四格式本地 `write()` 已接通。SDK 下载的 `raw=True` 可随四种解码格式在一次提交中附带原始 artifact，`variable/grid/bbox/resolution/resampling` 由 PyO3 传入 Core 处理；`encoder_options` 仍未迁移。Python 仅在显式 xarray 转换和可选 Zarr 互操作处参与；内存 `write()` 需要 FrameRef、字段/帧时间一致，目前拒绝 raw、远端 URI 与地理重网格。

SDK `iter_fetch`/`aiter_fetch` 的 bounded fetch/decode workers 已由 Rust `DecodedFetchStream` 持有：Tokio task 有界并发、完成顺序产出、返回一项前补位，取消和 `raise` partial 状态由 Rust 管理。Python `streaming.py` 保留查询输入解析、结果转换与迭代 facade；collect 模式不在 stream 中留存已返回 payload。Rust stream 单测、Python batch/stream/facade 合同已通过；T083 的旧 Python 业务实现清理、wheel 边界和导入审计已完成。

### 当前实现边界

- **当前复验：** 2026-09-30 的 `cargo test --workspace --offline --locked` 全部通过，core unit tests 为 264 passed、1 ignored，CLI、integration 与 doc tests 无失败；`cargo fmt --all -- --check`、`cargo clippy --workspace --offline --locked --all-targets`、Ruff 与 `git diff --check` 通过。用精确 macOS 11 arm64 wheel（SHA-256 `94f4a11b90dda7f90fb591eb83a735c6e6483171fa04278bf942faa821ac3b90`）运行的 Python 回归为 283 passed、24 个 opt-in live/provider skipped；clean install、Mach-O/资源定位和四格式读回通过。T083 已完成，wheel 保留 22 个 Python facade/adapter 模块；75 个 Python 测试文件没有直接 import wheel 排除模块。
- **性能：** 完整同机对照报告有 30 组配对样本，覆盖四个 CLI 场景和七个 loopback 冷/热、延迟来源、批量获取场景，`baseline_complete=true`。Native/Python p95 比为 list 0.119、offline discover-all 0.087、四来源 cold/warm 0.174/0.173；本地首帧 cat p95 降低 85.3%。样本记录了请求数、进程树 RSS 和临时盘峰值；结果不代表公网 provider RTT。详见 [性能对照](validation-results/rust-migration-comparison.json) 和[验收报告](validation-results/rust-migration-acceptance.md)。
- **来源和格式矩阵：** 24 个 adapter、26 个 discovery target、19 个保留 fixture frame/26 个 artifact 的身份、摘要与本地 raw commit 均有离线合同。T045 的 native CLI 进程合同覆盖七组命令；Rust-only 单来源 fixture 流程串接 discovery、acquisition、science decode 和 NetCDF manifest-last/readback。RainViewer/composite 的 PNG、NetCDF4、GeoTIFF、Zarr v2 与 TW/grid NetCDF 有本地格式合同。该矩阵没有把 fixture 或目录覆盖当成真实 provider、全来源科学解码或云存储验收；具体边界见 [US1 矩阵记录](validation-results/rust-migration-us1-matrix.md)。
- **科学和来源限制：** Native legacy display 对 23 条路径有 15 条逐像素通过、0 条差异待定、8 条因缺少同帧/合法历史样本或 provider artifact 而 blocked。WU 现有一条负向合同，确保不会从其他来源 artifact 猜测其 tile 布局；这不等于 WU 实际布局验收。用户目前没有来源匹配样本。来源瓦片布局和历史路径证据仍由 T023 跟踪；跨 CRS、来源科学值/quality、缺测语义及更广覆盖由 T034 跟踪。未验收来源维持 fail-closed。
- **持久化和平台：** 本地 Python v1 manifest/cache 兼容、锁、故障注入及 manifest-last 已通过合同；RemoteStore 的内存协议测试通过，但真实 AWS S3/S3-compatible/Aliyun OSS 仍由 T041 验收。用户目前没有可用于真实 provider 验收的 S3/OSS endpoint/prefix。Wheel 在 macOS 26.6.2 arm64 主机以 macOS 11 deployment target 运行；实际 macOS 11 主机与 GitHub source-build workflow 尚未执行。
- **浏览器和 Python 边界：** Rust 浏览器 adapter 的 fake-CDP 合同覆盖版本、CSRF/header、超时/取消与 profile 清理；opt-in 本机 headless Chromium 测试成功连接 loopback CDP 并打开 about:blank，不访问 provider。PH/Windy provider artifact/凭据仍未验收。Python 保留 Rust SDK/CLI facade、显式 xarray 转换及可选 Zarr adapter；`encoder_options` 和 `Client.write()` 的 raw/远端/重网格仍是已记录的 SDK 限制。

US1–US5 尚未整体完成。当前开放任务为 T023、T034、T041 和最终门槛 T082；T045、T083 与性能、wheel、浏览器运行任务已完成。完整任务状态以 [实施任务](specs/003-rust-core-performance/tasks.md) 为准。
- 缓存安全策略：Rust 默认使用独立的 `~/.cache/radiust-rust`，兼容合同覆盖旧 Python `entries` 索引、对象摘要、lease、修复和 GC；新旧 CLI 不隐式切换缓存实现。旧 v1 输出清单可由 Rust 读取和校验，本地写入继续使用 Python 兼容 `.radiust.lock` 与 manifest-last 协议。

按以下依赖顺序实施，每一阶段都应保留可运行的离线回放：

1. **固定基线与契约。** 保存 24 个来源的目录展开、帧身份、原始摘要、已验证的科学数值/质量位、地理位置、legacy 显示像素、CLI JSON/退出码和输出 manifest 的 golden 数据。记录冷/热启动、首帧、批量耗时、进程树峰值 RSS、请求数和临时盘峰值。旧台账中未验证的来源继续拒绝科学解码。
2. **建立无 Python 依赖的引擎。** 在 `radiust-core` 定义 `Query`、`FrameRef`、`RawFrame`、`RadarField`/`RadarDataset`、网格、质量位、错误及报告；由一个持有共享 transport、缓存和取消令牌的 `Engine` 执行 `discover`、`fetch`、`download`、`preview`、目录与缓存操作。大数组由 Rust 持有，绑定按需转换，避免每阶段复制整帧。
3. **迁移来源及获取。** 把内置来源及其发现、下载和已验收解码逻辑逐个移到 Rust；统一 HTTP/FTP 的网络 opt-in、重定向检查、重试、TLS、认证信息脱敏、大小限制、共享连接池和取消。`ph`、`windy` 的可选浏览器路径改由 Rust CDP 客户端驱动系统 Chromium；TH 的 OCR 路径从 Rust 调用可选 Tesseract。两种外部程序都不得由普通来源隐式安装或启动。退役来源保留目录状态且不发起请求。
4. **迁移处理与输出。** 在 Rust 实现色标解码、瓦片组合、显示规则、重网格、PNG/GIF 与终端预览，及 NetCDF4、GeoTIFF、PNG、Zarr v2 编码；`cat --file` 所需的 NetCDF4 读取也由 Rust 承担。NetCDF4 使用 Rust `netcdf` 的静态构建，坐标变换使用 `proj`，GeoTIFF 写入可读回的 CRS/仿射标签，Zarr v2 保留 consolidated metadata 和现有数据类型/分块语义。Rust 与 Python xarray 分别读回生成文件；不以字节完全相同作为要求。依赖版本固定到 `Cargo.lock`，并把工作区声明的最低 Rust 版本与当前 1.92 工具链对齐。
5. **迁移持久化和入口。** Rust 复现现有身份计算、原始缓存校验/租约/GC、本地提交及 S3/OSS 的 manifest-last 协议。继续识别并校验 v1 正式输出 manifest；编码器或解码器语义变化时提升处理版本，避免错误地跳过旧成果。兼容证明前，Rust 默认使用独立缓存根；显式指向 Python `entries` 索引时拒绝 repair，旧缓存读取、跨版本 lease 和同根 GC 留待 T039/T018 验证。新增 Rust CLI crate 实现 `list`、`discover`、`download`、`cat`、`doctor`、`config`、`cache`；Python wheel 的命令入口只转发到 Rust 命令实现，原生二进制可不经 Python 启动。

## 并发和延迟设计

- 新增 `runtime.discovery_workers`，默认 4；现有帧、请求、主机和解码并发默认值分别保持 2、16、4、2。`discover all` 与 `discover au vn` 使用同一调度器：按来源轮转待办目标，同一次运行内合并同一上游元数据请求，按实际主机（包括重定向后的主机）使用共享限额。轮转负责公平性，最终报告按 source/product/station 稳定排序；不依赖随机顺序。
- 多来源发现仅接受 `latest` 和 `max-age`；单来源继续支持产品、站点及显式时次/范围选择。重复来源和 `all` 与其他来源混用在发起网络请求前报参数错误。现有聚合状态、逐目标错误、计数、网络禁用行为和退出码继续适用；多来源 JSON 查询增加 `sources` 列表。
- 整批发现共用一个截止时间。到期时，正在运行的目标标记 `timeout`、未派发的标记 `not_started`；用户中断时运行中的目标标记 `cancelled`，CLI 输出一份完整报告并以 130 退出。普通 Rust 请求使用异步超时和取消；Chromium、Tesseract 等可能阻塞的外部路径保留可终止的进程隔离与临时文件清理。
- 批量下载改为按 `frame_concurrency` 有界并行；获取、解码和提交分别受共享资源限额约束，结果按输入顺序输出。`on_error=stop/raise` 停止派发新帧并取消运行中任务，已完成结果仍保留。使用流式读取/落盘及边写边摘要，避免当前 `read_bytes()` 导致的大文件重复分配；并发提交仍遵守同一路径锁和 manifest-last 顺序。
- `cat` 的原图路径优先得到首帧，不启动科学解码；渲染与读取只处理选中的帧。`doctor` 在 Rust 中检查原生能力，Chromium/Tesseract 仅在相关来源需要时检查；网络探测仍需显式选择。

## 对外接口及迁移规则

| 接口 | 决定 |
| --- | --- |
| Rust 库 | 异步 `Engine` 方法返回结构化发现/下载报告、Rust 数据场或带观测时间的 RGBA 预览；进度和取消为显式参数/事件。CLI 与 PyO3 共用这一实现。 |
| CLI | 原有命令及主要选项继续存在，增加 `discover SOURCE...`；保留现有 JSON 报告和退出码语义，`config show` 与 `doctor` 反映新的原生依赖。 |
| Python | 公开 Rust 绑定对象及便捷调用；xarray 转换和 Python Zarr 操作放在可选依赖中。原有 Python 来源插件和默认基于 xarray 的科学对象返回值属于明确的破坏性变更。 |
| 成果与缓存 | 旧正式成果可读、可校验、可按身份跳过；缓存可清理、可共享使用，不把缓存内容当成正式输出。 |

## 验证与完成条件

- 对全部 24 个来源执行离线回放；核对帧身份、原始摘要、来源请求/凭据规则和现有科学证据边界。浏览器来源覆盖 warmup、cookie/CSRF、HTTP 错误、缺少 Chromium、超时及进程清理；无需公网即可跑核心测试。
- 对科学处理核对 `float32` 值、`uint16` 质量位、缺测/透明、CRS 和重网格；原图与 legacy 显示分别做逐像素比较。NetCDF4 通过现有 CF/读回检查，GeoTIFF 验证数据与 CRS，Rust Zarr v2 由 Python xarray 以 consolidated 模式读回。
- 对并发调度注入多来源延迟、同主机争用、部分失败、悬挂请求、Ctrl-C 和输出冲突；断言并发峰值不超过配置、结果顺序稳定、状态计数完整、无遗留子进程/临时目录，远端提交仍最后发布 manifest。
- 在同一台 macOS arm64 机器对每个场景采集至少 30 次并报告 p50/p95。离线 `list` 和 `discover all` 的 p95 至少比现有 Python 入口快 2 倍；四个独立延迟来源的聚合发现至少快 1.5 倍；代表性 `cat` 首帧 p95 至少降低 20%。相同场景的进程树峰值 RSS 不高于旧实现，同时记录请求数和临时盘峰值。现有本机三次冒烟仅得到热启动 `list sources` 约 0.28 秒、离线 `discover all` 约 0.53 秒，正式基线须重新采集。
- macOS arm64 原生二进制和 Python wheel 都需在干净环境中安装、运行命令和读回成果；检查动态库依赖，发布产物不得依赖开发机的 Homebrew 路径。完成前更新 CLI、SDK、安装和来源开发文档，并给出旧 Python API/插件的迁移说明。
