# Phase 0 Research: radiust v1

日期：2026-09-16。依据：[spec.md](spec.md)、[原始设计](../../plan.md)及只读代码/官方文档研究。本文的 Decision 是实施决策，不代表实现已验证。三个 `luna_worker` 分别负责旧源盘点、科学格式、跨语言运行时；主代理负责总体约束、身份/提交规则与终端。

## 研究问题与证据边界

| 编号 | 问题 | 证据与解法 |
| --- | --- | --- |
| R01 | 语言、构建、支持矩阵 | 现有 pyproject/Cargo 配置 + 官方支持信息，选择有限 wheel 矩阵，M1 从 wheel 实测 |
| R02 | 跨语言异步、FTP、资源生命周期 | 官方工具文档 + Client 所有权契约，M1/M2 故障注入 |
| R03 | NetCDF/CF/Zarr/GeoTIFF 默认与语义 | 官方格式和后端说明，选择显式编码规范及 round-trip 门槛 |
| R04 | OSS/S3 提交、缓存与覆盖 | 单写者约束 + 不可变 generation，M3 provider 实测 |
| R05 | 旧来源总范围和真实样本 | 本地旧仓库只读盘点，区分代码存在与在线可用 |
| R06 | 终端展示和降级 | 官方协议、原始设计 200ms 探测预算 |
| R07 | 身份与 raw 完整性 | 细化用户设计中的 logical_id/revision/output_id，避免 raw 补存改变科学身份 |
| R08 | 配置、CLI 和离线验证 | 轻量 CLI/严格配置、真实样本重放、命令退出码 |

## R01 — 语言、构建与支持矩阵（已决策）

**Decision**: 保留 Python >=3.10 与 Rust edition 2024。初始发布验证矩阵为 CPython 3.10/3.11/3.12/3.13 的常规 GIL 构建，Linux x86_64/aarch64 和 macOS arm64/x86_64；Windows、musl、PyPy、free-threaded 和未列 Python 版本不在首版声明支持范围。核心与各 extra 分开记录支持矩阵，不能因一个 extra 的 wheel 问题让 list 失败。

采用 maturin mixed layout，`python-source="python"`、`module-name="radiust._core"`、manifest 指向 `crates/radiust-core/Cargo.toml`；crate 产物包括 cdylib，Python facade 管理公共名字。初期按 CPython 小版本构建，不启用 abi3。候选构建线 PyO3/pyo3-async-runtimes 0.29 同 minor、maturin 1.15，Rust 编译器下限不得低于 edition 2024 所需 1.85，实际工具链取所有所选 crate MSRV 的最大值并锁入 rust-toolchain.toml。Cargo.lock 与各 Python 版本 constraints 在 M1 生成，精确 patch 锁定作为构建任务而非悬而未决的产品问题。将 workspace resolver 从 2 升为 3 的变更与构建兼容性一起验证。

**Rationale**: 当前 Python 包仍使用 setuptools，Rust 仅 VERSION 常量，没有可复用 I/O 实现。限定矩阵且从安装后的 wheel 验证，可以发现平台依赖和 Python/Rust ABI 问题；先不用 abi3 减少未经验证的 limited API 约束。

**Alternatives considered**: 保留 setuptools 无法直接完成所选 mixed build；立即全平台/free-threaded/abi3 同时支持扩大未经验证范围；提高 Python 下限会偏离现有项目声明，暂不采用。

