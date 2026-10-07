# RDCAP CLI 与 SDK 合同

本合同定义实施后的行为。新增来源未实现；以下命令不是当前通过记录。复用[既有CLI](../../003-rust-core-performance/contracts/cli.md)与[SDK文档](../../../docs/python-sdk.md)的报告、配置、生命周期和错误边界。

## 来源与目录

```sh
radiust --json list sources
radiust --json list products rdcap
radiust --json list stations rdcap
```

默认list离线读取内置快照。新增rdcap/reflectivity，48唯一站点；station ID为TWRCHL、JPISHI、PHSUBI等。JSON保留已有字段并附可选metadata，显示country、原始catalog状态/冲突、快照时间、能力验证状态；未知坐标null，不能0。human输出区别“目录状态”与“实时发现状态”。不新增国家flag，使用完整站点身份选择国家。

Python `radiust.registry.get_source_info("rdcap")` 同样映射目录；StationInfo的lon/lat允许None、增加可选metadata，站点专用校验支持严格两字母国家前缀+站码。StationInfo以metadata.source_id=rdcap及所属SourceInfo复核限定命名空间；FrameRef/DiscoveryTarget及Query按source限定，不能漏掉Python DTO校验。SourceInfo、ProductInfo增加可选metadata以携带逐国能力和近期at/range能力。既有字段及有值坐标不变。

来源availability使用现有枚举，不新增blocked枚举；metadata逐国记录live=unverified/verified及证据。新增adapter有离线science合同不自动等于三国在线已验收。

## 发现与时间

```sh
radiust --json discover rdcap --station TWRCHL --latest
radiust --json discover rdcap --station JPISHI --latest --max-age 1800
radiust --json discover rdcap --station PHSUBI --at 2026-10-01T05:40:10Z
radiust --json discover rdcap --station TWRCHL --start 2026-10-01T05:00:00Z --end 2026-10-01T06:10:00Z
radiust --json discover rdcap --latest
radiust --json discover all --latest
```

固定2026-10-01时间仅演示语法；现场需用当前索引返回时刻。CLI未指定selector沿用latest；SDK Query仍要求恰好一种。at精确、range半开，不最近邻、不补时次。无站点展开全部去重站点；all维持既有latest-only/no station filter合同。上游新增站点经一次目录刷新进入当前目标；不把无资料/Inactive站删掉。

短码RCHL仅唯一时接受，返回规范身份TWRCHL。显式重复规范身份拒绝；未知站返回unknown_station，目录不可用时未知站返回catalog_unavailable。同key仅票据不同不算歧义，有稳定候选冲突为ambiguous_index。

完整report保留source/product/station实际UTC时刻、安全code/stage/retryable，公开JSON不含private locator。目录刷新时间计入发现总期限；刷新失败继续已知快照目标，未知目标单独失败。48站latest冻结回放sum(counts)=48；range totals按frame items，详见[data-model](../data-model.md)。

## 原始、科学下载与预览

```sh
radiust --json download rdcap --station TWRCHL --latest --raw-only --output ./data/rdcap-raw
radiust --json download rdcap --station JPISHI --latest --raw --format netcdf --output ./data/rdcap-nc
radiust --json download rdcap --station PHSUBI --latest --format geotiff --output ./data/rdcap-tif
radiust --json download rdcap --station TWRCHL --latest --format png --output ./data/rdcap-png
radiust --json download rdcap --station PHSUBI --latest --format zarr --output ./data/rdcap-zarr
radiust cat rdcap --station TWRCHL --latest --decoded --renderer text
```

raw-only保存原始响应/绑定/manifest，不科学解码。原始JSON/CSV不是图像，cat原始图像模式不隐式解释它；科学预览使用明确的--decoded，错误给出可操作说明。默认科学PNG使用验证15档palette，preview和writer共享规则。原始样本PNG含白色标记，因此默认科学PNG与网页图像不是逐像元完全相同，必须核对科学回波色阶和annotation透明。

多站可重复--station；未指定站点的download展开全部。单帧cat/fetch在多匹配时拒绝歧义，不任意取第一站。默认native网格；显式地理重网格与已有bbox/resolution/resampling合同一致，dBZ bilinear使用线性反射率。输出模板{station}对rdcap直接使用无斜杠站点ID（如TWRCHL）。

## SDK 与最小必要接口加法

已有 `Client/AsyncClient.discover/query/acquire/decode/fetch/fetch_many/iter_fetch/download/write`共用Rust Engine；不新增RDCAP专用获取函数。下面新增两项通用薄接口，底层能力已有Rust实现，但Python facade尚未公开：

| 接口 | 返回/行为 |
|---|---|
| Client.discover_report(query) / await AsyncClient.discover_report(query) | 绑定的完整DiscoveryReport，提供to_json()/frame(index)，成功、无数据及失败都保留 |
| Client.replay_raw_manifest(path) / await AsyncClient.replay_raw_manifest(path) | 调用Engine::replay_raw_manifest，经摘要/路径验证后返回独立内存RadarField，无网络 |

现有discover继续返回refs及既有异常语义，避免静默改变其返回类型。调用方需要逐站终态时用新增discover_report。批量query在on_error=collect/continue下须保留完整discovery报告并继续成功refs，不能因单站发现失败丢弃整批；既有batch结果增加可选discovery_report及discovery_counts，frame统计含义不变。stop/raise返回带上述报告和已完成项的partial_result。成功/失败/no_data目标均可从这份报告核对；不把未发现的目标伪造为FrameRef。

这是当前SDK `_discover_frames`遇首个失败即抛出而造成的真实缺口，需要明确实施和合同验证；CLI已有完整发现report路径可复用。新增方法与report属性适用于共用Engine，不创建第二套提供者pipeline。绑定若已内部公开Session.discover_report，直接复用，不重复定义报告DTO。

```python
import radiust

query = radiust.Query("rdcap", stations=("TWRCHL",), latest=True)
config = {"runtime": {"allow_network": True}}
with radiust.Client(config=config) as client:
    ref = client.discover(query)[0]
    with client.acquire(ref) as raw:
        field = client.decode(raw)
    client.write(field, ref=ref, output="./data/rdcap", format="netcdf")
```

异步使用AsyncClient并await对应操作，acquire用async with。field可在raw/Client关闭后使用；raw上下文关闭后不可访问。科学互操作显式to_xarray，保留f32/u16、时间、CRS、坐标、provenance；基础CLI/SDK不依赖NumPy编码。

## 报告、错误与兼容

现有schema_version、stdout单JSON、stderr安全信息、退出码和部分失败政策不变。必要的source错误码是ErrorCode枚举加法；SDK ErrorContext增加可选code以传递同一类别，不能把故障都压成transport或靠字符串识别。错误阶段沿用validate/discover/acquire/decode/regrid/stage/commit，具体类别见[provider-protocol](provider-protocol.md)。

冻结旧目录仍24source/26target，当前加rdcap后25/74；新动态目录可超过74。回归保留tw默认grid及其科学行为、tw-http、ph的原始与错误场景，不能更新总数断言来掩盖旧来源丢失。科学/regrid/palette gate仅新增rdcap，未验证来源不因此升级。
