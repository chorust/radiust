# Research: 局部投影与站表

**Date**: 2026-10-10
**Status**: Phase 0 设计研究；已核对当前 Core 接口及旧项目代码，未运行投影实现、产品测试或雷达观测上游验证。

## R01 — 局部坐标采用 WGS84 椭球方位等距投影

**Decision**: 用户位置作为 AEQD 中心；内部 x/y 单位 m，显示 km。原点到目标的投影半径对应椭球测地距离，方向按从中心出发的方位角定义。

**Rationale**: 用户请求的是“距我 50 km”。AEQD 的中心距离语义直接满足需求；使用 WGS84 椭球明确地球模型。它不是等面积投影，也不保证任意两个非中心点间的平面距离等于地表距离。依据：[PROJ AEQD](https://proj.org/en/stable/operations/projections/aeqd.html)、[GeographicLib AEQD 实现](https://raw.githubusercontent.com/geographiclib/geographiclib/main/src/AzimuthalEquidistant.cpp)。

**Alternatives considered**: 平移 Web Mercator、简单经纬度差、ENU 切平面、球面 AEQD。前两项不能直接保证地表半径，ENU 的平面距离语义不同；球面版可服务上游明确声明的球面投影，但不作为用户视图的默认地球模型。

## R02 — 使用测地线正反解实现 AEQD

**Decision**: 实施时拟引入 `geographiclib-rs` 的 WGS84 正反解，薄层计算 AEQD 的 x/y。设计核对的上游 manifest 为 0.2.7，实施时锁定实际版本并核对构建；本次不修改 Cargo 依赖。

**Rationale**: Rust workspace 目前没有测地线库；Python 科学可选依赖已有 pyproj，但不能让原生 CLI 的投影依赖 Python。稳健的椭球正反解超出几行代码的可靠实现范围。Rust 版本提供 direct/inverse，拟关闭用于多边形高精度面积的默认可选 feature，保留测地线功能；无需新增 PROJ 原生动态库与坐标数据库。依据：[维护者仓库](https://github.com/georust/geographiclib-rs)、[manifest](https://raw.githubusercontent.com/georust/geographiclib-rs/main/Cargo.toml)。

**Alternatives considered**: 手写 Vincenty、完整 PROJ 运行时、Python 投影。前者增加收敛和验证成本；后两项分别增加本地部署负担或复制 Rust 执行边界。PROJ 可用于离线独立参考，不作为产品运行依赖。

## R03 — 逐像元反查，复用采样语义

**Decision**: 输出网格点先逆 AEQD 到 WGS84，再转到源坐标并采样；不能分别预变换一条 x 轴和一条 y 轴。

**Rationale**: 当前 [grid.rs](../../crates/radiust-core/src/grid.rs) 的跨 CRS 路径分别转换 target_x、target_y，适用于现有 Web Mercator 映射。AEQD 的经纬度同时依赖 x 和 y。提取既有查轴/最近邻/bilinear 内核，保持旧 regrid 行为。现有 nearest 在域外会附带边界像元的 quality；局部入口须使用 NaN + missing/outside，不能带入边界 recovered 等标记。含 origin/alpha/adjustment 的类型不能先降为普通 RadarField 再声称质量完整。

**Alternatives considered**: 先投影整幅图再裁剪会增加缓冲与不必要处理；绘图阶段挪动图像中心没有改变坐标；直接运行现有 regrid 会漏掉二维依赖。

## R04 — 源投影与基准面转换分别准入

**Decision**: 首轮转换准入为已验证的 EPSG:4326、EPSG:3857。TW grid 保留 EPSG:3821/TWD67，不能改标成 WGS84。SG 资源声明的球面 AEQD 单列后续转换验证，不能仅改中心参数。

**Rationale**: [科学 decoder](../../crates/radiust-core/src/science.rs)和 [SG 资源](../../python/radiust/resources/sources/sg.json)保留不同地球模型；涉及 datum 时，投影与基准转换是不同操作。依据：[PROJ geodetic transformation](https://proj.org/en/stable/usage/transformation.html)。

**Alternatives considered**: 任意 PROJ 字符串、自动 ballpark datum shift、用“误差小于像元”作为无证据的理由。首版全部拒绝，不虚构支持。

## R05 — 站址、查询目标、产品域、观测覆盖分开

**Decision**: 复用既有目录及来源资源，按来源内身份组织空间记录；物理站类型与 query selector 分开，覆盖声明绑定到具体 product/selector。字段和整理流程见 [station-catalog.md](station-catalog.md)。

**Rationale**: 主目录有 48 个 RDCAP 站、2 个 MY 区域条目；SIDARMA 资源还有 47 条历史站址，两条印尼渠道共享相同站码清单。ESCOMP、CV1_3600、PHCOMP4 等是产品选择器，不能由 station 字段名称推断它们是实体雷达。现有 frame 网格外框也不等于有效扫描范围。

**Alternatives considered**: 独立全球站点数据库、按短码合并全部渠道、对每站默认 250 km 圆。这些方案超出当前证据和任务。

## R06 — 位置流程新增显式入口，不改变旧 Query 含义

**Decision**: Core 增加位置视图请求；裸 cat 路由到它。显式 `cat SOURCE` 与 `cat --file` 默认沿用原语义，`--local` 才请求位置视图。初版自动流程仅 latest。

**Rationale**: [Query](../../crates/radiust-core/src/model.rs) 当前为 source/product/station/time 查询；多来源不允许 station/product 筛选。不能通过空 source 或混用现有多来源 Query 来隐式表达位置发现。复用内部目标调度与 SourceContext，保持一份 budget 和 deadline。

**Alternatives considered**: CLI 自行 discover all、逐来源调用各自 300 秒预算、在 Python 或 Desktop 实现自动选择。都会增加获取量、重复调度或改变网络边界。

## R07 — 当前资料只能建立候选，默认准入另行验证

**Decision**: 目录整理全部现有记录；产品默认选择只开放已完成访问、时间、几何和当前链路准入的产品。当前没有因为本次设计而新增默认准入来源。

**Rationale**: RDCAP 三国正常 TLS 与科学独立读回仍缺，MAKI 例外路径单列；PT 有来源图像定位证据但没有通用 native science decoder；RainViewer 的全球瓦片域不是全球雷达覆盖。按证据建立不同层次的候选和能力，不把一张成功原图或站点坐标当作完整覆盖。

## R08 — Tropical Globe 用作交叉核对材料

**Decision**: 使用用户指定的 [CSV 导出页](https://tropicalglobe.com/radar_database/#show_export=1)比对已有站表，不直接导入全球查询目标。完整导出 1,867 条，含关闭、私有与不可用类别；默认导出只有 1,615 条。取得日期、hash、过滤条件和全部 95 条初筛见 [crosscheck](tropicalglobe-crosscheck.md)。

**Rationale**: 88 条有同国家 5 km 内候选，7 条没有；名称冲突、共址与迁站需要不同关系。JPTAKA 的近距候选为 Osaka，RCMD/RCWF 在外表一行，Sorong 最近记录为 closed 旧站。外表注释也包含估计图像范围与未确认最大距离，range 不能直接成为覆盖圆。当前运行环境没有已安装的 pyproj，本次使用 Haversine 近似作目录差异初筛，未作椭球投影验证。

**Provenance**: 同次 JSON 导出头声明 Tropical Globe 与 LGPL-3.0-or-later；JSON 列表尾随逗号导致严格解析失败，合法 CSV 是本次比较输入。原件仅在本机临时材料中保留，未纳入内置资源；目录及其链接图像的分发边界分别核对，见 [来源文档](../../DATA_SOURCES.md#站点参考目录)。

**Alternatives considered**: 只用默认导出、按最近坐标合并、统一采用外表 range、把所有全球站直接加入 discover。都会丢失隐藏类别或产生未经核实的身份/覆盖/可用性声明。

## R09 — 旧项目用于追溯注册与站表生成过程

**Decision**: 两个子代理分别只读核对 `caiyun-radar-icu` 的投影和站表，固定 HEAD `54972debade9a8e98de672fd7db9ad902ca9e6ff`。证据与对照结果集中在 [legacy-reference.md](legacy-reference.md)。不执行旧流水线，不读取其凭据。

**Rationale**: 旧转换是 EPSG:4326→3857，再按矩形裁剪/缩放，没有用户中心椭球 AEQD。可参考完整瓦片外边界、图像北到南的方向和 TMS 请求翻转；混合 lon/lat、x/y 顺序、两角点注册、固定 461×461 缩放和像素圆 mask 均不能直接迁移。旧站表还能追溯半径/bbox 的生成方法，但不能认证真实扫描覆盖。

旧混合目录 704 项，不能视作实体站数；当前 SIDARMA 47 项 ID/坐标/bbox 与旧列表逐项完全一致。旧印尼裁剪目录的 44 项显式关联中 38 个 bbox 不同，旧 SIDARMA 又跳过 IWJ/MCRC/PKN。因此保留材料类型、历史变体和待核原因，不直接合并或继承 skip。

**Alternatives considered**: 直接移植 Python crop/站表、把旧解析输出当独立投影参考。前者丢失坐标与质量语义，后者只重复同一组假设。独立参考继续使用 PROJ/GeographicLib；来源注册另外校验。

## R10 — 接入当前 Engine、配置和资源边界

**Decision**: 实施入口为 `Engine.local_view`，复用一个 operation、request coalescer、绝对 deadline、RequestBudget 与 RasterMemoryBudget；在 Core 中完成空间选择和投影。当前代码基线为 `fee15193ec9d69d08bf27c9405f7c501486d17e9`。

**Rationale**:

- [config.rs](../../crates/radiust-core/src/config.rs) 的 serde 与 key whitelist 均严格，必须同步加入 `location/local_view` 的默认值、校验和 origins；当前优先级为默认值 < YAML < 环境 < 显式覆盖。沿用 `RADIUST_` 前缀和 `__` 分段，例如 `RADIUST_LOCATION__LONGITUDE`。旧配置没有 location 仍合法。
- [engine.rs](../../crates/radiust-core/src/engine.rs) 的 `discover_seeded_inner` 每次建立新的 deadline 并刷新 RDCAP。局部请求应抽取现有内部刷新/单目标调度，传入统一绝对期限，不能连续调用八次 public discover。网络、frame 和 CPU 阶段都受剩余时间约束。
- [limits.rs](../../crates/radiust-core/src/limits.rs) 已有 RasterMemoryBudget/lease，但 `Limits.raster_memory_budget()` 每次创建新计数器，现有 native decode/regrid 没有跨阶段共享它。local operation 只创建一次，并在既有 decoder/图像读入的分配点传入同一预算；检查真实头部/shape 后预留，再分配，不另写 decoder。计入同时存活的源、输出、质量、RGBA 和临时缓冲，输出对象以私有 owner 持有 lease 至缓冲释放。现有 spawn_blocking 取消检查不足以中断像元循环；新循环按块检查，迟到结果丢弃，permit/lease 持有至 worker 真正结束。
- [catalog.rs](../../crates/radiust-core/src/source/catalog.rs) 的 metadata 已支持扩展键；空间子结构无需修改外层 schema。ID/SIDARMA 当前各自 include 来源资源，SIDARMA 的已校验库存可提供内部只读空间视图。
- 当前没有通用 native 来源 JSON loader，PT/SG JSON 也没有内置 Rust 镜像。PT 几何通过准入后才按 ID 的 `include_str!` 模式读取既有 `sources/pt.json` 的必要字段，由 [release/prepare.py](../../scripts/release/prepare.py) 同步和逐字节检查；不手写另一份几何常量或新增全部来源的通用加载框架。
- PyO3 typed 请求/结果和 awaitable 接口落在 [python_types.rs](../../crates/radiust-core/src/python_types.rs)，注册经过 [python.rs](../../crates/radiust-core/src/python.rs) 现有入口；Python `_bridge` 和 `rust_client` 同步/异步方法只委托它。CLI 的 local 分支在旧 `(file,source)` 必选其一及 raw 默认校验之前路由。

**Alternatives considered**: 独立预算、每来源重新计时、直接扩 all、Python 采样、维护重复坐标常量。现有模块已有相应能力，新增这些路径会破坏限额、身份或兼容性。

## R11 — 图像外框不直接认证像元 CRS

**Decision**: 几何准入同时核对 bbox 的边缘/中心含义、实际数据窗口、像元所在 CRS、图像尺寸与 frame receipt。PT 原图分支保留为验证目标，在完成这些控制点之前不自动准入。

**Rationale**: 当前 [PT 资源](../../python/radiust/resources/sources/pt.json) 把 Leaflet ImageOverlay 经纬外框解释为线性 EPSG:4326 像元中心；[历史验收记录](../../validation-results/us6-inventory.md)也采用此解释。Leaflet 1.9.4 的 ImageOverlay 实际在地图投影后的西北/东南角之间缩放图像，默认地图 CRS 为 EPSG:3857。因此仅有经纬外框不足以推出线性纬度轴；这是对 PT 现有解释的潜在冲突，尚未确认官方页面使用的 CRS、Leaflet 版本和内部控制点。依据：[ImageOverlay 源码](https://raw.githubusercontent.com/Leaflet/Leaflet/v1.9.4/src/layer/ImageOverlay.js)、[默认 CRS](https://raw.githubusercontent.com/Leaflet/Leaflet/v1.9.4/src/geo/crs/CRS.EPSG3857.js)。

2026-10-10 重新访问官方 `mapbuilder-md.js` 未取得内容，本次没有更新来源 geometry_status 或改写历史证据。实施时保存官方实际脚本/地图 CRS 与 hash，核对至少一个非角点位置，确定是 4326 中心轴、3857 边缘注册还是不支持的映射；无法确定则 `geometry_unverified`。该验证步骤已经明确，待取得的来源证据是准入 gate。

**Alternatives considered**: 信任资源中的 verified 字符串、检查四角后视作整幅正确、直接改标成 3857。都不足以认证当前图像的像元映射。

## R12 — 2026-10-17 T052 默认用途准入评估

**Decision**: 建议保持 `rainviewer/composite` 的自动准入为 false，T052/SC-008 不关闭。RainViewer 官方 API 页将免费用途限定为个人、教育及小规模社区；其 Terms of Use 条目只写个人或教育用途，未定义“小规模”，FAQ 明确高流量商业应用并非该免费 API 的目标，商业集成需逐案协商。页面要求应用内显示带链接的署名，但也说明不与底层雷达数据所有者签约、所有者可要求移除或停止供数；改图许可不能证明底层观测数据可再分发。上述范围不足以明确授权 Radiust 面向公众的广泛默认使用或底层数据再分发。[RainViewer 官方 API 条款（2026-10-17 核对）](https://www.rainviewer.com/api.html)

**Evidence boundary**: T051 的安装后显式 RainViewer composite 请求及同帧 CLI／同步／异步回放证明该帧可获取、转换，独立 Web Mercator 像素控制支持该测试点的瓦片坐标映射；记录仍标记 `needs_probe_coverage_unknown`，不能证明雷达观测覆盖或其他地区可用。[T051 记录（2026-10-10）](../../validation-results/006-local-view-live.json)是唯一成功的 T051 产品链路；其中 IPMA 只在发现阶段返回 `no_data`，没有获取或投影，CWA 不在该 T051 验收范围内，因此两者均不构成此次默认用途候选。

## R13 — 2026-10-24 T052 个人用途准入复核

**Decision**: 按用户确认的产品边界——用户在自己的设备上作个人使用——将 RainViewer `composite` 的 `local_view` 标记为可自动选择；安装后 CLI 人类报告显示 `Weather data by RainViewer` 并链接至 RainViewer。准入不覆盖商业或高流量集成，也不授予底层观测资料再分发权。当前官网说明免费 API 可用于个人／教育用途、要求应用显示带链接的署名，并说明其不与底层资料所有者签约且不保证持续供数；商业集成需个案协商。[RainViewer 官方 API 条款（2026-10-24 核对）](https://www.rainviewer.com/api.html)

**Evidence boundary**: 2026-10-24 的安装 wheel 裸 `cat` 在默认来源流程中成功发现、获取并生成 50 km/1 km 的 101×101 dBZ 视图；wheel hash、隔离配置、网络 opt-in、时间、输出署名与当前帧见 [T052 记录](../../validation-results/006-local-view-live.json)。空间关系仍为 `needs_probe_coverage_unknown`；Madeira 点的当前帧有效域和独立瓦片坐标控制都不证明观测覆盖，其他地区也未由这次请求验收。2026-10-17 R12 保留为当时的历史判断，其“默认用途过宽”前提由本次用户明确的个人用途边界取代。
