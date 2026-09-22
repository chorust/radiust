# Quickstart Validation Guide

本指南验证 [spec.md](spec.md) 的结果，也记录当前实现边界。仓库已经具备 CPython 3.13 的离线 MVP 闭环和本地输出/缓存/批量框架；离线 NetCDF 的 CF-1.8 检查已通过，但全量来源迁移、真实 provider、真实终端和完整安装矩阵仍未关闭。以下命令只有在对应证据实际存在时才能把 M0–M4 任务标为完成。

## 1. 前置条件

- 当前 `tests/fixtures/sources/my/` 是两个带 hash 的历史 display PNG，不能替代 M0 要求的获授权 canonical raw、发现响应、原生几何和参考值；阻塞证据见 `migration/blockers/my.md`。MY 已有注册的 HEAD/GET adapter、合成 transcript CLI 回放，以及两站真实 raw-only CLI 成功记录；TLS 验证开启，payload 已校验并从临时目录删除。24 个 HEAD source 中 16 个有 canonical hashed raw frame；另外 8 个 manifest 仍 blocked。`id` 额外保留一张旧链路 source payload，但缺 discovery JSON/URL 且旧请求未校验 TLS，因此不计为 canonical frame。raw 存在不代表科学解码和几何已验证，逐源状态见 `validation-results/us6-inventory.md`。
- 安装目标 CPython 3.10–3.13 之一、uv、所锁定 Rust 工具链；M1 已迁移 maturin 并定义 dev dependency group（pytest/pytest-asyncio、maturin、ruff、合同测试工具）。
- 依赖安装和 wheel 构建可联网；默认测试禁止公网，可启动 loopback HTTP/FTP/S3 mock 服务。在线/provider测试必须显式 opt-in。
- 命令从仓库根目录执行；测试输出写临时目录，不能使用生产 bucket/prefix。凭据通过环境或 provider chain 提供，不写到 fixture/日志。
- 数据及命令语义以 [data model](data-model.md)、[SDK](contracts/python-sdk.md)、[CLI](contracts/cli.md)、[storage](contracts/storage.md)、[encoding](contracts/encoding.md) 为准。

## 2. M1 构建并验证真实 wheel

```bash
uv sync --group dev
uv run ruff check python tests
cargo test --workspace
uv run pytest tests/unit tests/contract -m 'not live and not provider'

radiust_wheel_dir=$(mktemp -d /tmp/radiust-wheel.XXXXXX)
radiust_install_dir=$(mktemp -d /tmp/radiust-install.XXXXXX)
uv run maturin build --release --interpreter .venv/bin/python --out "$radiust_wheel_dir"
uv venv --python .venv/bin/python "$radiust_install_dir/venv"
uv pip install --python "$radiust_install_dir/venv/bin/python" "$radiust_wheel_dir"/*.whl
cd "$radiust_install_dir"
./venv/bin/python -c 'import radiust; import radiust._core; print(radiust.__version__)'
./venv/bin/radiust list sources --json
./venv/bin/radiust doctor
```

预期：构建成功，源码目录外也能导入扩展；list 是一个 schema_version=1 JSON 对象，未安装某些 extra 的来源仍可见；doctor 检查本地且不自动联网。干净核心环境能写 NetCDF，不因未装 GeoTIFF/Playwright/Zarr 而失败。

回到仓库根目录执行后续测试。每个平台/Python minor 重复干净 wheel 验收；只给实际通过的组合标记 supported。

## 3. M1 离线端到端闭环

运行 `tests/contract/test_offline_e2e.py`：通过本地 fixture 调用 Source/Client/CLI，不访问公网，也不只构造最终 Field。该离线闭环显式用 `FixtureSource` 读取 MY 历史 display PNG，验证公共生命周期；`tests/integration/test_source_cli_matrix.py::test_my_registered_adapter_cli_preserves_provider_bytes_without_fixture_fallback` 则用合成 HEAD/GET transcript 验证注册的 MY adapter 在 CLI raw-only 路径保全响应字节。两者都不构成真实 METMalaysia payload、合法留存授权或科学几何验收。其他 source 的 replay 需使用各自 source contract 和已获许可的 raw，不能把合成 `FixtureSource` 输出作为科学证据。

