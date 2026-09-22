# Data Model: radiust v1

本文件定义目标模型，不表示当前骨架已提供这些类型。依据 [spec.md](spec.md) 与 [research.md](research.md)。所有公开值对象由 Python 定义；Rust 仅接收 I/O 请求、身份字符串和受管理资料，不解释科学变量。

## 共同约定

- 时间在边界接受带时区输入，规范化为 UTC，持久化使用 `YYYY-MM-DDTHH:mm:ss.ffffffZ`；不接受 naive datetime，不以 retrieved_at 替代 valid_time。范围为 `[start,end)`，必须 start < end。
- JSON 属性仅允许字符串键、有限数字、布尔、null、数组、对象；禁止任意实例、NaN/Infinity。身份对象递归冻结；metadata 不参与身份。安全日志另有白名单过滤，不直接打印完整实例。
- schema_version 从整数 1 开始。读取未知主 schema 明确拒绝，不猜测；向后兼容新增可选字段不改变既有字段语义。
- 标识 source/product/station 使用来源目录定义的稳定名称，不允许分隔符或路径遍历。数据科学单位不从文件扩展名推断。

## 目录、查询与帧

| 实体 | 字段 | 校验与关系 |
| --- | --- | --- |
| SourceInfo | id, description, adapter_version, products, stations, required_extras, availability, availability_evidence | id 唯一；availability 为 available/missing_dependency/needs_configuration/upstream_unavailable/retired；静态目录状态不是实时探测 |
| ProductInfo | id, variables, units, native_grid_kind, cadence, publication_delay, suggested_max_age, historical, forecast, default, time_binding_policy, mutable | 每来源最多一个默认产品；时间绑定策略与可变性必填，不能根据 URL 静态外观推断 |
| StationInfo | id, name, longitude, latitude, altitude, product_ids | station 可省略的合成产品另有描述；坐标范围/单位显式 |
| Query | source, product?, stations[], latest=false, at?, start?, end?, base_time?, max_age? | 三种时间选择恰一；max_age>0 且只适用 latest；base_time 仅预报；station 无重复；产品省略须唯一默认 |
| FrameRef | source, product, station?, valid_time, base_time?, uri?, locator, locator_version, metadata, revision? | 不可变；uri 为运行期线索，持久化投影去掉认证和签名；locator 定义的所有影响身份字段参与 hash |

发现顺序：按 `(valid_time, source, product, station-or-empty, base_time-or-empty, logical_id)` 稳定升序；latest 先按 source/product/station 找最新有效时间，再保留所有同时间候选，单帧入口遇到不同 base_time 仍报歧义。`max_age` 使用当前 UTC 与 valid_time，预报有效时间在未来不据此视为过期；来源发布/起报约束单独检查。

## 身份规范

`identity_schema=1`。canonical JSON 采用 UTF-8、sort_keys、separators=(',',':')、ensure_ascii=False、allow_nan=False；字符串不隐式 Unicode 归一化，列表保留顺序，时间使用上述固定格式。可等价的网格/查询参数先规范化再计算。Python 计算 hash 并提供 golden vectors，Rust 不另算对象序列化身份。

| 标识 | 输入 | 规则 |
| --- | --- | --- |
| logical_id | identity_schema, source, product, station, valid_time, base_time, locator_version, locator | 缺省 station/base_time 显式 null；SHA-256 完整 hex |
| resolved revision | 可信上游版本；否则原始 artifact 内容清单 | 上游版本附来源命名空间；内容清单按 name 排序，包含 name/size_bytes/sha256；mosaic 由原始清单导出，不用临时文件名 |
| processing_hash | ProcessingSpec | raw-only 与 decoded 不同；decoded 是否额外保存 raw 不改此 hash |
| output_id | identity_schema, logical_id, resolved_revision, processing_hash | 不包含 output root、并发、缓存目录和凭据 |
| variant_id | output_id 前 12 hex | 仅用于命名；完整 hash 冲突检查不可省略 |
| cache_key | logical_id, revision/validator, acquisition_version, mosaic_version? | 版本未知另存 revalidated_at/expiry；不会因为 key 稳定就认为内容永久有效 |

## 原始资料和所有权

