# Phase 0 Research: Rust 核心迁移与性能优化

本研究使用 [spec.md](spec.md)、根目录 `migration.md`、现有代码/契约与逐来源证据台账。它为设计选定迁移路径；尚未执行的格式、平台或性能验证明确留作实施验收，不视为已通过。

## 1. 单一业务核心和入口

- **Decision**: 延伸现有 `radiust-core` 为可独立运行的业务核心，增加工作区原生 CLI；Python wheel 的命令入口转发到同一命令逻辑，Python SDK 通过薄绑定调用核心。核心不依赖 Python 运行时。
- **Rationale**: 现有 Python CLI、SDK 和部分 Rust 原语并存，重复实现会使来源规则、身份和安全限制漂移。共用核心也为未来 macOS 桌面界面留下直接可嵌入的边界。
- **Alternatives considered**: Python CLI 继续编排 Rust 原语（不能满足无 Python 运行）；CLI 与 SDK 各建一套 Rust 实现（重复且难以保持契约一致）；本轮建立服务/IPC（超出范围）。
- **Evidence / gate**: `Cargo.toml` 当前仅含 `radiust-core`，`pyproject.toml` 的脚本入口仍为 Python。独立 CLI 需在无 Python 的干净环境证明七组命令和所有格式可运行。

## 2. 契约冻结与迁移次序

- **Decision**: 先采集旧版离线 golden 契约和正式 30 次性能基线，再迁移核心模型与来源，最后切换 CLI/绑定/发布。迁移期间以来源/产品/站点为单位对照帧身份、原始摘要、可用状态和已验收科学输出。
- **Rationale**: 24 来源的历史证据并不等同；只有基线和离线回放能区分代码迁移回归与本就 blocked 的科学验证。
- **Alternatives considered**: 一次性替换全部适配器（回归定位困难）；以在线请求作为唯一验收（不稳定且可能需要凭据）。
- **Evidence / gate**: `tests/fixtures/sources/`、`tests/sources/`、`migration/` 与 `validation-results/` 已有样本/台账；未验证路径不得在迁移中升级为科学通过。
- **Catalog boundary**: `python/radiust/resources/catalog.json` 与 `python/radiust/registry.py` 登记的 24 个内置 ID 是 `au, bmkg, ca, cam, es, fr, id, id_sidarma, kr, my, nz, opensnow, ph, pt, rainviewer, sg, th, th_royalrain, tw, tw-http, uk, vn, windy, wunderground`。其中 `uk` 为 retired；`my` 有两个站点，`tw` 有两个产品，所以当前内置 `discover all` 展开为 26 个目标。仓库虽有 `br_cptec.py` 和 `br_sipam.py`，但它们不在内置 catalog/factory，本轮不得把这两者算进 24 个迁移完成率；其未来登记须另行确认。

## 3. 多来源发现与有界下载

- **Decision**: `discover all` 与显式 `discover SOURCE...` 共享一个按来源公平推进的有界调度；发现任务上限 `runtime.discovery_workers` 默认 4，独立于帧下载上限。现有请求、主机、解码和帧上限保持 16、4、2、2 的默认值；同一次运行中重复上游元数据请求合并，最终报告按来源/产品/站点稳定排序。下载只并行独立帧，停止策略不再派发新帧并取消尚未完成工作。
- **Rationale**: 当前全量发现逐目标等待；并发可缩短多个独立慢来源的墙钟时间，但必须共享预算，避免为速度放大上游压力或改变输出顺序。
- **Alternatives considered**: 随机打乱请求再无界派发（公平与可复现性差，资源不可控）；所有来源串行（达不到目标）；按每来源独立限额（难以遵守全局/同主机约束）。
- **Evidence / gate**: 同机固定延迟回放验证 p95、活动任务峰值、同主机共享限制、超时/中断状态计数；截止时间自整个任务开始计时，而非给每目标重新计时。
- **Existing behavior detail**: `python/radiust/discovery.py` 的 `discover all` 先在子进程内有界枚举目录，再逐目标启动隔离 worker；目前协调循环串行，而且 worker 内的帧/请求/主机上限被设为 1。`pipeline.py::_adownload` 逐帧串行；`batch.py::fetch_many_async` 已并发，但不能误当作下载已并发。迁移后的调度必须分别验证发现和下载。
- **Compatibility exceptions**: 当前单源 discover 的 JSON 是精简 frame 列表，聚合 discover 才是逐目标 `DiscoveryReport`，不能把所有查询都强制改成聚合 shape。现有空列表/异常的退出码存在不一致：单源 discover 返回空列表可为 0、抛 NoDataError 为 3；download 无 refs 可经 CLI 通用异常变成 2。除规格已明确的中断完整报告和多来源扩展外，先以真实旧行为做 golden 并保留这些边界；若要统一，需在迁移说明中列为显式行为变更。当前 CLI `raise/stop` 可能只输出错误 envelope 而丢失 `partial_result`；新设计按用户批准的完整批量报告修正，并把差异列为明确兼容例外。

