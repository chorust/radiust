# CLI

The Thailand TMD GIF source requires the system Tesseract executable to read
provider observation times from image footers. See [TMD source requirements](tmd-source.md).

入口是 `radiust`，每次命令都是一个有界任务。除 `cat` 外，标准输出只包含一个最终报告；`--json` 输出 `schema_version: 1` 的 JSON，进度和诊断走标准错误。

## 常用命令

```bash
radiust list sources --json
radiust list products my --json
radiust list stations my --json
radiust discover my --json
radiust discover all --json
radiust download my --at 2025-12-29T06:50:01Z --output ./data --json
radiust doctor --json
radiust config show --json
radiust cache status --json
radiust cache gc --dry-run --json
```

配置文件可放在根命令或支持的叶命令上：

```bash
radiust --conf ./config/example.yaml config show --json
radiust download my --conf ./config/example.yaml --latest --output ./data --json
```

`--quiet` 和 `--verbose` 互斥；`--quiet` 只影响人类进度，不能抑制显式 `--json` 报告。默认配置见 [config/example.yaml](../config/example.yaml)。

来源模式 `discover`、`download`、`cat` 省略所有时间选择器时默认 `--latest`；显式 `--at` 或完整的 `--start/--end` 保持原查询，不附加 latest。半范围或同时指定冲突选择器会报错。latest 按所选产品/站点分别取得最新帧，discover/download 可以返回多个站点；cat 必须消除歧义并选定唯一帧。文件 NetCDF 有多个时次时仍需显式 `--at`。

`discover all [--latest] [--max-age SECONDS] [--json]` 遍历目录中的来源、产品及其有效站点组合。它拒绝 `--at/--start/--end/--base-time/--product/--station`，需这些过滤条件时请查询单一来源。每目标保留独立结果，即使缺凭据、来源退役、网络受限、无数据或发生错误。`counts.total` 等于所有状态计数之和；成功但没有科学解码能力的帧仍可以是发现成功。退出码：0 全成功、4 部分成功、3 全部无数据/过期或目录为空、5 其他全部失败、2 参数错误、130 中断。默认网络关闭时不会擅自请求公网，可能返回多个 `network_restricted` 状态。整批预算通过 `runtime.discovery_deadline` 配置，默认 300 秒。

聚合状态为 `success`、`no_data`、`stale`、`missing_credentials`、`retired`、`network_restricted`、`upstream_failed`、`ambiguous`、`timeout`、`cancelled`、`not_started`。当前使用保守的逐目标 worker 调度；已有超时和后代进程清理测试，但目录展开、共享请求限额与全部中断场景尚未满足完整验收。

## download

默认流程是 `discover → download → decode → output`，默认格式为 NetCDF，默认输出根目录为 `./data`。可用 `--format netcdf|geotiff|png|zarr`。`--grid geographic` 必须同时提供 `--bbox west,south,east,north` 和 `--resolution`；`--raw-only` 会绕过解码、拼接和重网格。

`--dry-run` 只做发现和目标检查，不获取 artifact、不写正式目录。`--overwrite` 会重新验证并替换已有完整输出；同一路径已有不同身份时，未指定覆盖会返回 `output_conflict`。`--cache-dir` 和 `--no-cache` 只影响可删除缓存，不能删除正式 raw。

退出码是 0 成功/跳过，2 参数或配置预检错误，3 无数据或全部过期，4 部分成功，5 全部处理失败，130 用户中断。下载报告包含每帧状态和原因。

## cat

`cat` 必须在来源查询和 `--file PATH` 之间二选一。来源模式默认 latest，并可显式选择 `--at`；文件模式读取 NetCDF、PNG 或 GIF。来源模式默认显示原始 PNG/GIF，`--raw` 可显式指定这一模式，`--decoded` 才调用科学解码。原图预览保留供应方像素颜色，不调用科学解码、重网格或正式成果提交；`--legacy-display` 可单独请求有验证证据的旧灰度显示规则，不能与 `--raw` 或 `--decoded` 同用。多站或多产品需要指定 `--station`、`--product`，同时间仍不唯一时不能任取；多个 artifact 尚无验证通过的组合规则时明确拒绝预览。图片文件本身按原图预览，`--raw`、`--decoded` 和 `--legacy-display` 只用于来源模式。