```bash
uv run pytest tests/contract/test_offline_e2e.py -v
uv run pytest tests/packaging/test_installed_wheel.py -v
```

第二个测试在临时 venv 安装构建产物、在源码树外执行相同闭环，防止源码 import 掩盖打包缺文件。测试必须自动执行并断言：

1. list/discover 返回 fixture 中预定来源、产品、站点和准确 UTC 时间。
2. 同步 fetch 与异步 afetch 科学值/quality/grid/provenance相同（运行标识/获取时刻等允许按契约比较）；fetch 不创建正式 output。
3. download 默认创建本地 NetCDF 与最终 manifest；读回数值、质量、时间、CRS和来源信息；关闭 Client 和 RawFrame 后结果仍可访问。
4. 第二次相同 immutable revision 下载有效 skip；上游修订模拟会重新校验并生成新身份。
5. cat text 可被捕获且无图片控制序列，ANSI 用伪TTY测试；默认非TTY cat 在读取 artifact 之前退出2。
6. 返回报告只有一个 JSON 对象；进度/警告只走stderr；日志和清单无认证字段或签名地址。

## 4. M1/M2 科学、时间与复杂获取

```bash
uv run pytest tests/contract/test_identity.py tests/contract/test_query.py -v
uv run pytest tests/contract/test_scientific_model.py tests/contract/test_encoding.py -v
uv run pytest tests/sources -m 'not live' -v
cargo test --workspace
```

测试文件由后续 tasks 创建，并至少覆盖：

| 场景 | 预期 |
| --- | --- |
| 无时区、缺时间选择、start>=end、at无匹配、多起报歧义 | 明确错误，不回退最新/相邻帧 |
| 发现后 latest 内容变化、同URL新revision | 不提交错误时次；不可永久命中旧latest |
| 调色板乱序/未知颜色/透明/no-rain/below-detection | 值与quality符合契约；默认未知色失败 |
| 相同index不同palette、缺tile、边界crop | 无损RGBA；默认缺tile失败，允许部分产品有missing标记 |
| 原生polar/cartesian及重网格 | 原生不改变；类别拒绝bilinear；dBZ在线性域插值 |
| 流式HTTP/FTP、浏览器fallback、匿名S3读取 | 共用限额与清理，不绕过时间绑定 |
| close/取消后访问返回Field | 数据有效；输入tmp最终释放，取消结果不提交 |

六类代表：my、id_sidarma、rainviewer、au、ph、fr；tw 额外检查匿名 S3。所有非例外迁移条目最终必须运行同一来源 contract，并有至少一个真实raw样本。

## 5. M3 输出、raw、缓存和故障

```bash
uv sync --group dev --extra geotiff --extra zarr --extra recovery --extra scraping
uv run pytest tests/integration/test_output_commit.py -v
uv run pytest tests/integration/test_raw_modes.py tests/integration/test_cache_lifecycle.py -v
uv run pytest tests/integration/test_batch_cancel.py tests/integration/test_resource_limits.py -v
```

预期矩阵：

