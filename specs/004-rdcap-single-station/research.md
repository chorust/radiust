# RDCAP 单站支持研究决策

日期：2026-10-01。研究输入为 [规格](spec.md)、[网站分析](../../docs/rdcap-single-station-analysis.md)、[三国样本与目录](../../validation-results/rdcap-analysis/)，以及当前 Rust Engine/CLI/PyO3 实现。本文件定义待实施设计，不声明新增 adapter 已存在。

## R01 — 实现归属与扩展点

**Decision**：新增 Rust `RdcapSourceAdapter`，实现 `SourceAdapter::discover` 与 `fetch_raw`；新增受限数值 decoder，由 `Engine::decode_science_inner` 分派。CLI 和 Python 薄绑定共用 Engine，既有四种 writer、cache、manifest-last commit 继续复用。

**Rationale**：`crates/radiust-core/src/source/mod.rs` 已有来源专用获取 hook；`engine.rs` 已管理帧期限、原始缓存、CPU semaphore 与离线 manifest 重放。当前科学分派仅支持 RainViewer composite 和 TW grid，新增注册本身不会开放科学能力。[roadmap](../../.specify/memory/roadmap.md) C-02/C-10 明确当前实现归属为 Rust。

**Alternatives considered**：独立 Python provider 会形成第二条业务路径；复用 `tw`/`ph` ID 会破坏既有来源含义；通用图像反查色标会丢失网站实际提供的连续数值。

## R02 — 目录、动态目标与身份

**Decision**：内置 48 个去重目录身份供离线 list；国家代码固定为 TWN/JPN/PHL，站码来自目录。在线发现通过新增可选 adapter 目录 hook 在目标调度前刷新一次，并与快照合并。刷新成功后当前在线目录中的站点为当前目标；仅快照保留项也继续可查、可选，标明未在最新目录出现，不能自动判定 retired。短码只在完整可见目录中唯一时归一化，歧义和未知显式返回错误。默认 list 保持离线；不新增专用 CLI 国家参数或专用 Python 获取 API。

**Rationale**：`source/catalog.rs` 读取同一份 `python/radiust/resources/catalog.json`；`engine.rs` 在调用 adapter 前根据该静态目录过滤站点，单纯在 adapter 内归一化会使短码和新站点被提前丢弃。源级一次目录解析必须先于过滤与目标统计。49 条三国记录包含重复 PHL/BALE，形成 48 个目标；catalog 的 Active 标签不能代替实时可用性。

**Alternatives considered**：只写静态目录无法发现未来新站；一个 station=None 总目标无法为无数据站提供独立终态；按国家新增三个 source ID 与规格不符。目录刷新失败时保留已知目标并继续逐站请求；未知显式目标返回 catalog_unavailable，不能把刷新失败解释为 unknown_station。

**兼容边界**：目录 hook 缺省返回 None，其他来源继续原路径。CatalogSource/CatalogProduct/CatalogStation及其Python DTO增加可选 `metadata` 映射，保留国家、近期查询能力、原始状态列表、冲突说明、快照日期与能力证据；不提升 JSON schema_version。Python StationInfo坐标类型允许None，RDCAP映射未知坐标为None，已有来源映射和值保持原行为，本次不顺带改写其他来源元信息。

## R03 — 国家前缀/站码与路径安全

**Decision（2026-10-07 更新）**：RDCAP 公开 station 使用两字母国家前缀与站码直接拼接，例如 `TWRCHL`、`JPMAKI`、`PHSUBI`，语法为 `(TW|JP|PH)[A-Z0-9]+`。Rust 和 Python 共用这一公开身份合同；上游请求及来源元数据仍使用 `TWN`、`JPN`、`PHL`。短站码仍须在完整目录中唯一匹配。旧的含斜杠选择器不再接受，历史验证报告保留当时的 ID。

**Rationale**：公开站点 ID 与其他来源一致，不包含路径分隔符。输出模板 `{station}` 直接使用公开 ID；目录刷新和请求生成通过 RDCAP parser 还原上游国家及站码。identity schema 不变，新的 station 字符串会生成新的帧 hash。

**历史决策**：原规格使用 `TWN/RCHL`，输出模板编码为 `TWN%2FRCHL`；本次按用户要求和更新后的 FR-002 替换。来源范围、科学能力和在线验收状态不因此改变。

## R04 — 时间线与歧义

**Decision**：POST `get_radar_data` 使用 `datetime=""` 取得当前滚动索引；key 按有检查的整数 epoch milliseconds 转 UTC，保留毫秒精度。latest 逐站选最大真实时次；at 精确匹配；range 为 `[start,end)`。无长期归档声明，`historical=false`，近期 at/range 能力单列 metadata。

**Rationale**：三国观察到包含秒的非等间距时刻，约两小时窗口不是服务保证。`engine.rs::select_frame` 已采用精确 at 和半开区间。`datetime` 历史参数格式没有成功证据，不能猜测。当前 selector 会对部分同时间冲突候选取第一帧，RDCAP 必须先检测冲突并报告 ambiguous，不改变其他来源选择。