PNG/GIF 原图的颜色与亮度不证明物理反射率或雨强；text 摘要中的未知时间和单位保持 `unknown`。GIF 静态预览只取首画面并明确标注。legacy 灰度规则只有在来源、产品、子路径及输入均匹配，且有独立像素对比通过证据时，才由显式 `--legacy-display` 启用；普通原图预览不会自动套用它。未匹配/未验证的规则显示原图及原因，匹配规则执行失败会报错而不会暗中回退。多瓦片只按来源明确验证的完整组合规则处理，不推断拼接顺序或填补缺瓦片。规则版本只证明显示迁移，不升级科学验收状态。

当前清单有 25 条路径：14 条通过并在正式索引启用，11 条仍 blocked。通过项为 `au/composite`、`ca/rain`、`es/composite`、`fr/composite`、`id/composite`、`kr/composite`、`my/composite/peninsular`、`nz/rain`、`pt/composite`、`sg/composite`、`th/composite/kkn240Loop`、`th_royalrain/cappi`、`tw/observation` 和 `vn/cmax`。其中 7 条使用旧仓库原有的 raw/灰度配对；另外 7 条的合法来源 raw 由精确旧提交 `8d251601ca551fbd5c05451f1fb337fc4b75362c` 离线重放生成并冻结为 golden。后者明确不是旧仓库历史保存的灰度图；其样本与衍生图仅用于迁移验证，并遵守各 fixture 记录的上游署名和使用条件。

显示维护者可离线运行 `uv run python scripts/validation/compare_legacy_display.py --manifest tests/fixtures/legacy-display/manifest.json --report validation-results/legacy-display.json`。报告按来源、产品与子路径保留缺少合法 raw、旧配置或基准的 blocked 条目；含 blocked 或未接受差异时退出非零。可用 `uv run python scripts/validation/audit_migration.py --json` 查看逐来源显示记录与独立的科学迁移状态；台账登记齐全不代表旧灰度已迁移通过。

非 TTY 的原始图片预览要求**显式** `--renderer text`；会在获取资料之前拒绝其他 renderer。管道示例：

```bash
radiust cat --file ./frame.nc --renderer text
radiust cat --file tests/fixtures/sources/th/raw/kkn240Loop.gif --renderer text
radiust cat --file tests/fixtures/sources/th_royalrain/raw/takhli.png --renderer text
radiust cat th --raw --station cmp1 --renderer text
```

renderer 有 `auto`、`kitty`、`iterm2`、`ansi`、`text`。显式图片 renderer 只有在确认能力和 TTY 后才发送协议；未知能力会报错或由 `auto` 降级。PNG 的未知地理/物理信息会保持未知，不从颜色反推数值。

## cache 和 doctor

缓存默认位于 `~/.cache/radiust`，上限 20,000,000,000 bytes、最大年龄 30 天、维护检查间隔 24 小时。`gc --dry-run` 预览，非交互 `clear` 必须加 `--yes`。缓存根目录不能与正式输出根目录重叠。

`doctor` 默认只检查本地依赖、配置和可写性；只有显式 `--network` 才请求来源层探测。静态目录中的 `availability` 不等同于实时探测结果。

## Windy 浏览器获取路径

Windy 默认通过共享 HTTP transport 获取四张 tile。需要复现旧版 Playwright 路径时，先安装 `playwright` extra 和 Chromium，再在显式允许联网的配置中启用：

```yaml
runtime:
  allow_network: true
sources:
  windy:
    use_playwright: true
```

启用后仍受网络 opt-in、单 artifact / 整帧 / 临时数据大小限制和取消检查约束；未安装浏览器 extra 时会给出缺依赖错误。
