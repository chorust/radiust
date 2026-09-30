# Data Model: Rust 核心迁移与性能优化

本文件定义跨原生 CLI、核心库及 Python 绑定共享的语义模型。字段名称为设计契约，不要求与现有 Python 内部类型逐字相同。正式成果身份和旧格式以现有 `python/radiust/identity.py`、`python/radiust/storage/manifest.py` 的可回放结果为兼容基准；来源科学能力仍以 `migration/` 台账为准。

## SourceCatalogEntry（来源目录项）

| Field | Meaning / constraint |
| --- | --- |
| `source_id` | 全局唯一、规范化的稳定来源 ID。 |
| `products[]` | 产品标识及可查询站点清单；目录未能展开时保留来源级占位目标。 |
| `lifecycle` | `active` / `retired`；退役来源不可发起获取。 |
| `capabilities` | 原图、legacy 显示、已验收科学解码、可用协议/来源专用依赖及限制原因；不能把发现成功推断为科学通过。 |
| `rule_versions` | 获取、显示、解码和地理规则版本，用于溯源与身份控制。 |

关系：一个来源包含多个产品；一个产品可包含零到多个站点，并展开为多个 `DiscoveryTarget`。登记范围是当前 24 个内置来源。

## Query 与 DiscoveryTarget（查询和目标）

| Field | Meaning / constraint |
| --- | --- |
| `sources[]` | 单一来源、显式有序来源列表或保留值 `all`；重复 ID 及 `all` 混用非法。 |
| `product`, `stations[]` | 单来源可选过滤；多来源查询禁止。 |
| `selector` | `latest`、`at`、完整 `start/end` 三者择一；多来源仅允许 latest。 |
| `base_time`, `max_age` | `base_time` 为单来源可选，`max_age` 非负且与适用的时间选择一致。 |
| `DiscoveryTarget.key` | `source/product/station` 的规范化组合；同一次展开后唯一。 |
| `DiscoveryTarget.ordinal` | 展开顺序，用于调度公平性和报告映射，不决定最终排序。 |

验证：所有参数在网络请求前验证。时间为有时区的 UTC 时刻；整批 `deadline` 自目录展开开始计时。目标状态按 `source/product/station` 稳定排序；不可展开的来源保留来源级结果，不凭空丢弃。

## FrameRef 与 RawFrame（帧与原始资料）

| Field | Meaning / constraint |
| --- | --- |
| `FrameRef.source/product/station` | 对应目录身份；不能依赖含密钥 URL 作为公开身份。 |
| `valid_time`, `base_time` | 有时区 UTC，未知时间不能由图像外观猜测。 |
| `logical_id`, `revision` | 既有稳定身份及可信上游修订；mutable latest 无可信修订时须重新确认。 |
| `locator` | 获取所需的私有定位信息，不进入对外安全报告或正式清单的秘密字段。 |
| `RawFrame.artifacts[]` | 每个原始组成件的名称、媒体类型、大小、SHA-256、受限存放位置与租约。 |
| `RawFrame.receipt` | 原始资料与所选 FrameRef 的身份、修订及完整性确认。 |

关系：一个查询目标可发现零到多个帧；一个帧可包含多个原始 artifact。获取完成后才形成有效 RawFrame；关闭/取消释放临时资源和缓存租约，正式 raw 成果不随 RawFrame 生命周期删除。

## RadarField / RadarDataset 与 Preview（科学与显示）

| Field | Meaning / constraint |
| --- | --- |
| `RadarField.values` | 科学值或类别值及形状；连续值保持既有 `float32` 与缺测语义。 |
| `quality` | 对应位置的 `uint16` 质量位；零值表示有效，缺测必须通过非零质量位表达。当前保留位为 bit 0 缺测/透明、bit 1 网格外、bit 2 未知类别/颜色、bit 4 插值、bit 5 恢复值；未知位必须原样保留。 |
| `grid` | 网格形状、坐标、CRS/仿射信息及时间对应关系；转换后保持有效地理信息。 |
| `units`, `variable`, `provenance` | 变量、单位、来源、规则/处理版本及原始证据指纹。 |
| `RadarDataset.fields[]` | 多变量集合；字段共享可核对的帧与坐标关系。 |
| `Preview.pixels` | 原图或已验证 legacy 显示像素、尺寸、透明度、已知时间与显示模式；不代表科学结果。 |

