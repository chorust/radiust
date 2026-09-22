# Python SDK Contract v1

状态：目标公共契约。数据类型见 [data-model.md](../data-model.md)；实现路径由 [plan.md](../plan.md) 规定。公开入口为 Python SDK 与 CLI，不新增 HTTP endpoints。

## 查询和获取

```text
Query(source: str, *, product: str | None = None,
      stations: tuple[str, ...] = (), latest: bool = False,
      at: datetime | None = None, start: datetime | None = None,
      end: datetime | None = None, base_time: datetime | None = None,
      max_age: timedelta | None = None)

sources() -> tuple[SourceInfo, ...]
get_source(source_id: str) -> Source
fetch(query: Query, *, config=None) -> RadarField | RadarDataset
async afetch(query: Query, *, config=None) -> RadarField | RadarDataset
fetch_many(query_or_refs, *, config=None, on_error="collect") -> BatchResult
async afetch_many(query_or_refs, *, config=None, on_error="collect") -> BatchResult
iter_fetch(query_or_refs, *, config=None, on_error="collect") -> Iterator[FrameResult]
aiter_fetch(query_or_refs, *, config=None, on_error="collect") -> AsyncIterator[FrameResult]
download(query_or_refs, *, output="./data", format="netcdf",
         raw=False, raw_only=False, overwrite=False,
         output_template=None, on_error="collect", config=None, **processing) -> DownloadReport
async adownload(query_or_refs, **same_options) -> DownloadReport
```

以上为签名契约概要，`**processing` 仅允许明确的 variable/grid/bbox/resolution/resampling/encoder_options，未知项报参数错误，不是任意扩展口。

- `sources/get_source` 不联网、不实例化重型 decoder；同名注册抛 DuplicateSourceError。来源 availability 是静态状态，实时结果来自显式 doctor。
- latest/at/start+end 恰选一种。fetch 恰一帧，零帧 NoDataError、多帧 AmbiguousFrameError；不支持查询抛 UnsupportedQueryError，过期抛 StaleFrameError。
- 顶层便捷函数创建短生命周期 Client 并完成关闭。循环任务用显式 Client 复用连接；同一 Client 不能跨事件循环使用。
- fetch 返回已独立加载的科学数据，不写正式 output。允许缓存。raw 保存仅在 download/acquire 生命周期中完成，不能从已有 field 反推原始資料。
- on_error=collect 保留每帧失败；raise 在首个失败后取消未完成工作并抛 BatchError(partial_result)，已成功结果不丢失。query 参数错误在任何获取前失败。
- 输入 refs 必须有序且无重复 logical_id+已知 revision；同一逻辑帧不同已知修订可作为不同输入。默认 BatchResult 保输入顺序，iter 按完成顺序；未完成输入保留 cancelled/not_started 状态和原因。
- 流式预取上限默认等于 frame_concurrency；用户必须关闭提前停止的同步/异步生成器，关闭会取消未完成工作并回收资源；Client 关闭提供兜底收尾。

## Client 生命周期与阶段入口

| Client | AsyncClient | 结果/语义 |
| --- | --- | --- |
| `with Client(config=...)` | `async with AsyncClient(config=...)` | 构建配置、上下文与资源所有权 |
| `discover(query)` | `await discover(query)` | `list[FrameRef]`，固定选择与稳定排序 |
| `with acquire(ref) as raw` | `async with acquire(ref) as raw` | acquire 本身返回上下文管理器，**不是**先 await 一个 RawFrame |
| `decode(raw)` | `await decode(raw)` | 验证后的 Field/Dataset；同步科学工作进入有界 worker |
| `fetch/fetch_many/iter_fetch` | `await fetch/fetch_many`、`async for iter_fetch` | 与顶层对应入口相同，复用上下文 |
| `write(field, output=..., format=..., ...)` | `await write(...)` | 编码和正式提交，返回 DownloadReport，不接受 raw/raw_only |
| `download(query_or_refs, ...)` | `await download(...)` | 完整 pipeline，与 CLI 同路径 |
| `close()` | `await aclose()` | 幂等，停止新请求、取消并收尾，释放租约和连接 |

