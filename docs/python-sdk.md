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

`radiust.config.load_config(overrides, path, environ)` 由 Rust 完成 YAML/映射解析、分层、校验和路径检查，优先级为默认值、`~/.config/radiust/config.yaml`、当前工作目录的 `config.yaml`、环境变量、显式 `overrides`。自动文件不存在时跳过；传入 `path` 则用该文件替代当前目录的配置层，文件必须存在。文件按字段合并，未覆盖的全局字段保留。`EffectiveConfig` 保留 `values`、`origins`、`output_root`、`cache_dir` 与 `redacted()`；脱敏结果包含 `_origins`，全局文件来源标为 `user_file`、项目或 `path` 文件标为 `file`，凭据显示为 `<configured>`。基础运行时不需要 PyYAML。

`discover`、`fetch`、`fetch_many` 和 `download` 接受同步 `progress(stage, completed, total)` 回调。进度由 Engine 的安全生命周期事件驱动：`discover_targets` 表示 Rust 正在处理发现目标，`discover` 表示返回帧数，`acquire`/`decode` 表示单帧获取和解码，`resolve`/`fetch` 表示批量解析与获取，`download` 表示下载报告进度。事件不包含 locator、凭据或错误详情，进度为尽力通知；同一 Client 上带回调的操作会串行隔离事件流，未带回调的操作仍可并发。异步客户端的回调在其事件循环线程运行。

## 获取、批量和输出

```python
with radiust.Client() as client:
    field = client.fetch(query)
    report = client.download(
        query,
        output="./data",
        format="netcdf",
    )
```

`fetch()` 返回 Rust 持有的 `_core.RadarField` 或 `_core.RadarDataset`。成功结果独立于 Client 生命周期；`fetch()` 只获取和解码，不发布正式输出。`acquire()` 返回的 `RawFrame` 仅在上下文内有效。同步 `Client` 复用一个事件循环和 Rust Engine；异步代码使用 `AsyncClient`。

批量入口保持输入顺序并为每帧生成结果；`iter_fetch` 按完成顺序产生结果，预取数量有界。`on_error="collect"` 收集逐帧错误，`on_error="raise"` 或 `"stop"` 抛出带 `partial_result` 的 `BatchError`。重复的逻辑帧输入会在获取前被拒绝。

`download()` 经 Rust Engine 写入本地目录或对象存储，支持 raw-only、PNG、NetCDF、GeoTIFF 和 Zarr v2；科学格式只对已验收的来源/产品开放。`raw=True` 可将验证过的原始 artifact 与解码成果放进同一次正式提交。解码下载支持 `variable`、`grid`、`bbox`、`resolution` 和 `resampling` 参数；当前 geographic 下载重网格只对已验证的 RainViewer EPSG:4326 数据开放。绑定结果的 `RadarField.regrid()`/`RadarDataset.regrid()` 与 Rust `Engine::regrid()` 另支持 EPSG:4326↔EPSG:3857 的 Web Mercator 坐标变换；其他 datum/projection 转换（包括 TW EPSG:3821 到 EPSG:4326）仍明确失败，未验收来源也不会升级为科学可用。对象存储目标使用 `s3://bucket/prefix` 或 `oss://bucket/prefix`，并须在配置中显式启用 `runtime.allow_network`，通过 `storage` 配置提供 endpoint/region 和凭据；URI 本身不能包含凭据。解码格式支持安全的 `output_template` 字段 `{source}`、`{product}`、`{station}`、`{valid_time}`、`{base_time}`、`{date}`、`{hour}`、`{variant_id}` 与 `{ext}`；本地模板必须生成相对路径，不能越过输出根目录。非空 `encoder_options` 目前不支持。远端 generation/pointer 提交流水线已有内存故障合同，真实 AWS S3/阿里云 OSS 尚未验收；尚不支持的来源能力也会返回有界错误。

