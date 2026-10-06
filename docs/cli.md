# CLI

Cargo 安装的 `radiust` 是原生 Rust CLI。Python 包提供的 `radiust` console command 和 `python -m radiust` 将参数转发给同一 Rust CLI；Python SDK 通过 PyO3 调用共享 Rust Engine。本文记录当前原生命令及其明确的支持边界。

## 原生 Rust CLI

macOS Apple Silicon 可从[安装说明](installation.md#macos-apple-silicon-预编译版本预览)获取 v0.1.0 预览版；也可从仓库根目录构建或使用 Cargo 安装，步骤见[原生 CLI 安装说明](installation.md#原生-rust-命令行程序)。当前实现提供七组命令：`list`、`discover`、`download`、`cat`、`doctor`、`config` 和 `cache`。`--conf PATH`、`--json` 与 `--quiet` 是全局选项，可放在根命令或子命令后。命令默认输出人类可读报告；显式 `--json` 输出一条紧凑 JSON。TTY 上的 discover、download 和来源预览会把安全进度写到 stderr，JSON 仍只写 stdout；非 TTY 不输出进度。`--quiet` 抑制成功报告和进度，但不抑制 JSON、错误或 `cat` 预览。报告 schema 版本为 1，但各命令的字段形状不同。

### 终端报告

默认输出使用自适应表格：`list` 展示来源、产品或站点；`discover` 先显示查询条件和非零状态计数，再按来源、产品和状态汇总。相同错误合并到 Issues 区域，保留受影响来源、产品和站点。单来源的人类报告也展示部分失败；`--json` 继续使用原有报告形状。

表格时间统一显示 UTC，`oldest age` 表示该组最早一帧距当前时间的长度（秒、分钟、小时、天或年），只用于显示，不修改 discovery 状态；是否将旧帧判为 stale 仍由 `--max-age` 控制。`--verbose` 展开逐帧表格，并显示可用的帧标识、起报时间、错误阶段和可重试标记，以及安全诊断。`download` 逐帧显示状态和输出路径，`doctor`、`config show`、`cache` 使用分组字段表。

TTY 上使用状态颜色和动态进度条；进度始终写 stderr。`NO_COLOR`、`TERM=dumb` 和重定向输出禁用报告颜色；不具备终端能力时使用 ASCII 表格。表格宽度默认跟随终端，也可用 `COLUMNS` 指定；不足 60 列时使用自动换行的标签列表。机器处理请使用 `--json`。

```bash
RADIUST_RUNTIME__ALLOW_NETWORK=true radiust discover all
RADIUST_RUNTIME__ALLOW_NETWORK=true radiust discover all --verbose
radiust list sources
NO_COLOR=1 radiust doctor
COLUMNS=40 radiust list stations my
```

```bash
radiust list sources --json
radiust list fr --json
radiust list products my --json
radiust discover my --at 2025-12-29T06:50:01Z --json
radiust discover au vn --max-age 3600 --json
radiust discover all --json
radiust download fr --latest --dry-run --json
radiust download rainviewer --format png --json
radiust download rainviewer --format netcdf --json
radiust download rainviewer --format geotiff --json
radiust download rainviewer --format zarr --json
radiust download tw --product grid --format png --json
radiust download tw --product grid --format netcdf --json
radiust cat --file ./frame.png --renderer text --json
radiust cat --file ./series.nc --variable reflectivity --at 2025-12-29T06:50:01Z --json
radiust cat --file ./reflectivity.tif --renderer text
radiust cat --file ./fields.zarr --variable rain_rate --renderer text
radiust doctor --json
radiust config show --json
radiust cache status --json
```

### 查询、配置与网络

`discover SOURCE...` 接受一个或多个 source ID；单来源查询可选 `--product`、重复的 `--station`、`--at TIME`、成对的 `--start TIME --end TIME` 和 `--base-time TIME`。未指定时间选择器时使用 latest。`--max-age SECONDS` 只用于 latest，必须是正数。多来源查询（两个以上 ID 或 `all`）只支持 latest 和 `--max-age`，不接受产品、站点、起报时间或时间范围过滤。`all` 遍历目录中的 24 个 source ID（当前展开为 26 个目标）；目录登记仅代表可查询元数据，不代表有可用原生 adapter、可成功获取或科学解码已验收。

当前编译期目录有 24 个 source adapter、26 个 discovery target：`au`、`bmkg`、`ca`、`cam`、`es`、`fr`、`id`、`id_sidarma`、`kr`、`my`、`nz`、`opensnow`、`ph`、`pt`、`rainviewer`、`sg`、`th`、`th_royalrain`、`tw`、`tw-http`、`uk`、`vn`、`windy` 和 `wunderground`。普通 `id` 与 `id_sidarma` 是不同的 source ID。PH 使用隔离系统 Chromium 获取站点会话并执行当前签名模块，查询 Hybrid Reflectivity timeline 和原始数据 PNG；不再要求配置 `sources.ph.timeline_token`。浏览器支持通过 `HTTPS_PROXY=http://127.0.0.1:7897` 指定代理。1×1 占位 PNG 会被拒绝，科学解码仍未验收。Windy 支持最新时次发现和四张 256×256 PNG 的 HTTP raw 获取；`sources.windy.use_playwright: true` 会通过 CDP 获取原始 PNG 响应字节，不生成截图，也不代表科学数值已验证。浏览器路径需要安装 Chromium（可用 `RADIUST_CHROMIUM_EXECUTABLE` 指定可执行文件），并且网络必须显式开启。TH 时间绑定只在需要时调用系统 Tesseract；RoyalRain 与 TW observation 仅保留原始数据，几何/图像科学验证仍未完成。当前只有 RainViewer composite 与 TW grid 具备 Rust 科学解码；两者已有独立 fixture 数值/quality 对照，但不代表其余来源已验证。OpenSnow 在证据补齐前 fail-closed，UK 标记为退役，Weather Underground 需要外部 API key 且 raw 获取仍关闭。目录项和已注册 adapter 都不能据此视为完整来源支持。网络默认关闭。显式开启示例：

```yaml
runtime:
  allow_network: true
  discovery_workers: 4
  discovery_deadline: 300
```

```bash
radiust --conf ./native.yaml discover au vn --json
RADIUST_RUNTIME__ALLOW_NETWORK=true radiust discover au vn --json
```

配置按默认值、`~/.config/radiust/config.yaml`、当前工作目录的 `config.yaml`、`RADIUST_` 环境变量的顺序逐字段合并；后面的层覆盖前面的同名字段，其余字段保留。两个自动查找的文件不存在时跳过，存在但无法读取或配置无效时会报错。`--conf PATH` 指定的 YAML 替代当前目录的 `config.yaml` 这一层，全局文件仍提供默认配置，环境变量仍优先；显式指定的文件必须存在。不向父目录查找配置文件。

例如，把 SIDARMA Key 配置在 `~/.config/radiust/config.yaml`：

```yaml
runtime:
  allow_network: true
sources:
  id_sidarma:
    api_key: "你的 SIDARMA API Key"
```

项目的 `config.yaml` 可以只覆盖站点，保留全局 Key 和网络设置：

```yaml
sources:
  id_sidarma:
    radar_ids: [JAK]
```

在该项目目录运行 `radiust download id_sidarma --latest --raw-only --output ./data` 即可，无需 `--conf`。source 凭据放在 `sources.<source-id>` 下，`config show` 会脱敏已知凭据字段。网络关闭时，可运行目标会报告 `network_restricted`；缺少凭据和退役状态会优先保留。无数据、过期、上游错误、超时、中断和未开始也各自有状态。聚合报告中 `counts.total` 等于各终态计数总和。

SIDARMA latest 通过 `https://api.bmkg.go.id/radar/v1/arsip` 查询最近一小时的 CMAX 图像；`startTime`/`endTime` 使用 UTC，响应里的 `listTime` 与图像文件名交叉校验。`No Data` 占位图不会生成数据帧。原图请求使用公开的 `SidarmaMobile/2` User-Agent，不携带 API Key。2026-09-30 已实测 JAK 原始 PNG 下载成功；全部站点、历史查询、物理 dBZ 调色板或像素几何仍未验收，下载需使用 `--raw-only`。

### 命令边界与当前未实现项

- `list sources` 查询静态来源目录，`list SOURCE` 返回来源描述、可用状态、产品与站点 ID，JSON 将详情放在 `result` 字段，`list products SOURCE` 和 `list stations SOURCE` 返回对应目录项；这些命令不联网，也不是在线健康检查。
- `download SOURCE --dry-run` 执行发现并报告 `planned` 帧，不下载 artifact、不解码、不写正式输出。发现阶段仍受 `runtime.allow_network` 控制，因此 dry-run 不保证离线。原生批量下载的获取、解码和输出编码按 `runtime.frame_concurrency` 有界派发，解码与编码共享 `runtime.decode_workers` CPU 上限；结果和 manifest-last 提交保持输入顺序。`download SOURCE --raw-only` 已接通有界获取和正式提交；可用 `--output`、`--format`、`--overwrite` 和 `--on-error collect|continue|stop|raise`。未指定 `--format` 时使用配置中的 `output.format`（默认 `netcdf`），显式参数优先；这是原生 CLI 的配置优先级，旧 Python Click 命令的固定 `netcdf` 默认值曾遮蔽该配置。`--raw-only` 保存原始 artifact 与 `raw-manifest.json`，不做科学解码。普通解码下载支持已验收的 RainViewer composite 与 TW grid，可选择 `--format png|netcdf|geotiff|zarr`；`--raw` 会把已校验的来源 artifact 一并纳入解码成果。远端 `s3://bucket/prefix` 或 `oss://bucket/prefix` 可用 `--access-key`、`--secret-key`、`--endpoint` 覆盖 storage 配置；凭据必须成对提供并会由 `config show` 脱敏。远端提交使用 generation/pointer 协议，但真实 S3/OSS provider 尚未验收。四种格式均支持本地 manifest-last 提交；PNG 含 render sidecar，GeoTIFF 含数据/quality/provenance 三件组，Zarr store 文件列入清单。四种格式已有独立 fixture 读回合同。GeoTIFF 只接受规则坐标；EPSG:4326 的非规则纬度仅在转为规则 EPSG:3857 后输出，其他无法证明仿射的网格会拒绝。
- `cat --file PATH` 预览 PNG/WebP 原始像素，并可读取 Rust 生成的 NetCDF4、GeoTIFF 三件组和 Zarr v2 科学场。NetCDF4 使用 `--variable NAME` 选变量，`--at TIME` 选多时次中的帧；GeoTIFF/Zarr 的单时次成果可用 `--variable NAME` 校验或选择变量，并用 `--at TIME` 校验成果时间。Zarr 多变量成果必须指定变量；NetCDF4/Zarr 的歧义会在读取数据数组前拒绝。GeoTIFF 读取要求数据栅格、`_quality.tif` 和 `_provenance.json` sidecar 完整匹配；Zarr 读取要求受支持的 Radiust consolidated v2 布局。科学场支持 `--palette default`、`--vmin` 和 `--vmax`；ANSI 与 iTerm2 renderer 可用 `--width`/`--height` 指定显示范围，最多分别 1024 列和 512 行。`cat SOURCE` 只在发现出唯一帧后获取并预览原始图像，不进行科学解码；多帧歧义会报错。使用 `--json` 时，成功的 `discover` 条目和来源预览结果都包含 `source_urls` 数组，列出 locator 中的主 URL 和 artifact URL；为避免泄露，URL 的 userinfo、query 和 fragment 会被移除，保留 scheme、host、port 与 path。原图预览不接受 palette/value range 选项。renderer 支持 `auto`、`text`、`ansi`、`kitty`、`iterm2`：`auto` 在彩色 TTY（且未设置 `NO_COLOR`）中选择可用图形 renderer，否则输出文字摘要；`text` 始终输出摘要。`--quiet` 不会隐藏预览。原图像素不能解释为反射率或雨强。
- 图像 `cat` 模式显式区分 `--raw`、`--gray` 和 `--dbz`。本地 `--gray` 只检查并显示灰度像素；本地 `--dbz` 需要调用者声明 `gray-dbz-v1`，验证可见整数码在 0–224 后按 `gray × 5/16` 计算 dBZ。alpha 为零的像素是 missing，alpha 非零不缩放数值，黑色零码有效。普通灰度图不会因值域或外观自动变成 dBZ；本地模式不推断观测时间或地理定位。已有 NetCDF/GeoTIFF/Zarr 用 `--dbz` 时只接受变量 `reflectivity` 且实际单位为 `dBZ`，不会把其他变量或单位重标成 dBZ。
- 来源 `cat SOURCE --gray` 使用已验证的逐路径 gray 显示规则；没有适用规则时保留原图并在 `mode_info.actual` 报告 `raw`。`cat SOURCE --dbz` 先保留已验证的 native reflectivity/dBZ 路径；没有 direct 数据时，只对逐路径证据通过的来源图像使用 gray-dBZ 规则。blocked 路径失败关闭。旧 `--legacy-display` 仍可用于原来源预览，作为 `--gray` 兼容参数；人类输出会在 stderr 提示迁移，JSON 管道不写入提示文本。报告继续使用 Envelope v1，并用 `mode_schema_version: 1` 和 `mode_info` 添加实际模式、单位、时间/定位状态和限制。
- 本地 `download --file IMAGE --dbz` 显式声明 `gray-dbz-v1`，成功后通过 manifest-last 事务保存 Pixel dBZ 数值；当前 NetCDF、Zarr 保存数值和质量数组，PNG 是带 sidecar 的显示成果，Pixel GeoTIFF writer 尚未实现而会拒绝请求。缺少可信几何的 geographic/regrid 请求也会失败。`download SOURCE --dbz` 与 `replay MANIFEST --dbz` 使用同一 Core 分派：direct native reflectivity 优先，之后才尝试适用且已验证的来源 gray 路径。`--raw` 仍表示将同次获取的来源 artifact 附在解码成果中，不能与 `--raw-only` 同用。dBZ 解码失败会保留失败状态，不回退成成功的 raw 结果。省略 `--dbz` 时保留原 generic/native scientific 行为。具体编码、NZ clip 和验证边界见 [gray/dBZ 指南](gray-dbz.md)。
- 多帧本地图片必须明确选择帧，例如 `radiust cat --file ./frames.gif --gray --frame-index 0`；没有 `--frame-index` 且输入包含多帧时拒绝。读取已保存的数值反射率可用 `radiust cat --file ./reflectivity.nc --dbz --variable reflectivity`，Python SDK 的 `read_dbz()` / `write()` 可在没有来源 `FrameRef` 时读回和另存。旧 `--legacy-display` 是来源 gray 的兼容别名；新命令使用 `--gray`。JSON 沿用 Envelope v1，并通过 `mode_schema_version: 1` 与 `mode_info` 标识实际模式；未指定 `--dbz` 时其他科学变量继续 generic 解码并保留其真实单位。
- `doctor` 检查本机运行时、目录和可写性。指定来源时会报告相关可选能力：TH 的 Tesseract、PH 的自动会话及 Chromium 状态、Windy 的 HTTP/Playwright 原生支持状态。可选依赖缺失不会改变本机健康检查的 `checks.ok`。`--network` 明确选择后会报告 `requested_but_source_probe_is_adapter_specific`，并附带目录中的 `source_availability`；当前没有来源会声称已完成连通性探测，也不会发出网络请求。
- `config` 未写子命令时默认执行 `show`。`cache` 提供 `status`、`gc` 和 `clear`；`gc --dry-run` 与 `clear --dry-run` 只报告计划移除项，不改动缓存，非预览的 `clear` 需要 `--yes`。原生缓存默认位于 `~/.cache/radiust-rust`，与 Python 缓存分开；GC/clear 只处理索引确认的缓存条目和符合命名/年龄约束的自有临时文件，不触碰正式输出或未索引对象。
- `download` 接受 `--variable`、`--grid native|geographic`、`--bbox west,south,east,north`、`--resolution FLOAT` 和 `--resampling nearest|bilinear`。geographic 网格必须同时给出 bbox 与正数 resolution；native 网格不接受这两项；bbox 必须处于经纬度范围内且不能跨日期变更线；`--raw-only` 不能和 decoded processing 参数同用。RainViewer composite 与 TW grid 的 Rust decoder 已接入输出流程；当前已验收下载路径只有 RainViewer EPSG:4326 到 geographic EPSG:4326 可重网格。核心 `Engine::regrid()` 另支持 EPSG:4326 与 EPSG:3857 的 Web Mercator 坐标变换，但没有内置 EPSG:3857 科学来源；TW EPSG:3821 到 EPSG:4326 仍给出结构化 unsupported 错误，不会静默输出原网格。配置 schema 也接受 `output.format: netcdf|geotiff|png|zarr`、`output.grid` 和 `output.resampling`，但配置值通过校验不代表其他来源或几何都已验收。

多来源发现退出码：0 表示全部成功，4 表示部分成功，3 表示没有成功且仅有无数据/过期状态（或没有目标），5 表示其他全失败，130 表示中断，2 表示参数/配置错误。单来源发现沿用不同的错误边界；`download --dry-run` 在全部计划成功时为 0，无数据/过期为 3，其他目标失败为 5，网络全被限制时为 2，中断为 130。其他成功命令通常为 0，验证或执行错误为 2。对于 `--json` 下由命令处理器产生的错误，会输出 schema 版本 1 的错误报告；参数解析器自己的用法错误仍由 Clap 处理。

这份命令列表描述当前代码，不代表来源/格式覆盖或性能验收已经全部完成。Python source entry point 插件已改为编译期 Rust adapter，`fetch()` 默认返回绑定数据对象并通过显式 `to_xarray()` 转换。未获批准的变化包括静默改变既有命令的 JSON/退出码契约、科学值、质量、时间、地理位置或原图/科学解码边界。目录条目、fixture 或原图预览都不能替代来源和格式验收。

批量 `download` JSON 会保留逐帧部分结果；有成功也有失败时退出码为 4，中断时保留已完成帧并退出 130。Python console command 转发到同一 Rust CLI，因此使用相同报告和退出码。

## Python console command

`pip install radiust` 安装的 `radiust` 命令和 `python -m radiust` 使用与原生二进制相同的参数、报告和退出码。命令业务由 Rust CLI/Core 执行，Python 不会回退到旧 Click/source pipeline。Python SDK 与 xarray/Zarr 互操作边界见 [Python SDK 文档](python-sdk.md)。

### PH Hybrid Reflectivity

PH 需要系统 Chromium、显式网络授权和可达的 PAGASA 站点。旧 `timeline_token` 配置已不再用于认证；CSRF 与请求签名来自每次隔离会话。示例：

```bash
HTTPS_PROXY=http://127.0.0.1:7897 RADIUST_RUNTIME__ALLOW_NETWORK=true radiust discover ph
HTTPS_PROXY=http://127.0.0.1:7897 RADIUST_RUNTIME__ALLOW_NETWORK=true radiust download ph --latest --raw-only --output ./output/ph
```

数据图使用 `radar-data-image` 接口。公开 locator 不含会话 token，获取时重新建立会话。站点签名脚本结构变化、认证失败或返回 1×1 占位图会导致获取失败，不能据此声明原始数据已验证。
