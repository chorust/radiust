# CLI / SDK Contract: 位置视图

**Status**: 接口已实现；安装后禁网 smoke、固定输入三入口 parity 及 RainViewer 裸 `cat` 当前链路均通过。RainViewer 默认准入仅适用于个人用途 `local_view`，CLI 人类报告显示来源署名链接；观测覆盖仍按未知处理。行为和限制以当前实现及验证记录为准。

## 配置

```yaml
location:
  longitude: 121.60
  latitude: 24.00
local_view:
  radius_km: 50
  resolution_m: 1000
runtime:
  allow_network: true
```

示例点是演示配置，不宣称此时有通过默认准入的来源。联网仍需用户显式启用。新结构成对校验经纬度；只有显式入口依赖位置时才要求已配置 location。

沿用当前配置层：默认值 < YAML < 环境 < 显式参数。环境变量为 `RADIUST_LOCATION__LONGITUDE`、`RADIUST_LOCATION__LATITUDE`、`RADIUST_LOCAL_VIEW__RADIUS_KM`、`RADIUST_LOCAL_VIEW__RESOLUTION_M`。严格 key 校验、默认值和 origins 已扩展；旧配置无需新增 location。

## 路由与参数

| 请求 | 拟定行为 |
| --- | --- |
| `radiust cat` | 配置位置 → 自动 latest 候选 → 用户居中图；当前没有默认 admission，通过结果为 no_candidate |
| `radiust cat --radius-km 50 --resolution-m 1000` | 覆盖局部参数，触发局部流程 |
| `radiust cat --local --dbz` | 尝试受支持 dBZ；数值失败不能回退原图 |
| `radiust cat SOURCE` | 保留现有 source raw 默认，不隐式投影 |
| `radiust cat SOURCE --local --station ID` | 约束来源/站点，输出位置视图；不得切到别的来源 |
| `radiust cat --file PATH` | 保留原有文件行为 |
| `radiust cat --file PATH --local` | 首版拒绝；局部入口仅处理来源观测，文件入口保留原有行为 |

已加入 `--local`、`--radius-km`、`--resolution-m`、`--resampling nearest|bilinear`。范围/间距/resampling 参数本身触发局部模式。`--width/--height` 仍是 renderer 尺寸，不改变科学网格间距。所有局部分支拒绝 `--gray`/`--legacy-display`。local decoded 仅接受受支持的 native science，保留其物理量/单位，不能自动改标 dbz。

位置分支首版只接受 latest。显式 source/local 不新增 at/range/base-time 承诺，携带这些时间参数时报清楚的不支持；原非 local 入口的 at 行为保持现状。station/product 必须附 source；不在裸 cat 中跨来源猜测短站码。文件与任何触发 local 的参数组合都明确拒绝。

auto 对选定候选优先用已验证 native 数值路径；只有该产品没有 native 路径时才用可靠定位 raw。native 校验失败记为该候选失败并按相同显式限制尝试下一候选，不偷偷换成原图。`auto+bilinear` 只选连续 native 数值；raw/类别不适用。裸 cat 的默认 nearest 保留源取样值和质量。

## 报告

Envelope v1 和现有退出码继续使用；局部报告追加 `local_view_schema_version: 1` 及 `status`。selected 时包含中心、投影、半径、实际网格间距、shape、mode_info、选择理由、覆盖证据、receipt 绑定的 processing identity 和尝试结果；失败/取消/超时保留已知参数与 selection/error，selected/data 为空，不生成成功帧。JSON 不内嵌整幅数值数组或私有 locator；真正数据由 SDK 对象提供。明确显式请求的 raw/dbz 与实际数据模式。

新增空间原因作为附加字段：`location_missing`、`geometry_unverified`、`unsupported_crs`、`outside_domain`、`admission_unverified`、`candidate_limit_reached`。它们不重定义 DiscoveryStatus。配置/参数错误按现有预检码 2；取消 130、禁网/获取/无数据等复用现有对应分类，不为 006 发明另一套通用退出码。

回退人类提示说明旧/新来源和原因；成功后更新图例、有效时间和覆盖信息。超限不能用自动降低分辨率或切换原图伪装成成功。

默认尝试上限 8 个目标；candidate_limit_reached 可伴随成功或失败，不能被翻译成全球无覆盖。一次 `runtime.discovery_deadline` 贯穿全部阶段；取消、超时和共享资源上限停止整批，不继续尝试备用来源。源有效时间不因投影或 cache 命中更新。

## Core 与薄 SDK

单一入口 `Engine.local_view(LocalViewRequest) -> Result<LocalViewResult, EngineError>` 由内部空间候选查询、既有 discovery/fetch/decode 和投影处理组成。预检错误使用 EngineError；合法请求的 selected/no_candidate/failed/interrupted/timed_out 及过程错误按 [data-model](../data-model.md)返回。CLI 从现有错误分类映射退出码，SDK 可读取相同终态与尝试信息。

Python 同步 `Client.local_view(...)` 与异步 `AsyncClient.local_view(...)` 委托同一 PyO3 入口；request/result 中的 x/y 为 m，显式 xarray 转换仍可选。关键字为 `center=None, radius_km=None, resolution_m=None, source=None, product=None, station=None, mode="auto", resampling="nearest"`；None 在 Core 中从同一配置补全，显式 center 必须是完整 longitude/latitude 对。CLI/SDK 不各自实现参数默认、空间筛选或采样。SDK 不让已有 `fetch()` 的空 source 突然自动联网或改变含义。

绑定在 python_types.rs 注册 typed 请求/结果与 PyEngine awaitable；Python 保留既有 client 生命周期、operation gate、进度和取消机制。显式取消 token 的终态为 interrupted；异步 task.cancel 继续传播 CancelledError，同时取消底层 operation，不把取消转成成功对象。

正式 projected download/write、后台动画和 GUI 绑定不在本契约范围。已有结果类型的科学值、quality、raw receipt、身份和错误继续遵守原合同。