**Alternatives considered**：根据分钟/固定 cadence 拼 URL、对 at 做最近邻匹配、把网站参考时区再转为日本当地时间均会制造错误时间。重复 key 的不同临时 ticket 不足以证明不同 revision；只有稳定记录描述冲突或获取后的内容差异才支持冲突判断，不能因票据变化产生歧义。

## R05 — 自动获取、单次读取与统一预算

**Decision**：原生 HTTP 为唯一首轮生产获取路径，复用 `HttpTransport::post_form_bytes_with_headers` 和同源受限流式下载；AJAX POST 携带 `X-Requested-With: XMLHttpRequest` 与对应站点 Referer。每个选中 file ticket 只发送一次 GET，不 HEAD、不预读、不对同 ticket 自动重试。增加共享 HTTP 的可选 single-attempt 请求策略（缺省保持现有最多三次），文件请求同源 HTTPS `rdcap.cwa.gov.tw/file`。失败或 HTTP 200 空字符串时最多刷新索引两次，始终匹配原 key；最多三张 ticket/三次文件 GET，全部计入同一帧期限与请求预算。

**Rationale**：实测新 ticket 第一次有内容、第二次为 `""`；原因及寿命未有官方说明。`HttpTransport` 已有 form POST、host/request permit、取消与字节上限，但 GET 路径存在内建重试，不能直接对单读 ticket 套用默认策略。网络流失败可能发生在票据被消费之后，下一次必须重新申请，而不是重读旧 URL。

**Alternatives considered**：解 JWT 并自造 URL 不可接受；GET 同 ticket 重试不可靠；自动以最新帧替换原 key 破坏身份；curl 子进程或要求人工维持 Orca 页面不满足 FR-008。浏览器 fallback 未纳入本次生产架构；若原生路径被上游限制，保留在线验收阻塞，而不是偷偷扩充浏览器依赖。

**证据边界**：浏览器已获取三国文件；独立 HTTP 索引取得成功；浏览器外 file 获取曾超时，本机研究 curl 曾用 `-k` 排查证书链。当前这些证据不能证明 reqwest 的完整在线获取。实施先验证三国原生 HTTP 链路和 TLS，成功后才能更新在线能力；不得默认禁用证书验证。

## R06 — 原始保存与稳定身份

**Decision**：首先保存 file 响应的原始 JSON 字符串字节为 `file-response.json`，单次流式落盘并算 SHA-256；解析结果只用于验证/解码，不改写该 artifact。配套 `binding.json` 保存确定性 country/station/key/valid_time/file_sha256、协议版本及固定提供者地址，证明“索引选中的帧”与文件绑定；不包含票据、响应取得时间或随机值。原始 manifest 复用 v1，safe locator 仅含稳定 country/station_code/key 和版本，票据只在 `url`/`headers` 等既有过滤字段或内存私有结构中持有。

**Rationale**：CSV 不自带观测时刻，不能将请求时间冒充有效时间。`identity.rs::safe_locator` 会递归删除 url/token/headers，但不删除任意名为 header 的字符串，所以不得把 upstream 的 `header` ticket 塞入公开 locator。`raw_manifest.rs` 已校验摘要/路径和安全 locator，离线解码应读取完整 frame CSV 内嵌头，不依赖额外的三行 header ticket。

**Alternatives considered**：只保存 CSV 会丢失 HTTP 原始响应表达；保存完整未脱敏 index 会泄露临时凭据；在 revision artifact 中加入当前获取时间会破坏幂等复用。已有 `frame.csv` 是浏览器解析后的内容证据，不能称为完整 HTTP 原始字节；将其构造为测试 JSON envelope 时必须标注 reconstructed fixture。

## R07 — 有界稀疏网格解码

**Decision**：首版只解释已验证的 `T/int16/EPSG:4326`、`linearTransform(0.1000,0.0000)`、default/invalid=-999 及相同色标格式。解析 JSON string 再按七行 CSV 解析 compressed sparse row：ndv=NNZ，colIdx/vals 长度 NNZ，rowPtr 长度 H+1；先验证尺寸乘法、预算、数组长度/边界/顺序，再分配 dense f32/u16。每行 colIdx 必须递增，禁止重复。未知注册/CRS/变换/色标及结构损坏明确拒绝科学解码；合法原始响应仍可保留。

**Rationale**：这不是图片回波，也不是极坐标体扫；实测 RCHL 为 901²，ISHI/SUBI 为 900²，维度不是统一常数。科研值为 raw×0.1，不应从色阶反演。原生 decoder 使用 spawn_blocking 和现有 decode_workers；有效全缺测帧与空 HTTP 内容区别处理。

**Alternatives considered**：放行任意 transform/CRS 缺少证据；先 dense 分配再验证容易超过资源上限；把 ndv 当 nodata 将毁坏数据。

## R08 — 数值、质量与标记推断

