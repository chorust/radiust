# 实施后的离线验证指南

本文件描述验收步骤。当前已实现部分 CLI 功能及离线测试，但完整 discover-all 调度和 legacy 灰度显示迁移仍有未完成项；请以 `validation-results/002-cli-experience.md` 的实际结果为准。命令从仓库根目录运行。未提供的测试/脚本仍属于待完成项，不能把找不到测试当作通过。

## 环境与现有基线

```bash
uv sync --group dev
uv run radiust list sources --json
uv run pytest tests/contract/test_cli_acquisition.py tests/contract/test_cli_cat.py tests/contract/test_cli_reports.py tests/contract/test_cli_output.py tests/contract/test_query.py tests/terminal/test_cli_pty.py
```

保持默认 `runtime.allow_network=false`；测试通过固定时钟、注入来源目录及本地重放服务运行，不需要任何真实凭据。源码结构与依赖见 plan.md。完整构建需要项目既有 Rust/maturin 工具链；不因本次规划升级依赖。

## 1. 图片离线预览

```bash
uv run radiust cat --file tests/fixtures/sources/th/raw/kkn240Loop.gif --renderer text
uv run radiust cat --file tests/fixtures/sources/th_royalrain/raw/takhli.png --renderer text
uv run radiust cat --file tests/fixtures/sources/th/raw/kkn240Loop.gif --raw --renderer text
```

前两条成功，显示尺寸、raw、原图模式/未知身份，GIF 注明首画面；第三条参数错误退出2。本地未知图片不自动套用来源灰度规则。检查原文件哈希、缓存其他文件及正式 output 均未改变。

## 2. 默认 latest 与 raw 来源流程

新增 `tests/contract/test_cli_latest_defaults.py`、`tests/contract/test_cli_raw_preview.py`，采用 Click runner 注入固定多站目录、固定时钟和图片获取器：

```bash
uv run pytest tests/contract/test_cli_latest_defaults.py tests/contract/test_cli_raw_preview.py
```

比较 discover/download/source cat 省略选择器与显式 latest 完全同义；单站成功、多站 discover/download 全部保留而 cat 歧义；覆盖 at、完整/半范围、显式冲突、base-time、max-age及文件多时刻。raw 不调用 decode/regrid/commit，非 TTY 未显式 text 时获取调用数为零。覆盖损坏/超限/GIF/多 artifact/身份更新及获取或展示中取消；临时资源清零且退出130。

不要依赖历史 my fixture 在真实当前时钟下仍是“最新”；已有 fixture 的绝对时间查询只能用作基线。

## 3. 全量报告、预算与取消

```bash
uv run radiust discover all --json > /tmp/radiust-discovery.json
python -m json.tool /tmp/radiust-discovery.json
uv run pytest tests/contract/test_cli_discover_all.py tests/integration/test_discovery_deadline.py
```

上述两个测试文件已新增；进程级测试仍未覆盖完整浏览器/插件/并发限额矩阵。真实本地目录的第一条可能退出3/4/5，网络受限不是测试异常；固定状态组合的断言在注入目录测试里完成，不能要求所有公网来源 success。

覆盖每种 [报告状态](contracts/discovery-report.md)，包括无站点/无法展开来源、同时间歧义和科学解码不可用但发现成功。核对目标集合、null排序、逐项身份、counts和0/2/3/4/5/130退出码。禁止过滤条件在发现调用前拒绝。

用低预算配置与不响应取消的阻塞 worker 故障注入：从统一deadline耗尽到进程退出≤5秒；SIGINT到终端恢复及本次临时文件清理≤5秒。保留先完成结果、进行中timeout/cancelled、未开始not_started；迟到IPC结果不覆写终态。检查全局/host/来源并发峰值不越限。硬退出应在子进程外测量，不能仅断言 asyncio task 已取消。

## 4. 终端与安全

```bash
uv run pytest tests/contract/test_cli_output.py tests/contract/test_cli_reports.py tests/terminal
NO_COLOR=1 uv run radiust list sources
uv run radiust --quiet list sources --json > /tmp/radiust-sources.json
python -m json.tool /tmp/radiust-sources.json
```

扩展现有 PTY 测试至40/80/120列、中文/组合字符/长标识、NO_COLOR、quiet/verbose冲突、stderr进度及SIGINT。stdout只有最终报告，管道无ESC/C0/C1序列；注入带token、认证URL和恶意控制字符的上游错误，检查所有文本/JSON/诊断均脱敏。真实终端目视检查三份成功/无数据/部分失败报告，验收者每份30秒内指出数量、受影响目标及有依据建议（SC-006）。

## 5. 全来源灰度证据

先按 [显示契约](contracts/legacy-display.md) 准备合法 manifest；禁止读取旧仓库凭据或自动下载缺失 raw。

```bash
uv run python scripts/validation/compare_legacy_display.py --manifest tests/fixtures/legacy-display/manifest.json --report validation-results/legacy-display.json
uv run pytest tests/contract/test_legacy_gray_source.py tests/contract/test_migration_audit.py tests/contract/test_migration_inventory.py
uv run pytest tests/contract/test_legacy_display.py
```

compare脚本、manifest及最后一个测试文件现已存在。当前 manifest 的 25 条路径全部 blocked，所以比对命令**预期退出非零并仍生成完整报告**；机制测试使用明确标注的合成配对素材，不能算真实旧路径迁移证据。所有指定地区/瓦片产品路径必须有台账；材料齐全者逐像素/alpha/背景/缺测一致，有意差异单列复核；缺材料者 blocked 且列缺失物。核对迁移记录及 us2/us6 文档含版本、裁剪、指纹、基准、差异，并确认科学状态没有升级。

## 合并前回归

```bash
uv run pytest tests/contract tests/integration tests/terminal
uv run ruff check python tests scripts/validation
```

若改动 Rust bridge/transport，再按仓库既有构建方式运行相应 cargo tests。公网/provider验证单列且需既有显式许可。保留测试摘要、平台/终端信息及逐路径阻塞清单；仅规划完成不能勾选 TODO 或声称全部迁移通过。
