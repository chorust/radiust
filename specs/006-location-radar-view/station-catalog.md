# 雷达站表与产品覆盖设计

**Status**: 目录设计、材料盘点与空间 metadata 读视图已实现；未新增站点、来源在线能力或数据授权。

## 1. 当前材料与整理结果

本次核对主目录、内置镜像、各来源资源及 adapter 中的 selector。完整逐条记录见 [station-inventory.md](station-inventory.md)。按用户提供的 [Tropical Globe 导出页](https://tropicalglobe.com/radar_database/#show_export=1)取得 CSV，并对 95 条站址候选完成[交叉核对](tropicalglobe-crosscheck.md)：88 条有同国家 5 km 内候选，7 条没有；候选不等于确认同站。

| 材料 | 现有记录 | 可用于什么 | 不能推导什么 |
| --- | --- | --- | --- |
| 主 catalog 的 RDCAP | 48 个唯一站点，台湾 13、日本 20、菲律宾 15，均有经纬度 | 来源内身份、目录位置、观测机构原文、状态冲突 | 当前有数据、扫描半径、全球实体站总量、默认准入 |
| SIDARMA 来源资源 | 47 条历史站址和 bbox，普通 id 同样有这 47 个站码 | 迁移站表候选、两渠道查询键核对 | bbox 是实际像元几何、覆盖半径已验证、坐标 datum 已核实 |
| MY 主目录 | 两个区域选择器及代表位置 | peninsular/east 产品选择 | 两个实体雷达站及其扫描中心 |
| adapter 固定 selector | ESCOMP、PTST2、SGCOMP、CV1_3600、PHCOMP4、TMD 两个 selector | 明确运行时过滤键 | 都是物理站，名称中的数字就是覆盖半径 |
| 其他 composite/tile 来源 | 主目录没有 station | 按产品定位与发布域筛选 | 无 station 就不适用，全球瓦片域就是全球观测覆盖 |

RDCAP 共 49 条上游目录行去重为 48 身份，PHBALE 同时有 Active/Inactive。95 条 RDCAP/SIDARMA 站址记录只是两个材料组的合计，不是已验证的全球物理站总数。普通 id 与 SIDARMA 不再复制为另外 47 个新站。

旧项目 [只读对照](legacy-reference.md)另外确认：混合目录 704 项包含产品入口与裁剪框；当前 SIDARMA 47 项 ID/坐标/bbox 与旧列表完全一致。旧印尼裁剪目录的 44 项显式关联中 38 个 bbox 不同，旧 SIDARMA 还跳过 IWJ/MCRC/PKN。不同材料不合并，旧 skip 不自动成为禁用或退役依据。

## 2. 最小目录结构

复用 `CatalogSource`、`CatalogProduct`、`CatalogStation` 和现有 metadata 扩展；不新建数据库、不修改公开站码格式。`metadata.spatial` 内显式版本化新字段，旧 catalog v1 仍可读取；缺失空间字段等于未知，不自动补默认覆盖。

| 层级 | 必需整理内容 |
| --- | --- |
| Source | 官方/第三方来源链、访问条件、退役状态、自动启用用途的准入证据 |
| Station/selector | 来源内 id、kind、原名、产品关系、坐标/基准/证据、目录状态与冲突 |
| Product/selector spatial | product_domain、observation_coverage、源几何与 CRS、像元注册、能力和证据 |

`kind` 为 `physical_station`、`region_selector`、`product_selector`、`composite` 或 `unknown`。station 的 longitude/latitude 继续存在；只有实体类别及坐标性质有依据时才能称为固定站址。区域代表点或产品中心必须标注自己的 role，不能进入“最近雷达站”排序。MCRC 等固定站属性未确认的历史条目保留 unknown；没有当前帧位置的移动雷达不能按历史位置进入固定站排序。

空间元信息的设计字段：

| 字段 | 语义 |
| --- | --- |
| `schema_version` | 当前拟定 1，与外层 catalog schema 独立 |
| `kind` | 上述目标类别 |
| `coordinate_reference` | WGS84、上游明确声明值或 unknown；记录 datum 不确定性 |
| `coordinate_evidence` | reference、observed_at/checked_at、status、冲突记录 |
| `external_catalog_refs` | 外部快照 hash/行、名称候选、关系类型、差异与审核状态；不作运行 Query ID |
| `external_range_claim` | 外部原始 range/unit、声明出处及不确定性；不等于 observation_coverage |
| `product_domain` | 上游发布/网格域，可信地理 bbox 或多边形；含坐标和几何依据 |
| `observation_coverage` | verified geodesic circle / polygon / coverage mask / unknown；必须 product-specific |
| `geometry` | 像元 CRS、x/y/affine 来源、center/edge registration、row_order、data_window、shape/版本/receipt 适用范围与验证状态；经纬 overlay 外框不等于线性纬度轴 |
| `admission` | 自动默认用途、状态、证据日期；与目录 availability 分开 |
| `evidence` | 来源链、日期、内容摘要与适用版本，URL 不含凭据/票据 |

不把原始请求 locator、cookie 或文件票据放入站表。range 支持、cadence 和 freshness 优先复用 CatalogProduct 已有字段，不重复维护另一套值。

## 3. 站址与覆盖的判定

**可信扫描圆**：上游明确给出适用产品的最大距离、地球/坐标模型与覆盖说明，并有独立核对，才可以用椭球测地距离作 point-in-circle。目录没有范围字段时不填 250/300 km。TMD 的 kkn240Loop、RDCAP 图面标记和 SIDARMA 的旧 bbox 都不能据名字/外观直接生成圆。

**发布域**：可靠 frame 网格可确定“产品可以定位的图像区域”。它用于候选粗筛与当前帧反查，不保证域内全部有扫描数据。PT overlay 外框只能先作地理域材料；其像元 CRS 与内部注册须按 [research R11](research.md)补核。RainViewer world tiles 同样只代表发布域。

**观测覆盖**：只有上游提供或已验证的 mask/footprint 才能标为已知；current missing 不能反推未覆盖，current nonmissing 不能反推整个域永久覆盖。缺测多的源不能因“有效像元少”自动降权为无雨/无覆盖。

**历史 frame 域**：绑定到具体 frame/decoder 证据与日期，只作候选探测提示。当前帧改变几何时重新校验，不沿用昨天的 bbox 强行投影今天的图。

**日期线**：bbox 表达包含 wrap 标记，或在地理层拆分两个区间。不能对 `179°/-179°` 用普通西小东大 bbox。多边形只在已支持且验证的格式下参与筛选，未经支持的 geometry 类型明确排除。

## 4. 数据整理与运行索引

1. 保留来源内 `(source,id)`；产品关联形成 `(source,product,station?)`。同名、同码、同坐标均不自动合并实体站。id/SIDARMA 两条渠道保留自己的 Query 键。
2. 主目录现有站址为默认材料；SIDARMA 已有资源库存校验可扩为内部只读空间视图，普通 id 关联同一组历史材料而保留自身 Query 键。提供方更新合并沿用现有 Engine 目录机制。禁止手工维护两份不同坐标。
3. 资源中的 47 个 BMKG 历史条目先核实坐标/年代与 adapter 选择键，再补入空间索引；其 bbox 不升级为几何。现有 all 的目标展开不因空间索引改变，原 74 目标快照仍为历史基线。
4. 产品几何元信息写既有来源资源，product/selector 指向同一条材料。当前没有通用 native loader，也没有 PT/SG Rust 镜像；仅为通过几何 gate 的 PT 路径按现有 include_str 模式读取必要字段，由 release/prepare.py 同步/检查其 Python canonical 与 builtin 字节。SG 本轮不新增 loader。逐条 inventory 是设计审计记录，不由程序加载为第二份生产目录。
5. 目录刷新只能更新有证据的字段，空值或冲突不能变成默认坐标；目录状态与当前观测状态分开。来源退役立即排除默认选择。
6. Tropical Globe 已完成针对现有 95 条站址的差异初筛；完整导出 1,867 条包含隐藏类别，不能由默认导出缺项推断退役。保留 URL/日期/过滤条件/hash/许可声明，按国家、名称/别名、机构及坐标建立审核候选，不自动覆盖。外部额外站点以后逐源补充，不批量新增未接入的全球查询目标。
7. JPTAKA 的 Takamatsu/Osaka 名称冲突、RCMD/RCWF 共址和 Sorong 新旧站分别核实；同一 CSV 行可以记录多个共址雷达，最近行也可能是已关闭旧站。关联关系先留在现有 metadata，不改变来源内 ID。六条 1–5 km 差异与七条无近距对应记录须保留待核状态。
8. 旧列表的 Stasiun 可作为 IWJ/PWK/NGW 别名线索，Git 入库日期不可充当采集/测量日期；旧裁剪框变体与 skip 记录只入审核材料。不能用它们补造覆盖半径或固定站属性。

## 5. 候选选择与有界探测

自动选择首先按默认用途准入、配置、退役和支持的几何路径筛选。用户显式选择来源时仍遵守该来源访问与网络条件，但不将“尚未取得自动默认准入”当作额外阻止其显式查询的理由。实时资料仍须通过 discovery 和当前 frame 检查。

- `inside_verified_coverage`：有产品级可信覆盖，且包含用户；优先级最高。
- `inside_product_domain`：只知道可靠发布域含用户；可尝试，报告实际扫描覆盖未知。
- `needs_probe`：有可用查询键和已验证的 frame geometry decoder，但没有完整 footprint；仅可有界探测，不能先宣称覆盖。站址可靠时距离用于排序。
- `outside`：可信覆盖或可定位的发布域明确不含用户，排除。
- `unknown`：没有支持的定位方法、坐标冲突或材料不够，说明缺口并排除自动投影。

先按上述几何证据层次排序，同层优先适用官方产品，然后按视图尺度所需的已有产品质量/分辨率证据、可信站距和稳定查询键排序；不建立不透明评分模型。各项使用可审计的离散证据级别，缺少质量/原生分辨率不推测。末级站距为 Option：可信固定站按距离升序，None 之后按 `(source,product,station)` 稳定排序；无可信站距的 composite/selector 不填 0，不因缺少站距被排除。

mode 先约束候选适用性，不用未校准 gray 数值提高排名。auto 对选定候选优先使用已验证 native 路径；仅没有该路径时使用可信定位 raw。native 校验失败是候选失败，不能改为 raw 后称成功；可在剩余候选内透明回退。auto+bilinear 只接受连续 native 数值，不选择 raw/类别再静默改采样。

首版最多尝试 **8 个目标**，使用同一个 `runtime.discovery_deadline` 覆盖刷新、发现、获取、解码、投影、展示和回退，各 frame/request 超时受剩余时间限制。抽取 Engine 现有内部刷新/单目标调度，复用同一 operation/coalescer；不能逐候选调用会重建 deadline 的 public discover。必要的 RDCAP 刷新至多一次，且遵守配置/准入筛选。候选目标先按上述顺序固定；同一目标最多一次进入源级流程，票据刷新仍由 adapter 的既有规则处理。

当前帧加载后检查用户是否在可信发布域，以及是否被已知 source_outside 排除；不合适才继续下一目标。没有可信 coverage mask 时允许诚实显示域内数据并标明覆盖未知。整个帧仅 missing、低于阈值或无彩色回波不能触发回退。候选达到 8 个上限须报告截断，不能报告“世界上没有覆盖雷达”。

## 6. 首批产品的实施顺序

| 顺序 | 产品 | 设计用途 | 仍需取得的证据 |
| --- | --- | --- | --- |
| 1 | RDCAP reflectivity | 站点选择与 EPSG:4326 数值投影的离线闭环 | 正常 TLS/准入/当前帧/逐国证据；覆盖不能由站址或研究图面圆推断 |
| 1 | RainViewer composite | 无实体 station 的合成产品分支 | 实际访问条款、当前链路与视图尺度；现有低 zoom 不承诺 1 km 原生质量 |
| 2 | PT Madeira 原图 | 有来源定位材料而无通用 native 数值 decoder 的验证分支 | 补核官方地图 CRS/内部控制点，解决线性 4326 解释的潜在冲突；再核数据窗口、当前 raw 链路、自动展示用途准入；未闭合保持 geometry_unverified |
| 后续 | SG 原图/类别 | 保留球面 AEQD 的来源模型 | 源球面转换、像元注册及推断误差核对；不在首版源 CRS 准入表 |
| 后续 | TW grid | 数值观测但 datum 转换未闭合 | EPSG:3821→WGS84 的明确方法、适用范围和独立控制点 |
| 后续 | BMKG/SIDARMA 站点图 | 补充实体站选择 | 坐标年代/基准、原始图像窗口、定位/色标及访问证据 |

以上是设计实施顺序，不是默认来源名单。首版实际开放须逐产品通过准入；现有未确认许可与 live 状态不因目录整理变化。