**Decision**：有效回波质量=0；缺测 -999 为 NaN+bit0。新增可选质量 bit6 `source_annotation`，被验证规则排除的 9999 为 NaN+(bit0|bit6)=65；既有 bits0–5 含义和来源值保持不变。原始值及 `rdcap-annotation-v1` 推断依据由原始 artifact、字段 provenance、科学 writer flag 元信息保留。9999 规则仅作用于本次验证的 RDCAP 协议/头/色标组合；规则或标记模式无法核实时拒绝科学解码，不能把 999.9 dBZ 当真值。负值和低于5 dBZ 的有限值保持数值与quality=0，不标为缺测或 below_detection。

**Rationale**：三国 9999 在网页显示为白色范围圆/站心，是实测推断，提供者未正式声明 sentinel。使用 outside_coverage 或 unknown_color 不准确；增加独立质量位才能在导出后区分缺测与被排除的图形角色。bit6 为本功能必要的加法契约；不据此解决 roadmap 中其他历史质量文档冲突。

**Alternatives considered**：保留 999.9 作为科学值产生极端伪回波；全透明像元转 NaN 会丢失真实弱回波；仅写总数量说明无法逐像元追踪排除。未来未验证标记类型不自动沿用本规则。

## R09 — 几何与统一显示

**Decision**：由当前帧头计算 dx=(last_lon-first_lon)/(W-1)、dy=(last_lat-first_lat)/(H-1)，要求本版南→北行序和正间距。翻转行后 affine=[west,dx,0,north,0,-dy]，bounds=[west,north-H×dy,west+W×dx,north]；坐标为像元中心。native 输出 EPSG:4326，显式 geographic/regrid 复用已验收几何算法，RDCAP 加入该能力检查；dBZ 双线性仍在线性反射率域执行。

**Rationale**：`T` 是像元左上角注册，网站用 Web Mercator 显示不能说明源网格是3857。三站边界在 data-model 中列出，不能以站点坐标推测网格或按端点缩小一个像元。

**Alternatives considered**：当作中心注册或套用 TW grid 的 EPSG:3821 会偏移/错位；默默重投影改变 native 语义。

**显示决策**：新增版本化 `rdcap-reflectivity-v1` 离散 palette，默认由 RDCAP 字段的可信来源/decoder metadata 选择；`output/png.rs` 的 preview/write 共用选择与渲染规则，其他来源 default palette 不变。15 个5–75 dBZ 下界档，低于5透明但保留科学值；缺测/annotation透明；PNG sidecar保留身份、几何、palette/rule version。当前 PNG 只有六色渐变，所以这是实际新增工作，不能只登记颜色资源。

## R10 — 错误与验收证据

**Decision**：沿用报告 envelope/终态枚举，增加必要的 typed provider error→safe code 映射，而非靠字符串匹配。unknown_station、catalog_unavailable、no_matching_time、ambiguous_index、ticket_exhausted、selected_frame_disappeared、unexpected_body、decode_unverified、invalid_grid 有可辨代码和阶段；no_data/stale/timeout/network_restricted/cancelled 仍用现有终态。目录刷新也在一次 discovery 的总期限内；逐帧获取期限不被刷新重置。

**Rationale**：当前 CoreError 的一般 Transport 不足以区分票据耗尽、拒绝 HTML 与未验收解码，CLI/SDK必须共用分类，在error_contract.rs增加必要ErrorCode枚举，SDK ErrorContext增加可选code。源码中的count=24 sources/26 targets为旧快照断言，新增后25/74；003历史验收基数保持原含义，测试区分旧覆盖集合与当前总数。

**Alternatives considered**：HTTP200直接视为成功、fixture代替live、skip当验收通过均违反规格。SDK discover沿用refs/异常语义；新增薄入口discover_report和replay_raw_manifest公开底层已有能力，collect批量保留完整discovery_report并处理成功refs，不能被现有SDK首错抛出路径丢失。接口加法见cli-sdk合同，不发明全成功结论。

## 研究收尾

设计选择均已明确，没有待用户决定的技术项。保留的实施风险是原生网络完整链路和现场证据，而非未定的架构选项。实际扫描高度、上游QC和长期archive明确未知且不在能力声明中。

按用户建议尝试 Orca supervised GPT-6-Luna worker（run `run_63359da2e550`、task `task_e3ee51555d81`、dispatch `ctx_d44be89140fb`，requested/effective均为gpt-6-luna/high）；Orca agent_readiness超时，任务未送达。该任务终态为failed，worker终端已按精确release流程关闭。资料审计由协调者完成，不归功于worker。

2026-10-01设计收尾验证：七份plan产物存在、相对文档链接有效、无模板占位，未生成tasks.md；本地研究decoder重放三国后shape/bounds/counts/min/max与基准一致，RCHL八参考点通过≤0.01dBZ/缺测检查。这仅验证研究内容和文档，不是原生adapter或live测试。Orca最终worker-list确认terminalState=released、liveness=exited，无遗留需回收worker。