- decoded → 重跑 skip → raw 补齐，同 revision 后 raw_complete 才真；缓存只有 mosaic 时重新取原 tiles，版本变化不能与旧科学结果混配。
- raw-only 不调用 decoder/mosaic/regrid，能从 raw-manifest 离线重放；清理 cache 后正式 raw 仍存在。
- 在 stage、verify、commit fence、manifest响应位置注入失败；未完成输出不能被 skip，已接受取消不能迟到提交；最终提交响应丢失需重查 committed/unknown。
- 同路径不同身份、短hash碰撞、路径越界和本地root锁冲突均明确失败；overwrite重新校验，远端旧generation仍在。
- NetCDF/Zarr完整读回；GeoTIFF数据/quality/CRS/affine和sidecar一致；PNG视觉副本不改变科学值。
- cache损坏可重新获取；GC跳过lease；no-cache结果等价；清理输出目录被拒绝。
- batch中每帧有结果，continue部分失败退出4，全部处理失败5，无数据3，参数2，取消130；stream预取不超过上限。

## 6. M2 终端验收

自动测试：

```bash
uv run pytest tests/terminal -v
```

使用离线闭环生成的文件（将路径替换为该次输出，不能拿未知PNG假设地理信息）：

```bash
radiust cat --file ./data/example.nc --renderer text
radiust cat --file ./data/example.nc --renderer ansi --width 100 --height 35
radiust cat --file ./data/example.nc --renderer kitty
radiust cat --file ./data/example.nc --renderer iterm2
radiust cat --file ./data/example.png --renderer text
```

`example.nc/png` 是前一步实际输出路径的示意，不是预置文件名。多变量文件增加 --variable；多时间增加 --at。

在真实支持的终端分别执行 ANSI、Kitty、iTerm2，不在不兼容终端硬发协议。记录终端版本、操作系统、SSH/tmux场景、图像方向/比例/legend、缺测区分、最大尺寸和Ctrl-C后的恢复。探测超时应在200ms预算后降级；普通管道默认cat退出2且不发图像序列，text可重定向。

## 7. M3/M4 显式 provider 与 live 验证

```bash
uv run pytest tests/integration/test_storage_providers.py -m provider -v
uv run pytest tests/live -m live -v
```

这些作业默认不运行。来源 live smoke 使用 `RADIUST_TEST_ALLOW_LIVE=1`；对象存储测试使用 `RADIUST_TEST_ALLOW_PROVIDER=1`。SIDARMA 与 PAGASA 的凭据型 live smoke 同时带 `live` 和 `provider` 标记，因此两项 opt-in 都要启用：

```bash
RADIUST_TEST_ALLOW_LIVE=1 RADIUST_TEST_ALLOW_PROVIDER=1 \
  uv run pytest tests/live/test_representative_sources.py -m 'live and provider' -v
```

认证值只从本机环境读取，不能贴到聊天、fixture 或日志。对应测试变量名是 `RADIUST_TEST_ID_SIDARMA_API_KEY` 与 `RADIUST_TEST_PH_TIMELINE_TOKEN`；只有在新凭据存在时才运行相应 provider smoke。UK 已作为退役来源 fail-closed，不再读取旧 DataPoint key；Weather Underground adapter 的 key 使用 `RADIUST_SOURCES__WUNDERGROUND__API_KEY`，目前没有单独的凭据型 live smoke。

真实对象存储测试直接读取以下环境变量；不需要额外 provider 配置文件：

| 目标 | 隔离 URI | 凭据和可选连接配置 |
| --- | --- | --- |
| AWS S3 | `RADIUST_PROVIDER_AWS_S3_URI` | 可使用 AWS 标准 credential chain；也可设 `AWS_ACCESS_KEY_ID`、`AWS_SECRET_ACCESS_KEY`；可选 `AWS_REGION` |
| S3-compatible | `RADIUST_PROVIDER_S3_URI` | 可选凭据对 `RADIUST_PROVIDER_S3_ACCESS_KEY`、`RADIUST_PROVIDER_S3_SECRET_KEY`、endpoint `RADIUST_PROVIDER_S3_ENDPOINT`、可选 region `RADIUST_PROVIDER_S3_REGION` |
| Aliyun OSS | `RADIUST_PROVIDER_OSS_URI` | 凭据对 `RADIUST_PROVIDER_OSS_ACCESS_KEY`、`RADIUST_PROVIDER_OSS_SECRET_KEY`、endpoint `RADIUST_PROVIDER_OSS_ENDPOINT`、可选 region `RADIUST_PROVIDER_OSS_REGION` |