`Client.write()` 和 `AsyncClient.write()` 接收 Rust `RadarField` 或 `RadarDataset`，将已解码对象通过 Rust PNG、NetCDF4、GeoTIFF 或 Zarr v2 writer 发布到本地 manifest-last store。调用方必须传入产生该对象的 `FrameRef`；字段时间必须与帧时间相同。多变量 Dataset 必须用 `variable=` 选择一个变量。重复写入完整且身份相同的成果会返回 `skipped`；目前内存对象写入不接受 `raw=True`、远端 URI 或地理重网格选项。

```python
with radiust.Client() as client:
    ref = client.discover(query)[0]
    field = client.fetch(ref)
    report = client.write(field, ref=ref, output="./data", format="netcdf")
```

## 科学对象

顶层 `radiust.RadarField` 与 `radiust.RadarDataset` 是 PyO3 绑定类型，字段数组由 Rust 持有。需要 NumPy/xarray 时显式调用 `radiust.to_xarray(value)`；它保留 `float32` 值、`uint16` quality、坐标、CRS、UTC 时间和 provenance。基础安装导入这些绑定类型不需要 NumPy/xarray；`to_xarray()` 需安装 `radiust[science]`。`radiust[zarr]` 安装 Python Zarr v2 互操作依赖。

质量位为：bit 0 `missing`、bit 1 `outside_coverage`、bit 2 `unknown_color`、bit 3 `recovered`、bit 4 `interpolated`、bit 5 `below_detection`。零只表示没有已知质量异常，不能把缺测或透明像元静默变成无雨。

## 来源扩展

来源由 Rust 编译期 registry 管理；不再自动加载 `radiust.sources` Python entry point。第三方来源需实现 Rust adapter 并重新构建 wheel/应用。来源发现和获取共享 Engine 的网络限额与临时资源；科学解码只对留有验证证据的来源开放。

来源目录中的每个内置来源都有独立 adapter。MY adapter 现在使用 provider image endpoint 的 HEAD 元数据并保留原始响应字节；valid time 仍使用旧代码的 `Last-Modified - 9 minutes` 推断，明确标为未验证。仓库内的 MY PNG 只由离线测试显式注入，registry/SDK/CLI 不会把 fixture 冒充成 provider 数据。MY 的 palette、原生几何与图像再利用许可仍未验证，因此科学解码保持拒绝；联网需在配置中显式启用 `runtime.allow_network`。

## CLI 与兼容性

Python `radiust` console command 和 `python -m radiust` 转发到原生 Rust CLI。Python API 和命令行已共用 Rust Engine；原有 Click/source pipeline 不作为隐式回退路径。

这是有意的 API 变化：依赖 `fetch()` 直接返回 `xarray.DataArray/Dataset` 的调用方需改用显式转换：

```python
import radiust

async def load_array(query):
    async with radiust.AsyncClient() as client:
        field = await client.fetch(query)
    return radiust.to_xarray(field)
```

Python 只在显式互操作边界创建 NumPy/xarray 对象，并保留可选 Zarr 读写支持。`radiust.sources` entry-point 插件不会自动加载；内置来源在 Rust registry 中，第三方来源需实现 Rust Source adapter、固定编译版本并重新构建 wheel。下载的当前格式/来源边界会以明确错误返回；安装包没有 Python pipeline 回退。

安装 wheel 不再携带旧 Python 来源、科学解码、图像显示、传输、存储及非 Zarr 输出实现；可选 `radiust.outputs.zarr.write_zarr()` 接受绑定的 `RadarField`/`RadarDataset`，显式转为 xarray 后写出 Zarr v2。旧 `render_field()`/`show()` Python API 已移除：预览使用原生 `radiust cat`，PNG 成果用 `Client.write(..., format="png")`。批量 SDK 仍由 Rust Engine 完成获取和解码，`iter_fetch` 保留为 SDK 迭代器 facade。
