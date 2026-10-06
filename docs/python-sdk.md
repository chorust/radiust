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

批量入口保持输入顺序并为每帧生成结果；`iter_fetch` 按完成顺序产生结果，预取数量有界。对包含 `Query` 的 `fetch_many()`，`on_error="collect"` 或 `"continue"` 会保留完整发现终态并继续获取成功 refs；返回的 `BatchResult.discovery_report` 保存逐目标报告，`discovery_counts` 是其状态计数。`no_data` 也会出现在发现报告中，但不会生成虚构的 frame。发现状态计数与 frame 结果的 `BatchResult.counts` 分开统计。`on_error="raise"` 或 `"stop"` 遇到发现失败时会抛出带 `partial_result` 的 `BatchError`，其中仍含完整发现报告和可用的成功 frame 句柄；发现阶段失败时不会开始获取这些成功 frame。输入只有 `FrameRef` 时没有发现报告，这两个属性为 `None`。重复的逻辑帧输入会在获取前被拒绝。

`download()` 经 Rust Engine 写入本地目录或对象存储，支持 raw-only、PNG、NetCDF、GeoTIFF 和 Zarr v2；科学格式只对已实现相应 decoder 的来源/产品开放，RDCAP 科学值仍须结合本节的逐国在线验收状态使用。`raw=True` 可将验证过的原始 artifact 与解码成果放进同一次正式提交。解码下载支持 `variable`、`grid`、`bbox`、`resolution` 和 `resampling` 参数；RainViewer EPSG:4326 geographic 下载有既有验证，RDCAP 解码场为 EPSG:4326 原生网格，但三国在线发现、获取和科学读回仍未验收。绑定结果的 `RadarField.regrid()`/`RadarDataset.regrid()` 与 Rust `Engine::regrid()` 支持 EPSG:4326↔EPSG:3857 的 Web Mercator 坐标变换；其他 datum/projection 转换（包括 TW EPSG:3821 到 EPSG:4326）仍明确失败。对象存储目标使用 `s3://bucket/prefix` 或 `oss://bucket/prefix`，并须在配置中显式启用 `runtime.allow_network`，通过 `storage` 配置提供 endpoint/region 和凭据；URI 本身不能包含凭据。解码格式支持安全的 `output_template` 字段 `{source}`、`{product}`、`{station}`、`{valid_time}`、`{base_time}`、`{date}`、`{hour}`、`{variant_id}` 与 `{ext}`；本地模板必须生成相对路径，不能越过输出根目录。RDCAP 的 `{station}`（例如 `TWN/RCHL`）会编码为单个路径分量 `TWN%2FRCHL`。模板仅用于解码输出；raw-only 与模板组合会被拒绝。非空 `encoder_options` 目前不支持。远端 generation/pointer 提交流水线已有内存故障合同，真实 AWS S3/阿里云 OSS 尚未验收；尚不支持的来源能力也会返回有界错误。

`Client.write()` 和 `AsyncClient.write()` 接收 Rust `RadarField`、`RadarDataset` 或有 receipt 的 `RasterResult`，通过 Rust writer 发布到本地 manifest-last store。未绑定的旧 field/dataset 必须提供来源 `FrameRef`；带 Source/Local/NumericFile 身份的新 `RasterResult` 可用自己的读取 receipt，无需合成 ref。字段时间与显式 ref 必须相同，多变量 Dataset 必须用 `variable=` 选择一个变量。重复写入完整且身份相同的成果会返回 `skipped`；RasterResult 当前不接受远端 URI 或地理重网格选项。

```python
with radiust.Client() as client:
    ref = client.discover(query)[0]
    field = client.fetch(ref)
    report = client.write(field, ref=ref, output="./data", format="netcdf")
```

## Gray 与 dBZ

灰度解码和 scientific dBZ 都由 Rust Core 执行。`gray-dbz-v1` 是显式声明，visible code 必须是灰度整数 0–224，公式为 `gray * 5/16`；alpha 0 表示 missing，非零 alpha 不乘入数值，opaque black 是有效 0。local 结果不含推断时间或 CRS。同步、异步文件入口分别为 `decode_gray_file()` 和 `await decode_gray_file()`：