URI 必须指向专用测试前缀，其路径至少有一个包含 `test` 的独立组件；测试再追加唯一的 `radiust-validation-<run-id>` 子路径。不可使用生产 bucket/prefix。测试报告只记录配置状态，不记录密钥或完整 URI。

要关闭 MY blocker，需要获授权可在仓库保留的 upstream GIF/raw 与发现响应，且带有精确有效时次、源 palette/产品说明和原生几何/控制点。现有 `tests/fixtures/sources/my/raw/*.png` 是旧的渲染 PNG，不是可接受的源样本。若不提交大文件，可在本机保留样本，并提供仓库可读取的本地路径及其许可/元数据说明；禁止把凭据贴到聊天。

验收目标为 AWS S3、至少一个 S3-compatible 和 Aliyun OSS，各自运行首次、skip、raw补齐、覆盖、冲突、multipart中断与最后manifest故障；本地也跑同矩阵。fixture/mocks通过不能代替真实provider支持证明。无账户/上游不可用时明确记录未运行或失败，不能计为通过。

live 只做有界代表源smoke，使用数据的真实发布时间判断 max_age，记录实际探测时间与异常。retired结论需要单独证据，不从一次失败推导。

## 8. M4 最终关闭

```bash
uv run pytest tests/contract/test_migration_inventory.py -v
uv run python scripts/validation/benchmark_sources.py --offline --output ./validation-results/benchmarks.json
```

Inventory test 已存在，验证每个HEAD来源有去向、fixture hash和迁移记录；不得靠手工删除缺失行来通过。Benchmark入口现已实现离线冷暖两轮重放，记录输入、环境、吞吐/RSS/请求/采样临时盘峰值及无法与旧链比较的原因。`my`/`id_sidarma`/`ph` 经注册 adapter 执行确定性合成请求重放，并单列为 synthetic-only；不代表 canonical provider bytes、来源性能或科学验证。`au`/`fr` 测正式 adapter raw 重放；`rainviewer` 测正式 adapter 的发现、获取和科学解码。当前只有 3/6 为 canonical fixture 测量，另 3 项缺 canonical raw，旧链基准也不可比；T148 的实现和报告任务已完成，但报告仍以 `task_status=partial` 表示证据不完整。完整 quickstart 仍受真实来源、provider、终端和托管 CI 门槛约束。

完成证据汇总：安装矩阵、全源离线contract、格式/存储故障、每个声明终端目视、代表live、基准、例外接受记录。对照 spec SC-001～SC-011逐项判定；未执行/失败项不能勾选完成。只有此时才进入旧仓库归档决策，本指南没有归档命令。

## Current local recheck — 2026-09-21

- The default offline suite passed **308 tests, 24 expected skips** after adding an actual-request-header test for the Taiwan HTTP adapter. No live or provider opt-in was enabled. Focused adapter/inventory/CLI contracts passed **56 tests**, the Rust FTP contract passed **8 tests**, Ruff and Cargo formatting passed, and all workflow YAML files parsed.
- T092/T093/T095/T097/T098/T100/T103 are complete for their adapter acquisition contracts. The offline terminal-contract job remains configured in `.github/workflows/offline.yml`; hosted Actions has not run for this uncommitted worktree, and Kitty/iTerm2/SSH/tmux visual acceptance remains open.
- The quickstart is still incomplete: MY canonical raw/license/time/geometry, source-specific palettes and native geometry, credentialed SIDARMA/PAGASA/WU cases, the deferred AWS/S3-compatible/OSS matrix, visible terminal acceptance, hosted platform wheel matrix, and the six-source benchmark remain unresolved. The remaining task state is tracked in `tasks.md` and `migration/release-readiness.md`.
