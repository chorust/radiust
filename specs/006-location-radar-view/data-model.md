# Data Model: 位置视图

**Status**: 设计模型；复用既有领域类型，不新增全局站点数据库。

## LocalViewRequest

| 字段 | 语义与校验 |
| --- | --- |
| `center.longitude/latitude` | 请求未给 center 时使用配置 location；Core 统一补全后必须为成对、有限、范围合法的 WGS84 度，经度按统一规则规范化；部分显式 center 不从配置补另一个坐标 |
| `radius_km` | 默认 50，有限且 `0<radius≤500` |
| `resolution_m` | 默认 1000，有限且为正；网格预算在分配前检查 |
| `source/product/station` | 可选显式限制；station/product 必须有 source，不跨来源解释短码 |
| `mode` | `auto`、`raw`、`decoded`、`dbz`；auto 对选中候选优先使用其已验证 native 数值路径；仅没有该路径时使用可靠定位原图。native 校验失败记为该候选失败，不以 raw 掩盖；decoded/dbz 不回退 raw，decoded 保留实际物理量 |
| `resampling` | nearest 默认；bilinear 仅受支持的连续数值，auto 下不得绕过原图限制 |
| `selector` | 首轮 latest；不得用位置视图改写 SDK Query 的时间选择要求 |

Core 配置增量为 `location: {longitude, latitude}` 与 `local_view: {radius_km, resolution_m}`。location 整项可空，部分坐标无效；旧配置和显式 source/file 不要求位置。同步扩展当前严格 root/section whitelist、serde 默认/Default、成对校验和 origins，沿用默认值 < YAML < 环境 < 显式覆盖。环境字段为 `RADIUST_LOCATION__LONGITUDE/LATITUDE` 与 `RADIUST_LOCAL_VIEW__RADIUS_KM/RESOLUTION_M`，不新增配置解析器。

请求可确定的非法值、checked shape/像元/最低输出内存超限在联网和分配前拒绝；源尺寸取得后的同时存活内存另作运行时检查。不得降分辨率来隐藏超限。`auto+bilinear` 只筛选连续 native 数值路径，不选择 raw 或类别后悄悄改回 nearest。

## 空间目录视图

复用 `CatalogStation`/`CatalogProduct` 及 metadata；新空间子结构 `metadata.spatial` 有自己的 `schema_version=1`，具体见 [站表设计](station-catalog.md)。来源内 station ID 稳定，source/product/station 组合形成查询键。

既有 catalog station 与 adapter 自有站表可供内部 `SpatialTarget` 读取，不能因建立空间索引就把后者批量加入 `expand_targets(all)`。空间索引是派生读视图，保留资源出处；主目录与内置资源镜像按现有打包流程一致维护。

外部目录关联写入现有 metadata，保留 snapshot hash、行号、候选名称、审核状态与 same_site/colocated/successor/conflicting 关系。Tropical Globe 没有统一可连接的 station ID，其 range 单列为未验证声明。unknown 类别及固定位置未确认项不参与实体站距排序；共址与迁站不改写原 Query identity。

## SpatialCandidate

| 字段 | 语义 |
| --- | --- |
| `target` | 复用 DiscoveryTarget；仅实际支持的 source/product/station |
| `target_kind` | physical_station / region_selector / product_selector / composite / unknown；unknown 不能据站距取得覆盖资格 |
| `site_distance_m` | 有可信站址才提供，用于探测排序，不证明覆盖 |
| `spatial_relation` | inside_verified_coverage / inside_product_domain / needs_probe / outside / unknown |
| `access_state` | 复用来源/配置状态，附自动准入证据；目录 available 不足以准入 |
| `geometry_state` | verified / declared / unverified / conflicting / unsupported |
| `evidence_refs` | 安全的本地或公开依据、观察/核对日期、版本 |
| `rank_reason` | 本次排序理由；不设置不透明学习分数 |

## LocalViewResult

复用原始 `FrameRef`/receipt、`RadarField`/图像数据和 Grid，外层保留选择与视图元信息。

- `status`：selected / no_candidate / failed / interrupted / timed_out。合法请求的终态与诊断由本结果承载；参数/配置预检错误沿用 EngineError。
- `selected`：仅 selected 时存在；实际 source/product/station、valid_time、输入 identity。
- `selection`：每次排除、未开始、尝试和终态；回退链；`candidate_limit_reached`。
- `projection`：AEQD 方法及版本、WGS84 椭球、中心。
- `grid`：成功时的 shape、x/y m、affine、请求/实际间距、radius；失败报告可保留预检所得 shape，但不伪造已生成数据。
- `data`：仅 selected 时存在；原生数值与 quality/伴随平面，或可靠定位的原图 RGBA；mode_info 与图例保持实际能力。
- `in_view`：圆形请求范围 mask；独立于源 quality。
- `source_domain`：像元是否落入可靠定位的发布域；不等于真实扫描覆盖。
- `observation_coverage`：可信覆盖声明的状态与依据；未知保持未知。
- `provenance`：保留源历史并追加选择/投影/采样记录。
- `error`：非成功终态使用既有安全错误分类/阶段与空间附加原因；取消、超时、资源上限均可保留已有尝试，不创建成功帧或重定义 ProviderError。

`in_view/source_domain` 等像元平面只有成功数据存在时提供。每次尝试绑定同一 frame receipt、几何版本和采样记录；结果内不得混入不同候选的像元。候选上限截断是 selection 事实，可能伴随 selected 或 failed，不单独表示全球无覆盖。

不为先前不可定位 Pixel dBZ 生成坐标，不让局部中心污染源身份。报告可以显示用户主动配置的位置；运行日志不额外打印精确位置，公开 live 报告使用明确选择的测试点。

## 状态推进

```text
validate_request → load_spatial_targets → filter_access_geometry → bounded_probe
  → discover_latest → acquire → decode_or_georeference_image
  → verify_current_domain → project → render → selected
```

每个候选失败步骤保留来源错误与空间原因，可在剩余预算内继续；取消、整批超时或共享资源上限结束全流程。零候选为 no_candidate；全部尝试失败为 failed；因上限未穷尽时附 candidate_limit_reached。`outside`、`geometry_unverified`、`unsupported_crs`、`admission_unverified` 是位置流程的附加原因，不覆盖已有 DiscoveryStatus 的定义。全缺测也可以形成诚实的 selected 视图，其 data_state 为 missing；不能因此伪造 no_rain。
