# Python SDK

## 查询和生命周期

`Query` 的 `latest`、`at`、`start/end` 三种时间选择必须恰好使用一种。时间必须带时区，内部统一为 UTC；`max_age` 只用于 `latest`。`fetch` 对零匹配、多匹配和不支持的历史查询分别抛出稳定错误。

```python
from datetime import datetime, timezone
import radiust

query = radiust.Query(
    "my",
    product="composite",
    stations=("peninsular",),
    at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc),
)

with radiust.Client() as client:
    refs = client.discover(query)
    with client.acquire(refs[0]) as raw:
        field = client.decode(raw)
```

`Client` 复用一个私有事件循环；不能从另一个线程或已有运行中的事件循环调用同步入口。异步代码使用 `async with radiust.AsyncClient()`。`acquire` 返回上下文管理器，离开上下文后 `RawFrame` 不再可访问；解码结果已经加载到独立内存。

## 获取、批量和输出

```python
with radiust.Client() as client:
    field = client.fetch(query)
    report = client.download(
        query,
        output="./data",
        format="netcdf",
        raw=True,
        grid="native",
    )
```

批量入口保持输入顺序并为每帧生成结果；`iter_fetch` 按完成顺序产生结果，预取数量有界。`on_error="collect"` 收集逐帧错误，`on_error="raise"` 抛出带 `partial_result` 的 `BatchError`。重复的逻辑帧输入会在获取前被拒绝。

允许的下载处理参数是 `variable`、`grid`、`bbox`、`resolution`、`resampling` 和 `encoder_options`。未知关键字会在网络获取前报错。`fetch` 不写正式输出；`raw=True` 保存原始资料，`raw_only=True` 只保存原始资料和 `raw-manifest.json`，不能从已解码 Field 反推原始资料。

## 科学对象

`RadarField` 表示一个主要变量，`RadarDataset` 表示共享网格和时间的多个变量。两者都携带 `uint16` quality、Grid 和 provenance，并提供 `validate()`、`to_dataset()`、`regrid()` 与 `to_geographic()`。默认保留 native grid；地理转换必须显式提供 `bbox` 和 `resolution`。

质量位为：bit 0 `missing`、bit 1 `outside_coverage`、bit 2 `unknown_color`、bit 3 `recovered`、bit 4 `interpolated`、bit 5 `below_detection`。零只表示没有已知质量异常，不能把缺测或透明像元静默变成无雨。

## 来源扩展

来源实现 `Source.info`、异步 `discover`/`download` 和同步 `decode`。decoder 不联网、不写正式输出、不自行重试；所有获取通过共享 `SourceContext` 的限额、transport 和临时资源。第三方可通过 `radiust.sources` entry point 注册，但重复 id 会失败。

来源目录中的每个内置来源都有独立 adapter。MY adapter 现在使用 provider image endpoint 的 HEAD 元数据并保留原始响应字节；valid time 仍使用旧代码的 `Last-Modified - 9 minutes` 推断，明确标为未验证。仓库内的 MY PNG 只由离线测试显式注入，registry/SDK/CLI 不会把 fixture 冒充成 provider 数据。MY 的 palette、原生几何与图像再利用许可仍未验证，因此科学解码保持拒绝；联网需在配置中显式启用 `runtime.allow_network`。