验证：只有台账已验收的来源/产品可构造正式科学结果；预览不改变科学能力。`NaN` 只能表示带非零质量位的缺测/无效样本；正负无穷值一律无效。绑定可共享核心拥有的数据，显式 `to_xarray()` 时再转换；转换结果必须保留值、质量、坐标、时间。

## DiscoveryItem / Report 与 DownloadItem / Report（任务报告）

| Field | Meaning / constraint |
| --- | --- |
| `schema_version` | 对外机器报告维持 v1，新增字段不得改变旧字段类型/含义。 |
| `query`, `run_id` | 安全查询投影与任务身份；多来源报告增加 `sources` 列表。 |
| `items[]` | 每目标或每输入帧恰一个状态，附身份、可选结果/安全错误；发现最终稳定排序，下载最终输入排序。 |
| `counts` | `total` 等于各终态数量之和；所有旧状态字段保留。 |
| `interrupted` | 用户中断标志；中断退出状态 130 优先。 |
| `error` | 仅用于顶层参数/整体错误，不吞掉部分成功项。 |

发现状态：`pending → running → success | no_data | stale | missing_credentials | retired | network_restricted | upstream_failed | ambiguous | timeout | cancelled`；在截止/取消前未派发者 `pending → not_started`。状态为终态后不可再被迟到结果覆盖。每目标恰一个终态；`timeout` 表示已运行且过期，`not_started` 表示未发起请求。

下载状态：`pending → planned`（dry run，不获取或提交），正常执行为 `pending → running → written | skipped | failed | cancelled`，未派发则 `not_started`。有 `on_error=stop/raise` 时停止派发，已经完整提交的 `written` 保留；提交响应未知时不能宣称 `written`，须提供安全复核状态/错误。

## CacheEntry 与 OutputManifest（缓存和正式成果）

| Field | Meaning / constraint |
| --- | --- |
| `CacheEntry.key/validator` | 稳定帧及获取规则身份、上游验证器；缓存可过期且可驱逐。 |
| `CacheEntry.object_hash/size/expiry/lease` | SHA-256、大小、有效期和在用保护；与现有 Python `entries` 索引兼容。 |
| `OutputManifest.schema_version` | 继续识别 v1。 |
| `logical_id/revision/processing_hash/output_id` | 旧成果读回和幂等跳过的完整身份；处理语义变化应改变相应版本/哈希。 |
| `artifacts[]` | 相对路径、角色、媒体类型、大小、SHA-256；集合完整后才发布清单。 |
| `raw_complete`, `generation`, `supersedes` | 原始集合完整性及适用的远端 generation/覆盖关系。 |

关系：RawFrame 可以来自缓存或新获取；只有已验证且完成提交的 Manifest 代表正式输出。缓存清理绝不修改正式成果。提交状态为 `staging → verified → publishing → committed`；失败时转 `aborted/incomplete`，发布响应未知时转 `outcome_unknown` 并读回核对，不把半成品标记为成功。

## Cross-entity invariants

1. 目录、发现、获取、科学处理、输出都使用同一规范帧身份；源定位 URL、token 和私密 metadata 不得进入公开报告。
2. 并发不改变目标/帧的最终报告排序，且不会给同一目标或帧生成多个终态。
3. 科学输出的数值、质量位、坐标和处理版本必须可从正式成果读回；原图/legacy 显示不是科学验收证据。
4. 旧缓存合法条目和 v1 正式成果可识别、校验；不合法条目不得参与 skip。正式输出以最终有效 manifest 为可见完成边界。
5. 整批发现和下载共享运行上下文中的网络许可、时间/容量/并发限制及取消信号；来源不能单独绕过预算。