| 实体 | 字段 | 校验与关系 |
| --- | --- | --- |
| Artifact | name, role, media_type, payload, size_bytes, sha256, source_revision? | role=data/metadata/tile；payload 为 bytes 或 ManagedPath 二选一；原始与派生产物显式区分 |
| ManagedPath | path, owner, lease? | owner 为 temporary/cache；用户 raw output 不属于此对象；路径内容由 owner 保活 |
| AcquisitionReceipt | retrieved_at, resolved_revision, upstream_validator?, acquisition_version, original_artifacts, time_binding_evidence | original_artifacts 保存无敏感信息的名称/大小/hash；派生 mosaic 不丢原始身份依据 |
| RawFrame | ref, artifacts, receipt, metadata, closed | artifacts 名称唯一；单帧聚合上限；实现 close 与同步/异步上下文 |

状态：`DISCOVERED → ACQUIRING → ACQUIRED → DECODING → RELEASED`。raw-only 从 ACQUIRED 直接进入输出暂存，跳过 mosaic/decode。异常或取消均进入释放路径。cache lease 的释放不等于删除缓存；temporary 在没有 worker 引用后删除。close 幂等，关闭后访问 payload 报明确错误。

默认科学数据在返回前 `.load()` 成独立数组；v1 不返回偷偷依赖临时文件的 lazy 数据。不可中断 CPU worker 延长输入 lease 至结束，取消后的结果丢弃。

## 科学对象、质量与空间

| 实体 | 字段 | 约束 |
| --- | --- | --- |
| RadarField | data: DataArray, quality: DataArray, grid: GridSpec, provenance: ProcessingRecord | 一个主要变量；quality 与 data 同形状/坐标；quality 不计第二个变量 |
| RadarDataset | data: Dataset, grid: GridSpec, provenance: ProcessingRecord | 多个主要变量，每个配 `<variable>_quality`；共享时间和网格，不隐式 align 异构数据 |
| ProcessingRecord | logical_id, revision, retrieved_at, source/adapter/software/decoder/resource versions, transformations[], receipt_digest | transformations 顺序有意义；保留 palette 与静态资源版本；无凭据 |
| VariableDescriptor | name, units, kind, standard_name?, flag_values?, flag_meanings?, nodata_code?, detection_threshold? | continuous 默认 float32/NaN；categorical 整数、独立 nodata；不能为无标准名变量编造 standard_name |

quality 为 uint16 位掩码，可组合：

| 位 | 值 | 含义 |
| --- | --- | --- |
| 0 | 1 | missing |
| 1 | 2 | outside_coverage |
| 2 | 4 | unknown_color |
| 3 | 8 | recovered |
| 4 | 16 | interpolated |
| 5 | 32 | below_detection |

0 表示无已知异常，不作为 quality 的 `_FillValue`。无雨 rain_rate=0，单位 `mm h-1`；reflectivity 单位 `dBZ`；level 单位 `1` 且保留类别映射。仅知道低于检测阈值时为 NaN+below_detection，不能赋 0 dBZ。未知颜色 strict 报错；permissive 为 NaN+unknown_color 并记录计数。其他 missing 位按实际缺测原因附加，不能仅凭 NaN 抹去其余质量信息。

| GridSpec 分支 | 坐标与必填字段 | 验证 |
| --- | --- | --- |
| GeographicGrid | latitude/longitude 一维中心坐标、CRS、shape、extent | 轴单调且匹配数组；不规则一维轴显式保存；extent 为边界 |
| CartesianGrid | y/x 中心坐标、CRS WKT、shape、extent、规则时 affine | 单位由 CRS 定义；affine 与中心/边界一致 |
| PolarGrid | azimuth/range、站点经纬高程、elevation、gate、beam_model | 方位正北顺时针，距离 m；投影记录波束假设 |
| CurvilinearGrid | 二维 latitude/longitude、索引维度、CRS、shape | 仅盘点确认存在后实施对应来源；禁止伪装成一维规则网格 |