同步 Client 内部复用其私有 asyncio loop，不在每个方法新建 loop；当前线程已运行 event loop 时立即报 AsyncContextError 并提示 async 入口，不嵌套执行。跨线程调用同一 Client 不支持。async 路径保留 CancelledError 的取消语义，不吞掉取消当普通成功。

## 科学对象与显示

```text
field.validate() -> None
field.to_dataset() -> xarray.Dataset
field.regrid(target: GridSpec, *, method="nearest") -> RadarField
field.to_geographic(*, bbox, resolution, method="nearest") -> RadarField
dataset.select(variable: str) -> RadarField
render(field_or_dataset, *, variable=None, palette=None,
       vmin=None, vmax=None, grid=None) -> RenderedImage
show(field_or_dataset, *, renderer="auto", width=None, height=None,
     **render_options) -> None
```

- `.data` 为 xarray 对象，wrapper 不继承 xarray；quality、grid、provenance 独立且经 validate 检查。
- 变换返回新对象且记录处理历史，不就地改变原对象。连续默认 float32、缺测 NaN；类别和质量不被自动转为浮点或把 0 当缺测。
- render 是无终端副作用的纯展示计算，可用于测试/PNG；show 明确写终端。多变量必须选择，不默取第一个。show 遵守 [CLI cat](cli.md) 的终端规则。

## 错误族

| code / 类型 | stage | retryable 默认 |
| --- | --- | --- |
| config_error / ConfigError、missing_dependency / MissingDependencyError、unsupported_query / UnsupportedQueryError、async_context / AsyncContextError | validate | false |
| no_data / NoDataError、stale_frame / StaleFrameError、ambiguous_frame / AmbiguousFrameError | discover | false |
| authentication / AuthenticationError | acquire | false |
| transport / TransportError | acquire | 按底层暂时性分类 |
| integrity / IntegrityError、resource_limit / ResourceLimitError | acquire/validate | false，损坏缓存允许驱逐后一次重新获取 |
| unknown_color / UnknownColorError、decode / DecodeError | decode | false |
| grid / GridError、georeferencing / GeoreferencingError | regrid/render | false |
| output_conflict / OutputConflict、output_locked / OutputLockedError | commit | false |
| storage / StorageError | stage/commit | 仅重试幂等且确定可重试的操作 |
| duplicate_source / DuplicateSourceError | registry | false |

所有领域错误继承 RadiustError。批量停止抛 BatchError，携带已脱敏的逐帧部分结果。CancelledError/KeyboardInterrupt 在边界映射中断，不包装成可重试失败。timeout 分请求超时与帧 deadline；一次逻辑获取共享最多 3 次网络尝试预算，避免 SDK 与传输层叠加重试。

## 源开发者扩展契约

```text
Source.info -> SourceInfo
async Source.discover(query, context) -> list[FrameRef]
async Source.download(ref, context) -> RawFrame
Source.decode(raw, context) -> RadarField | RadarDataset
```

- Source 只持有静态描述与版本化资源，不持进程全局连接、可变缓存或凭据。
- context 提供受控 transport、临时目录、限额、凭据引用、缓存 lease 和 cancellation token。所有请求走共享预算；Playwright adapter 也必须遵守帧预算、清理和 RawFrame receipt。
- download 声明原始资料；tile 模式由基础设施执行获取与必要拼接，raw-only context 禁止拼接。decode 不联网、不提交输出、不自行重试。
- locator schema 与 acquisition/decoder/resource 版本明确；历史查询不支持就拒绝，不拿当前图替代。
- 内置来源懒加载；第三方 entry-point group 为 `radiust.sources`，重复 id 拒绝。
- 每源遵守统一 fixture contract；缺 fixture 的项不得标记 contract_passed，见 [source-inventory.md](../source-inventory.md)。