**Evidence**: 仓库 `pyproject.toml`、`Cargo.toml`、`crates/radiust-core/Cargo.toml`、`python/radiust/__init__.py`；[maturin layout](https://www.maturin.rs/project_layout.html)、[PyO3 distribution](https://pyo3.rs/main/building-and-distribution)、[Rust 2024 resolver](https://doc.rust-lang.org/stable/edition-guide/rust-2024/cargo-resolver.html)。候选版本来自研究时文档，不宣称完成联合解析。

**Validation gate**: M1 在各声明矩阵从 wheel 安装并验证 `_core`、目录、离线 fetch、NetCDF round-trip；未通过的环境不得标为支持，不能靠源码树 import 冒充 wheel 验证。Python 依赖 metadata 必须与实际可安装版本一致。

## R02 — 异步运行时、HTTP 与 FTP（已决策）

**Decision**: Tokio + reqwest 为 Rust 网络层，PyO3 普通函数用 pyo3-async-runtimes 的 future_into_py 返回 awaitable；不采用实验性原生 async pyfunction 路径。请求使用流式落盘、rustls TLS、显式代理/headers/cookie 配置。所有 bridge 任务有取消 token 和被跟踪的句柄；关闭 Client 必须停止并 join，不能丢句柄造成后台任务继续。decode/encoder 使用有界线程 worker，依赖释放 GIL 的科学库处理；纯 Python 重 CPU 特例在对应 source 引入前通过样本决定进程隔离，不引入默认分布式 worker。

FTP 采用独立 suppaftp Tokio/rustls adapter。v1 声明只读 passive plain FTP，explicit FTPS 在本地 TLS 合同测试通过后声明；不声明 active、implicit FTPS、上传或 SFTP。以 au 为代表。HTTP 发现与 FTP 资料下载仍遵守同一帧 deadline/重试预算。浏览器来源保留 Python Playwright adapter，浏览器生命周期、导航超时、下载资料与临时目录纳入同一 Client 契约。

**Rationale**: async bridge 提供 Python future 与 Rust future 的连接，但不能代替应用层资源所有权。Tokio 丢弃 JoinHandle 会 detach，已开始 blocking 工作不可强制停止，因此必须保留输入租约并丢弃取消后的结果。FTP 与 HTTP 方言不同，单独适配能对 au 的 listing/下载/中断做精确验证。

**Alternatives considered**: Python 网络实现作为默认会偏离用户 Rust I/O 决策；OpenDAL FTP 可复用但来源方言与取消控制需要额外证明；为每个来源单独网络客户端会重复预算与泄漏风险。

**Evidence**: [future_into_py](https://docs.rs/pyo3-async-runtimes/latest/pyo3_async_runtimes/tokio/fn.future_into_py.html)、[Tokio JoinHandle](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html)、[CancellationToken](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html)、[reqwest](https://docs.rs/reqwest/latest/reqwest/)、[suppaftp](https://docs.rs/suppaftp/latest/suppaftp/)。

**Validation gate**: M1 Python cancel → Rust request 停止与临时清理；M2 FTP plain/TLS/passive listing、流中断与取消、本地浏览器重放。TLS 默认验证，不能迁移旧实现中的 verify=False 为默认；无效证书作为配置/上游问题报告。

## R03 — 科学存储与依赖（已决策）

**Decision**: 核心固定 xarray 2025.6.1 兼容线（该版本官方 metadata 支持 Python 3.10）；NetCDF 明确 engine=h5netcdf、format=NETCDF4、invalid_netcdf=False，h5py 为显式直接依赖，不依赖后端自动选择或可选依赖传递。h5netcdf 取 >=1.3,<2 且每个 Python 环境解析兼容 patch，不强制 >=1.8，以免未核实的新版本 Python 下限影响 3.10。h5py >=3.11,<4，NumPy >=2.0,<2.3、pyproj >=3.6,<3.8、Pillow >=10.4,<12 作为首轮解析约束，M1 锁定完整依赖图；这些是候选兼容边界，不是已完成安装证明。Click/PyYAML 同样进入锁文件。

CF 元数据目标固定 CF-1.8。保留用户计划的 uint16 quality，flag_masks 与变量 dtype 一致，0 不设置为缺测，主变量用 ancillary_variables 关联；不因工具建议而静默改变公开质量类型。Polar 元数据额外保留波束/站点几何，不仅加 Conventions 属性就宣称完整 CF 合规。

GeoTIFF extra 采用 rasterio >=1.4,<1.5 保留 Python 3.10 兼容线；数据 TIFF + quality TIFF + provenance JSON，同 CRS/affine/shape。mask 不能替代多位 quality。Zarr extra 固定 `zarr==2.18.3`、`numcodecs>=0.13,<0.14`，格式固定 v2，每帧独立 store；显式 zarr_format=2（所锁 xarray 的兼容入口若需 zarr_version，则由 encoder facade 适配）。chunk 默认每二维轴 min(512,轴长)，主变量和 quality 同 chunk，M3 实测后仅通过版本化 encoder 参数改变。科学数据默认在内存中完成，不强制 Dask。

**Rationale**: 自动 backend/格式选择会令同一配置随环境产生不同文件；Python 3.10 需要显式兼容线，不能盲目依赖最新版。quality 精确与 CRS/坐标读回比逐字节一致更重要。

**Alternatives considered**: netCDF4 可作为独立交叉读回测试工具，但不引入额外默认 backend；scipy NetCDF3 不作默认；Zarr v3 与共享时间 append 延后；自动换 Rasterio 新大版本会提高 Python 下限。

**Evidence**: [xarray 2025.6.1 metadata](https://pypi.org/project/xarray/2025.6.1/)、[xarray NetCDF I/O](https://docs.xarray.dev/en/stable/user-guide/io.html)、[h5netcdf](https://h5netcdf.org/index.html)、[h5py threading](https://docs.h5py.org/en/latest/threads.html)、[CF 1.8](https://cfconventions.org/cf-conventions/v1.8.0/cf-conventions.html)、[Rasterio installation](https://rasterio.readthedocs.io/en/stable/installation.html)、[Rasterio masks](https://rasterio.readthedocs.io/en/stable/topics/masks.html)、[Zarr 2.18.3 metadata](https://pypi.org/project/zarr/2.18.3/)、[xarray to_zarr](https://docs.xarray.dev/en/stable/generated/xarray.Dataset.to_zarr.html)。

**Validation gate**: M1 独立读回数值/quality/坐标/时间/CRS/provenance、CF 检查和候选依赖矩阵；M3 GeoTIFF/Zarr round-trip，缺依赖前置失败，NumPy/二进制 wheel 兼容性。h5py 写入用专门单 writer 通道，不把 HDF5 多线程吞吐当目标。

## R04 — 存储与缓存（已决策）

**Decision**: Rust OpenDAL 统一 S3/OSS 流式 I/O，显式启用所需服务 feature 随标准 wheel 编译。本地原子提交单独使用文件系统操作以明确 fsync/rename/锁语义；不能因为远端 adapter 提供 rename 就假定原子。`radiust[storage]` 保留为空的兼容 extra，并在文档声明 OSS/S3 能力已随核心 wheel 发布。认证在配置边界解析，provider chain 最后兜底。

远端不可变 generation + 最后单对象 manifest 发布，明确 single-writer；provider 特有条件写仅在 capability/集成测试通过后使用，不是 v1 安全性的前提。未完成 multipart 尽力 abort，并记录可重查 generation/upload 标识；残留由显式输出维护或 bucket 生命周期清理，cache gc 永不处理正式 root。SQLite 索引+本地对象/mosaic，采用内容校验、每 key 发布锁和独立 lease，网络期间不持 SQLite 事务。

**Rationale**: OpenDAL 同时提供 S3-compatible/OSS 服务和 writer close/abort，但各 provider 能力不同；不能把 AWS 条件写保证套用到 OSS。应用层统一提交状态机才能保持 raw、覆盖和重跑语义一致。

**Alternatives considered**: 为每个 provider 使用不同 Python SDK 会重复流式传输、认证与提交逻辑；仅以 exists 判断幂等无法处理失败和并发；cache gc 清远端 staging 会混淆正式输出所有权。

**Evidence**: [OpenDAL services](https://opendal.apache.org/docs/rust/opendal/services/)、[Writer](https://opendal.apache.org/docs/rust/opendal_core/struct.Writer.html)、[AWS conditional writes](https://docs.aws.amazon.com/AmazonS3/latest/userguide/conditional-writes.html)、[OSS PutObject](https://www.alibabacloud.com/help/en/oss/developer-reference/putobject)。

**Validation gate**: M3 本地 + AWS S3 + 至少一个兼容服务 + Aliyun OSS 分别执行完整存储矩阵；无实际访问条件时保留未执行状态，不能标该 provider 已验证。R09 commit fence 覆盖响应丢失/取消竞态。

## R05 — 迁移口径与代表源（已决策）

**Decision**: 固定旧仓库基准 commit `8d251601ca551fbd5c05451f1fb337fc4b75362c`，按“具体实现路径+注册状态”建立 [source-inventory.md](source-inventory.md)。HEAD 包含 19 个 scraper 实现文件与 5 个雷达 tile 实现；不能把它们简单等同独立逻辑来源。两项仅存 Git 历史的巴西实现也保留在盘点附录，M0 给出迁移或历史排除依据，不静默遗漏。非雷达卫星/底图和旧生产业务集成明确排除。

首批代表：my 单图、id_sidarma palette、rainviewer tile、au FTP、ph 浏览器 fallback、fr source-specific recovery/WMS；tw 增补匿名 S3 acquisition。旧 source id 原样保留区分实现，tw 的 HTTP 历史路径以 tw-http 内置独立 id 暂存，不因同地区就合并；印尼三路径、泰国两路径分别保留。具体产品与站点 id 从旧实现/资料事实确定，列为 M0 交付字段。

**Rationale**: 只读注册表会漏掉注释和未导入实现。旧 output 目录有图像，但不是版本化真实 raw fixture；旧 uint8 编码产物不能直接充当新科学真值。原始计划里的 jma 是接口示例，本次未在旧仓库确认它是现有来源，因此不凭示例新增 JMA 迁移任务。

**Alternatives considered**: 只迁移启用项不满足全量要求；按国家合并会掩盖不同上游、产品和时间绑定；将历史文件自动标 retired 缺少证据。

**Evidence**: 旧仓库 `core/source_app.py:8`、`core/scrapers/__init__.py:1`、`core/app.py:11`、`core/tiles/radar.py`、`.gitignore:211`、`core/scrapers/base.py:309`；详细条目在 inventory。

**Validation gate**: M0 每行产品/站点/时间绑定/网格/依赖/fixture 状态可追溯；M4 每个非例外来源至少一个真实原始样本完成统一 contract，所有例外有证据并被接受。此次未做 live 探测、不宣称来源在线或样本验收完成。

## R06 — 终端显示

**Decision**: 使用 Pillow/NumPy 生成唯一的 RGBA+图例+标题表示，PNG 和终端共用。协议层仅负责传输。auto 总探测预算 200ms，先 Kitty 查询确认，再使用可确认的 iTerm2 能力，最后 ANSI/text；未知能力不得猜测图片支持。Kitty 使用内联 PNG、分块发送；iTerm2 使用 inline PNG 并保持纵横比。tmux/screen 未确认 passthrough 时降级。`NO_COLOR`/`TERM=dumb` 自动文字；非 TTY 默认在获取前拒绝，显式 text 可重定向。成功、错误、取消均恢复 termios，限制探测读取，不吞用户键盘输入。

**Rationale**: Kitty 定义 query 和分块传输，iTerm2 定义 inline、宽高和比例选项；不依赖远程路径或外部 GUI。预算、降级顺序与保守探测是本项目设计选择，不是协议对所有终端的保证。

**Alternatives considered**: 外部 icat/imgcat 命令增加部署依赖；Sixel 增加协议范围；Matplotlib/浏览器不适合作为默认核心依赖。

**Evidence**: [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)、[iTerm2 inline images](https://iterm2.com/documentation-images.html)、[iTerm2 escape codes](https://iterm2.com/documentation-escape-codes.html)。

**Validation gate**: M1 ANSI/text 快照与伪终端测试；M2 Kitty/iTerm2 每种真实终端验收，记录应用版本、TTY/SSH/tmux 场景及取消恢复。探测逻辑无法确认时允许保守降级，不允许宣称实测覆盖。

## R07 — 稳定身份、修订与 raw 完整性

**Decision**: Python 是公开身份规范的唯一计算方。版本化 canonical JSON 明确 UTF-8、键排序、无多余空白、禁止 NaN/Infinity、UTC 固定精度时间；进入 hash 的对象在构造时递归冻结，不能只冻结外层。Rust 接收身份字符串，不独立实现另一套浮点序列化。签名 URL、授权 header/cookie 从身份和持久化记录中排除。

- logical_id：source、product、station、valid_time、base_time、locator_version、locator。
- revision：优先可信上游 revision；否则按 artifact 名称稳定排序，以名称、字节大小、SHA-256 的清单计算内容 revision。ETag 只作 validator。实际获取后通过 receipt 补充，不修改 FrameRef。
- processing hash：输出类型 decoded/raw-only、decoder/resource 版本、变量、网格、重采样、encoder 版本和参数。decoded 的“另存 raw”是完整性要求，不改科学 processing hash；raw-only 有独立类型。
- output_id：完整 logical_id、resolved revision、processing hash；variant_id 为前 12 个 hex，碰撞必须报错。
- `raw=true` 的 skip 条件额外要求 manifest 的 raw_complete；mosaic 缓存不满足原始瓦片完整性。补存 raw 必须证实同一 revision，否则作为新修订输出，不能混配旧 decoded 与新 raw。
- 默认路径含 variant，不同修订自然产生不同组；自定义模板压缩身份差异时冲突。overwrite 只在显式允许下替代所选目标，并保留远端旧 generation。

**Rationale**: 将数据身份与保存完整性分开，可满足“已有 decoded 可以补 raw”的用户意图，避免秘密信息或运行环境改变缓存/输出身份。

**Alternatives considered**: URL 作为 key 无法支持动态与签名地址；把 raw flag 纳入 decoded 身份会重复科学文件；Python/Rust 各自计算任意 JSON hash 容易出现差异。

**Evidence**: 原始设计“FrameRef 的稳定身份”“Manifest 与提交协议”；[Python JSON 编解码](https://docs.python.org/3/library/json.html)支持严格数值和排序选项，但上述身份规则是本项目规范。

**Validation gate**: 固定身份 golden vectors 覆盖时区等价、键顺序、空值、locator 版本、相同内容不同 URL、起报差异、raw 补存和短 hash 人工碰撞。

## R08 — CLI、配置和离线验证

**Decision**: CLI 使用 Click 命令组，配置使用 PyYAML safe loader + 显式 schema 校验，模型以 dataclass/枚举及专门 validate 组织。禁止配置求值与动态执行模板。普通参数优先级遵照 spec；凭据字段成组解析，不能拼接不同来源的 access/secret 成为一对。使用 logging 库，库不设置全局 handler；CLI 安装带脱敏过滤器的 stderr handler。

**Rationale**: 组合命令与懒加载满足目录查询和可选依赖隔离；显式 schema 避免重型配置框架。离线测试走真实 source 协议和本地传输重放，测试入口不加入公开 source registry。

**Alternatives considered**: 全手写 argparse 可行，但多层命令与测试维护成本较高；通用配置求值和生产配置中心不满足独立边界。

**Evidence**: [Click 官方文档](https://click.palletsprojects.com/en/stable/)提供命令组、帮助与懒加载；取消规则参考 [Python asyncio tasks](https://docs.python.org/3/library/asyncio-task.html)。Python 3.10 基线不得直接依赖 3.11 才有的 TaskGroup/timeout；采用兼容的 gather/wait_for 和显式取消收尾。

**Validation gate**: M1 创建离线 replay fixture 和 SDK/CLI 测试，先让命令按契约失败，再接入实际闭环。quickstart 中未来测试路径明确标注为实施后可运行，不把当前骨架当已支持。

## R09 — 中断、覆盖与完成判定的统一边界

**Decision**: 所有高层导出使用同一提交状态机，输出身份在获取 receipt 后最终确定。重跑只认可受支持 schema、完整身份和全部文件完整性均有效的 manifest；单独文件存在不是成功。文件完整性采用本地重新校验，远端使用能证实内容的 provider checksum，若 provider 无此能力则流式读回计算摘要，不能把自写 metadata 或 multipart ETag 当 SHA-256 校验成功。

- 本地持有 root 独占锁，staging 必须与目标同文件系统。发布前验证并落盘；替换旧组前撤除/移走旧完成标记，再移动产物，最后原子发布新 manifest。失败可以暂时没有可用新组，但不能让旧标记指向半替换内容。
- 远端每次提交独立不可变 generation。先提交全部数据及 generation manifest，再单对象更新公开组 manifest（记录 generation 和相对 artifact 路径）。单写者由外部保证，不假定跨对象事务或 rename。pointer 更新失败保留旧组；响应丢失时读取并核对后再决定结果。
- 提交临界点之前观察到取消则不发布；若不可撤回的最终远端请求已经发出且取消与提交竞态，进入结果核对，若发现新清单已可见则记录实际 committed，不能伪报“取消且零副作用”。对调用者的中断状态与已经完成的逐帧结果分别报告。

**Rationale**: 原始计划要求取消不迟到提交成功，实施通过显式 commit gate 满足“已接受取消的未提交工作不再发布”。一旦发布已完成，不能逆向宣称没有写入。这个线性化边界使取消契约可测试，并保留可靠报告。

**Alternatives considered**: 原地多文件覆盖无法保证读者不会看见半组；将删除新版本作为默认取消回滚会制造额外丢失风险；每个 provider 自行定义业务提交会导致语义分裂。

**Evidence**: 原始设计的单写者与 manifest-last 约束。此节是基于其要求的协议推导，不声称所有存储提供同等一致性或已验证实现。

**Validation gate**: 在每个状态转换注入失败、取消、写入成功但响应丢失；核验 manifest 可读性与数据摘要。取消发生于最终提交前不得出现迟到 manifest；最终请求已开始的竞态必须得到可诊断、可重查结果。没有实测的 provider 不列为已支持。

## R10 — 有界资源与验证优先级

**Decision**: 每个 Client 管理网络预算、worker、临时资源和租约；应用级 RuntimeManager 汇总同进程活跃 Client 的实际 I/O 限制，避免多个 Client 绕过全局上限。默认并行帧 2、全局请求 16、每 host 4、decode worker 2；tile 与普通请求共享预算，来源声明更严格限制时取最小值。同一进程同 cache key 合并获取，跨进程同 key 互斥发布。不同进程的请求限流不宣称全局统一。

**Decision**: 首轮保护值为单 artifact 512 MiB、单帧原始总量 2 GiB、解码图像 100 百万像素、临时盘 10 GiB、用户态传输缓冲合计 64 MiB、请求超时 30 秒、整帧 deadline 300 秒。全部可配置且在来源盘点后以真实样本校正，属于保护阈值而非性能承诺。元数据已知时先检查；未知长度流按累计字节检查；图像头不可信时解码分配前再次核验。内存限制必须计入转换副本与质量数组，不仅输入压缩文件。

**Rationale**: 原始计划给出了并发起始值，但没有全部资源默认数值；采用明确的保守初值让后续实现与超限测试可执行。真实超大产品由来源测试定义显式配置，不静默放开限制。

**Alternatives considered**: 只限制并发不能约束单个巨大文件；CPU工作取消后马上删除其输入会导致悬挂引用；用同一个不受限线程池执行所有科学计算不能保证事件循环与终端取消响应。

**Validation gate**: 在 M1 首先完成真实样本的大小/像素预检，M2 测量各家族峰值后确认默认值，M3 验证多 Client、缓存租约、网络重试总预算与取消收尾。CPU worker 的输入所有权延长到真正结束，禁止迟到结果进入 commit gate。性能报告记录硬件、输入摘要、并发、缓存状态和依赖版本，冷缓存与暖缓存分别测试。

## 规格到研究问题的覆盖检查

| 规格范围 | 研究问题 | 必须落实的设计产物 |
| --- | --- | --- |
| FR-001、002、034～037 | R01、R05、R08 | 包布局、source inventory、registry/source contract、wheel 与迁移门槛 |
| FR-003～007 | R02、R07、R10 | Query/FrameRef/receipt、时间绑定、所有权与身份规范 |
| FR-008～013 | R03、R05 | 数据模型、quality、grid、decoder 和转换契约 |
| FR-014、015 | R02、R08、R10 | sync/async Client、批量和流式结果、取消和背压 |
| FR-016～022 | R03、R04、R07、R09 | encoder、manifest、raw 完整性、存储状态机 |
| FR-023～027 | R02、R04、R08、R10 | 缓存索引/lease、资源阈值、配置与清理 |
| FR-028～030 | R08、R09 | CLI、报告、错误和退出状态 |
| FR-031～033 | R06 | render/show、TTY、协议与无终端测试 |

研究阶段只做代码/文档核实；不安装项目、不写业务代码、不执行真实采集、不探测需要凭据的生产端点、不归档旧仓库。样本取得、构建锁定、真实终端与对象存储验收有明确后续门槛，不作为本次已完成能力。

## Phase 0 结论

R01–R10 的设计选择已明确，无阻塞用户澄清项。候选库版本的联合解析、真实样本、在线状态、CF/存储/平台兼容性保留为有明确阶段与失败处理的实施验证，不伪装成本次已完成能力。可进入 Phase 1 数据模型、契约与验证指南的完整汇总；设计后重新执行规格约束检查。