`RegridSpec`：target_crs、extent、resolution 或完整目标 GridSpec、method=nearest/bilinear、missing_policy、beam_model?。所有 resolution 单位显式。v1 bbox 是经纬度 `(west,south,east,north)`；跨日期变更线拒绝；缺定位信息报 GeoreferencingError。类别只允许 nearest；dBZ 的 bilinear 在线性反射率域执行，保留插值质量标志。v1 missing_policy=strict：nearest 继承选中像元的值与全部质量；bilinear 只有四个贡献点均在覆盖内且有有效物理值时输出有效值，质量为贡献点标志的按位或再加 interpolated。任一贡献点缺测/站外/未知色/仅低于阈值则结果缺测并保留原因，不跨洞补值。

## 输出与报告

| 实体 | 字段 | 约束 |
| --- | --- | --- |
| ProcessingSpec | schema_version, output_kind, decoder_version, resource_versions, selected_variables, grid_transform, encoder, encoder_version, encoder_options | raw-only 不带科学处理选项；encoder 参数展开默认值后参与 hash |
| OutputRequest | root, format, template?, raw=false, raw_only=false, overwrite=false | raw 与 raw_only 互斥；raw_only 与显式科学输出选项互斥；root 不得与 cache/tmp 重合或互相包含 |
| OutputArtifact | relative_uri, role, size_bytes, sha256, media_type | 相对当前 generation 或本地 root；无签名 URL，路径不能越界；role 区分 decoded/raw/sidecar |
| OutputManifest | schema_version, logical_id, revision, processing_spec, processing_hash, output_id, generation?, raw_complete, artifacts[], created_at, supersedes? | 完成标记；内容、身份、文件完整性全部验证；一个不可变 generation 的文件列表不得后补修改 |
| RawManifest | schema_version, safe_ref, receipt, artifacts[] | 支持离线 acquire/decode 重放；明确原始资料角色和版本，不序列化 RawFrame 对象 |
| FrameResult | ref, status, data?, error? | status=success/failed/cancelled/not_started；成功有 data，失败有 error；未完成有原因 |
| BatchResult | items[], counts | 保留全部输入；稳定输入顺序；流式迭代单独按完成顺序交付 |
| DownloadItem | ref, status, output_id?, manifest_uri?, artifact_uris[], error? | status=written/skipped/failed/cancelled/not_started；不携带全量数组 |
| DownloadReport | schema_version, run_id, query, counts, items[], interrupted | 计数一致；中断仍保留已提交帧；日志与报告分流 |
| RadiustError | code, stage, source?, frame_id?, retryable, message, cause? | cause 不直接序列化；公开 message 脱敏；code 稳定 |

输出状态：`VALIDATED → DISCOVERED → ACQUIRED → [DECODED → REGRIDDED] → STAGED → VERIFIED → COMMITTING → COMMITTED`。raw-only 跳过方括号部分。skip 是完整性验证后的终态。COMMITTING 前已接受的取消进入 CANCELLED，不能再发布；最终提交已不可撤回时先核对实际结果，不能把已提交帧从报告中抹掉。输出故障恢复规则见 [storage contract](contracts/storage.md)。

## 缓存、配置和迁移证据

- `CacheEntry`：key、path、kind(object/mosaic)、size、checksum、validator、schema_version、created_at、last_accessed_at、revalidated_at、expiry、source、receipt。索引仅存可重建信息，运行期 lease 独立管理；stale tmp、过期、LRU 顺序回收。无长期 tile 条目。
- `EffectiveConfig`：runtime/cache/sources/storage/output 五部分，值与 origin 分开存；凭据用独立不可打印对象，导出快照不含密钥；map 深合并、list 整体替换，拒绝未知 key 与重复 YAML key。
- `RenderOptions`：variable、palette/version、vmin/vmax、target_grid?、max_columns/max_rows；continuous vmin<vmax，categorical 拒绝连续范围覆盖。
- `RenderedImage`：RGBA、像素尺寸、legend、title、display_metadata；不依赖 TTY。纯 PNG 文件预览不构造伪科学场。
- `MigrationEntry`：旧 commit/path/symbol、legacy_enabled、target_source/product、family、dependencies、fixture_records、status、migration_notes、exceptions。status 分 inventoried/fixture_ready/implemented/contract_passed/live_checked/retired_exception/accepted_exception；缺证据不可标完成。
- `FixtureRecord`：origin、许可/使用依据、采集时间、内容摘要、脱敏说明、关联帧、参考数值/质量/几何、容差、维护者。不能用只有合成数据的测试替代每源真实样本。