```python
import asyncio
import numpy as np
import radiust

image = "./gray.png"
with radiust.Client() as client:
    local = client.decode_gray_file(image)
    written = client.write(local, output="./data", format="netcdf")
    numeric = client.read_dbz(written["output_uri"])
    field = radiust.to_xarray(numeric)

async def load_again():
    async with radiust.AsyncClient() as client:
        return await client.decode_gray_file(image)

async_result = asyncio.run(load_again())
np.testing.assert_allclose(
    radiust.to_xarray(local).values,
    radiust.to_xarray(async_result).values,
    rtol=0,
    atol=0,
    equal_nan=True,
)
```

`fetch(ref, mode="gray")` returns the source gray decision; `fetch(ref, mode="dbz")` returns a Rust-owned `RasterResult` when a direct native decoder or an evidence-passed source-gray rule succeeds. `decode_dbz(raw)` performs the same explicit source-mode decode on an acquired `RawFrame`. `fetch_many(..., mode="dbz")`, `iter_fetch(..., mode="dbz")`, and their async counterparts preserve per-frame status, error and additive `mode_info`; a failed item is not reported as dBZ. `download(..., mode="dbz")` writes a numerical result and can attach the same verified acquisition with `raw=True`.

For retained data, `read_dbz(path, variable="reflectivity", valid_time=...)` validates the actual variable and dBZ units. It returns a new file-bound `RasterResult`; `write(result, ...)` can commit it without a `FrameRef`, and the values are never multiplied by 5/16 a second time. Embedded processing becomes upstream provenance, while current identity is based on file content and selection. Supplying a `FrameRef` for a local numeric result is rejected. A legacy unbound `RadarField` still needs its source `FrameRef` when written.

Pixel results write to NetCDF, Zarr, or display-only PNG; a Pixel result with a complete trusted geometry mapping can also write GeoTIFF with separate value, quality, origin-quality, adjustment, alpha, and provenance components. The reader checks that the components share one transform and CRS, and includes each declared component in the numeric-file receipt. A Pixel GeoTIFF may keep its time unknown. Without a complete trusted mapping, GeoTIFF and geographic requests fail closed; no CRS, time, or extent is inferred.

`replay_raw_manifest(path, mode="dbz")` validates and replays only retained bytes offline; omitting `mode` preserves the historical generic science return type. Manifest SHA or frame-binding failures are integrity errors and do not trigger network retrieval. Use `to_xarray(RasterResult)` for pixel dBZ to explicitly allocate NumPy arrays; a native result exposes `native_field` for the existing `RadarField` conversion. See [gray/dBZ semantics and qualification](gray-dbz.md) for clipping, blocked source paths, provenance and the distinction between encoding validation and physical/live validation.

`GrayDbzDecoder` is the strict canonical decoder. `LegacyGrayDbzDecoder` preserves the former permissive/historical API as a compatibility profile; it does not make an image eligible for canonical `mode="dbz"`. These are thin Rust Core facades, not Python image-processing implementations. Optional xarray/NumPy arrays are created only when `to_xarray()` is called; the bound pixel result retains `float32` values, `uint16` quality, `origin_quality`, `encoding_adjustment`, and original alpha dtype/bit depth.

For batches, `fetch_many()` keeps input order and returns each frame's status; `iter_fetch()` yields as work completes with bounded prefetch. `on_error="collect"` retains successful items and per-item errors in the partial result. With `on_error="raise"`, `BatchError.partial_result` exposes completed work. `Client.cancel()` / `AsyncClient.cancel()` request cancellation of active Core work; a cancelled write raises `OperationCancelled` (`code="cancelled"`) and must not publish a success manifest. These names are lazy exports from `radiust`, so importing the CLI or basic package does not eagerly load scientific Python dependencies. The wheel includes only the decoder facade and its canonical/compatibility resources; it does not package the excluded Python processing pipeline. `to_xarray()` is the explicit copying boundary from Rust-owned buffers to NumPy/xarray arrays.

## 科学对象

顶层 `radiust.RadarField` 与 `radiust.RadarDataset` 是 PyO3 绑定类型，字段数组由 Rust 持有。需要 NumPy/xarray 时显式调用 `radiust.to_xarray(value)`；它保留 `float32` 值、`uint16` quality、坐标、CRS、UTC 时间和 provenance。基础安装导入这些绑定类型不需要 NumPy/xarray；`to_xarray()` 需安装 `radiust[science]`。`radiust[zarr]` 安装 Python Zarr v2 互操作依赖。