## 4. 获取策略与来源专用外部程序

- **Decision**: 所有内置来源通过共享网络许可、重定向、认证脱敏、重试、大小/时间和取消策略。浏览器来源仅按需驱动系统已有 Chromium；OCR 来源仅按需调用系统已有 Tesseract，并保留有界终止及临时资料清理。退役来源只保留目录状态。
- **Rationale**: Rust core 已有部分 HTTP/FTP/对象存储原语，但来源获取仍主要在 Python；分散策略会使并发时的主机限制和安全行为不一致。
- **Alternatives considered**: 打包浏览器（体积及维护成本高）；普通请求隐式启动浏览器/OCR（破坏延迟和可预测性）；将来源专用路径留在 Python（违背原生 CLI）。
- **Evidence / gate**: 浏览器来源覆盖 warmup、cookie/CSRF、HTTP 错误、缺程序、超时及进程清理；网络默认关闭，不读取旧仓库凭据。
- **Source status caveat**: 当前 `discover all` 对 `id/ph/id_sidarma/wunderground` 有来源特定凭据预检，不是按所有 `required_extras` 通用推断。迁移时须逐目标比对禁网、缺凭据和退役状态，不能因为统一实现而悄悄改写状态。
- **Upstream detail**: [Chrome 的远程调试变更](https://developer.chrome.com/blog/remote-debugging-port) 要求较新 Chrome 使用非默认用户数据目录。若采用 [chromiumoxide](https://github.com/mattsse/chromiumoxide) 驱动系统浏览器，应使用隔离临时 profile、限制调试端口为本机并验证系统浏览器版本变化；它的启动路径也有解析英文进程输出的限制。备选是手写较小 CDP 子集，但维护成本更高。临时 profile 与子进程清理列为必须通过的故障注入。

## 5. 科学数据与输出格式

- **Decision**: 核心拥有科学数据、质量位、网格和处理历史，区分原图/legacy 显示与科学结果；原生 CLI 输出 NetCDF4、GeoTIFF、PNG、Zarr v2，Python 的 xarray/Zarr 适配按需使用。每种格式以读回语义而非输出字节完全相同验收。
- **Rationale**: 原生 CLI 不能借 Python 编码；现有输出承诺包括时间、地理信息、`float32` 数值、`uint16` 质量位和缺测语义。
- **Alternatives considered**: 原生 CLI 不支持 Zarr（与明确范围冲突）；将图片灰度直接当科学数值（违反科学验收边界）；只比较文件 hash（编码细节变化会导致误报）。
- **Evidence / gate**: 原生读回及 Python xarray 的 NetCDF/Zarr 读回，GeoTIFF 地理标签读回，PNG/legacy 逐像素对比；需要验证发布产物的本地原生库依赖，不以开发机可编译代替可分发。
- **NetCDF decision**: 优先评估 [GeoRust netcdf 的静态构建](https://github.com/georust/netcdf)；[netcdf-sys 构建说明](https://docs.rs/crate/netcdf-sys/latest)指出该路径需从源码构建 netCDF/HDF5，并需要 CMake/C++ 工具。其底层 C 库调用按上游同步约束处理，不假设科学编码可无界并行。备选为系统动态库，但发布产物更容易依赖开发机路径。arm64 完整组合及 CF 语义仍须在干净环境实测。
- **Coordinates/GeoTIFF decision**: 优先评估 [GeoRust proj 的 bundled 构建](https://github.com/georust/proj)，并显式打包 [PROJ 资源文件](https://proj.org/en/stable/resource_files.html)；运行时 `proj.db` 和可能需要的格网不是静态链接自动提供的。GeoTIFF 写入可评估 [libgeotiff](https://github.com/OSGeo/libgeotiff) 与 libtiff 的组合，或受限纯 Rust TIFF+地理标签方案；[GeoRust geotiff](https://github.com/georust/geotiff) 是读取库，不能据此宣称具备写入。以跨工具 CRS/仿射读回决定最终选型，而非仅以可编译性决定。
- **Zarr decision**: 先用现有 Python 输出样本核对 v2 dtype、chunk、codec、consolidated metadata，再评估 [zarrs 的 v2 路径](https://book.zarrs.dev/arrays/array_init.html)；其 v2 支持是与 v3 兼容的子集，v2 数组需显式 metadata，不能假定通用 builder 覆盖旧输出。若子集不够，应实现受限 v2 编码器或改选经互操作验证的库，而不能改变用户输出格式。实施前固定 codec/版本并对每个 chunk 写入加互斥约束。
- **Release implication**: 静态链接不等于无需随包提供运行时数据文件；macOS 分发还要按 [Apple 公证文档](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution) 检查实际代码签名/嵌套依赖。是否进行 Developer ID 分发属于后续发布任务；本轮的硬门槛是干净 arm64 安装与无开发机路径。

## 6. 缓存与正式成果兼容

- **Decision**: 复用现有逻辑/修订/处理身份规则；原生缓存读写兼容实际 Python `entries` 索引及原始文件格式，不让两套 schema 在同一根目录互相覆盖。正式输出继续校验 v1 manifest，本地与远端均以最终 manifest 作为完成边界；处理语义变化提升处理版本。
- **Rationale**: 缓存可重建，正式输出不可被错误跳过或半成品覆盖。当前 Rust 与 Python 缓存 SQLite schema 不同，直接替换会损坏共享目录使用。
- **Alternatives considered**: 清空旧缓存（增加流量并违背兼容期）；按新规则猜测旧成果完整性（可能错误 skip）；原生/旧 Python 同根目录各自写索引（冲突）。
- **Evidence / gate**: 旧缓存样本、旧 manifest 和中断注入回放；同路径并发、远端上传/清单发布失败、已提交结果读回均须验证。
- **Code audit finding**: `python/radiust/cache.py` 使用 `entries` 索引，`crates/radiust-core/src/cache/index.rs` 使用不同的 `objects` 索引；两侧启动修复都会清理自己索引外的缓存文件。未完成兼容迁移前，必须使用独立缓存根目录，禁止旧 Python 进程和新 Rust 进程在同一根目录并行读写或修复。原生实现以旧 `entries` 格式及文件/租约规则为兼容目标，完成双向回放后才允许同根使用。
- **Code audit finding**: Python 本地输出锁使用 advisory `flock`，现有 Rust `RootLock` 使用 `create_new`。新写路径必须兼容旧锁协议，不能仅靠 Rust 锁文件存在与否判断；跨实现并发写入需要独立验证。

## 7. Python 迁移与兼容策略

- **Decision**: `fetch` 默认返回由核心持有的结果；显式 `to_xarray()` 转换，xarray/Zarr Python 操作为可选依赖。旧 `radiust.sources` entry-point 插件接口退出，新增来源走编译期 Rust 扩展；提供调用和来源开发迁移说明。
- **Rationale**: 避免基础入口加载大型科学依赖，降低内存复制与启动负担；破坏性边界已由用户接受，应明确发布说明而非暗中保留第二套实现。
- **Alternatives considered**: 默认立即转换 xarray（保留启动/复制开销）；运行时加载 Python 来源插件（原生 CLI 再次依赖 Python）。
- **Evidence / gate**: 基础 wheel 无 xarray 仍可执行发现/获取；可选依赖安装后转换的数值、质量、坐标和时间一致。

## 8. 发布与性能测量

- **Decision**: 首发只以 macOS arm64 为门槛；构建并安装原生程序与 Python wheel，检查动态库路径。固定同机同输入至少 30 次，记录 p50/p95、整个进程树峰值 RSS、请求数、临时盘峰值，并把旧版正式基线与新值共同存档。
- **Rationale**: 单次运行和开发机二进制无法证明交互延迟、资源目标或可分发性。
- **Alternatives considered**: 只测平均值或热启动（掩盖尾延迟）；将其他平台作为首发阻塞项（扩大用户批准范围）。
- **Evidence / gate**: 对照 [spec.md](spec.md) SC-001～SC-008；已有少量约 0.28/0.53 秒本机冒烟只作线索，不作为正式基线。

**Clarifications**: 规格和根目录计划已确定平台、入口、Python 破坏性变更、Zarr/浏览器边界与并发范围；本阶段无未解决的 `NEEDS CLARIFICATION`。依赖库与分发方式的可行性由实施期读回/干净安装门槛证明，不将尚未运行的检查写成完成事实。
