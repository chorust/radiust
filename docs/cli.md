# CLI

The Thailand TMD GIF source requires the system Tesseract executable to read
provider observation times from image footers. See [TMD source requirements](tmd-source.md).

入口是 `radiust`，每次命令都是一个有界任务。除 `cat` 外，标准输出只包含一个最终报告；`--json` 输出 `schema_version: 1` 的 JSON，进度和诊断走标准错误。

## 常用命令

```bash
radiust list sources --json
radiust list products my --json
radiust list stations my --json
radiust discover my --latest --json
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

## download

默认流程是 `discover → download → decode → output`，默认格式为 NetCDF，默认输出根目录为 `./data`。可用 `--format netcdf|geotiff|png|zarr`。`--grid geographic` 必须同时提供 `--bbox west,south,east,north` 和 `--resolution`；`--raw-only` 会绕过解码、拼接和重网格。

`--dry-run` 只做发现和目标检查，不获取 artifact、不写正式目录。`--overwrite` 会重新验证并替换已有完整输出；同一路径已有不同身份时，未指定覆盖会返回 `output_conflict`。`--cache-dir` 和 `--no-cache` 只影响可删除缓存，不能删除正式 raw。

退出码是 0 成功/跳过，2 参数或配置预检错误，3 无数据或全部过期，4 部分成功，5 全部处理失败，130 用户中断。下载报告包含每帧状态和原因。

## cat

`cat` 必须在来源查询和 `--file PATH` 之间二选一。来源模式只支持 `--latest` 或 `--at`；文件模式读取本项目生成的 NetCDF 或 PNG。非 TTY 默认拒绝图片 renderer，管道中请使用：

```bash
radiust cat --file ./frame.nc --renderer text
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