质量位为：bit 0 `missing`、bit 1 `outside_coverage`、bit 2 `unknown_color`、bit 3 `recovered`、bit 4 `interpolated`、bit 5 `below_detection`、bit 6 `source_annotation`。bit 0–5 的既有含义保持不变；RDCAP 原始值 9999 按当前跨站样本推断为图面标记时，解码值为 NaN、quality 为 65（bit 0 与 bit 6 同时设置）。NetCDF、GeoTIFF provenance 和 Zarr 写出新增 mask 64/meaning `source_annotation`，读取器仍接受旧六位布局。零只表示没有已知质量异常，不能把缺测或透明像元静默变成无雨。`to_xarray()` 保留 `uint16` quality 及其 masks/meanings。

## RDCAP 单站 API 与当前边界

`rdcap/reflectivity` 提供台湾、日本和菲律宾的完整站点 ID（如 `TWN/RCHL`、`JPN/ISHI`、`PHL/SUBI`），内置离线目录快照有 48 个去重站点。快照中的逐国 discovery、raw acquisition、science、readback 能力仍标为 `unverified`；离线目录可用于查站，不能代表实时索引或在线服务可用。当前支持近期 `latest`、精确 `at` 和时间范围查询，不承诺历史归档。联网操作必须显式设置 `runtime.allow_network: true`。

`discover_report()` 返回逐目标终态的 `DiscoveryReport`，含 `items`、`counts`、安全错误字段和 `to_json()`。JSON 不含私有 locator/ticket；成功项只有在原进程内的报告上才能用 `frame(index)` 取得带私有 locator 的 `FrameRef`。从 JSON 重建的报告不携带该进程内句柄。错误报告保留安全 `code`、`stage`、`retryable` 字段；SDK 的 `RadiustError.context` 提供对应错误上下文。现有 `discover()` 仍返回 refs。对单站可这样使用：

```python
import radiust

query = radiust.Query(
    "rdcap", product="reflectivity", stations=("TWN/RCHL",), latest=True
)
with radiust.Client(config={"runtime": {"allow_network": True}}) as client:
    report = client.discover_report(query)
    print(report.to_json())  # 安全 JSON，不含 ticket/locator
    if report.items and report.items[0].status == "success":
        ref = report.frame(0)
        with client.acquire(ref) as raw:
            field = client.decode(raw)
```

`AsyncClient` 提供同名 `await discover_report()` 和 `await replay_raw_manifest()`，且 `await fetch_many()` 具有相同的 `BatchResult.discovery_report` / `discovery_counts` 行为。`Client.replay_raw_manifest(path)` / `AsyncClient.replay_raw_manifest(path)` 通过 Rust Engine 校验 raw manifest、binding、路径和摘要后离线解码为独立内存 field，不需要网络或 ticket；应传入一次成功 raw 下载实际生成的 manifest 路径。篡改内容、路径越界或身份不一致会失败。单次 `discover_report()` 与 query 批量报告均保留成功、无数据和错误目标。

RDCAP 默认科学 palette 为 `rdcap-reflectivity-v1`：15 个下界包含档，阈值 5、10、…、75 dBZ，最后一档包含所有 ≥75 dBZ 值；低于 5 dBZ 的有限科学值仍保留在 field 中，但 PNG 显示透明。缺测与 annotation 也透明；PNG sidecar 记录 palette、annotation rule、decoder 和几何身份。科学 preview 与 PNG writer 使用同一默认渲染规则。此 palette 来自当前研究样本对 provider legend 的复现，不能作为三国在线服务验收证据；PNG 是显示结果，需保留连续数值时使用 NetCDF、GeoTIFF 或 Zarr。

`Client.write()` / `AsyncClient.write()` 可将解码后的 RDCAP field 写为 PNG、NetCDF、GeoTIFF 或 Zarr v2；必须传入产生该 field 的同一 `ref`。内存 field 的 `write()` 只支持本地输出和 native grid，不接受 raw 附加、对象存储或重网格参数。要保存实际响应字节并生成可重放 manifest，使用 `download(..., raw=True)` 或 `download(..., raw_only=True)`，然后将已提交 manifest 交给 `replay_raw_manifest()`。三国标准入口 live raw、science 和独立读回仍未验收；见 [RDCAP 验证指南](../specs/004-rdcap-single-station/quickstart.md) 中 T028/T053 状态。

原生 CLI 的 `radiust replay <raw-manifest.json>` 可禁网校验并重放本地 manifest，默认输出 PNG、NetCDF、GeoTIFF 和 Zarr v2；用 `--format` 指定逗号分隔的子集，重复提交会标为 `skipped`。

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
