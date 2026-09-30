# Quickstart Validation Guide

本指南用于在实现后验证 [spec.md](spec.md) SC-001～SC-008；写下命令不代表迁移已经完成。仓库根目录 `migration.md` 是实施边界，现有 `docs/migration.md` 和 `migration/` 仍决定哪些来源具有科学验收证据。接口/实体细节见 [CLI](contracts/cli.md)、[core/SDK](contracts/core-sdk.md)、[persistence](contracts/persistence.md) 和 [data model](data-model.md)。

## Prerequisites

- macOS arm64、`rust-toolchain.toml` 指定的 Rust 工具链、Python 3.10–3.13、uv/maturin 及现有离线 fixture。构建或装依赖可联网；默认测试禁止访问公网。
- 使用专门的临时缓存与输出目录，不指向个人生产数据或真实 bucket。真实 provider 验证需要明确 opt-in 与隔离测试前缀；不要把凭据写入命令历史、fixture 或报告。
- 实施前先冻结旧版 CLI/SDK JSON、退出码、v1 manifest、缓存样本、24 来源目录/帧身份/合法原始摘要、科学值与显示像素，并在同机采集至少 30 次旧版基线。现有 `scripts/validation/benchmark_sources.py --offline` 可提供部分来源重放，但其输出不能代替本规格的完整旧版对照。
- 当前仓库已提供 `radiust-cli` 原生 crate、`target/release/radiust` 命令和 PyO3 绑定。以下步骤中的“实施后”说明表示规格的目标行为；执行结果以本仓库对应合同测试、打包产物和验收报告为准。

## 1. Existing baseline and migration inventory

从仓库根目录执行现有检查：

```bash
uv sync --group dev
cargo test --workspace
uv run pytest tests/contract/test_migration_inventory.py tests/contract/test_manifest_validation.py tests/integration/test_discovery_deadline.py tests/integration/test_output_commit.py -q
uv run python scripts/validation/audit_migration.py --json
uv run python scripts/validation/benchmark_sources.py --offline --output /tmp/radiust-legacy-sources.json
```

预期：离线检查可运行；盘点报告明确通过、未验证与 blocked 来源，不能把目录覆盖或 raw 成功当成科学通过。采集当前旧 CLI 的 `list sources --json`、`discover all --json` 和合法 fixture 的下载/预览报告，保存完整环境、输入、版本与结果。旧基线应在修改入口前固定，并用于后续同机比较。

## 2. Native build and Python-free smoke (after implementation)

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo build --release -p radiust-cli

target/release/radiust list sources --json
target/release/radiust discover all --json
target/release/radiust discover au vn --json
target/release/radiust doctor --json
target/release/radiust config show --json
target/release/radiust cache status --json
```

当前无 Python 进程证据由 `crates/radiust-cli/tests/native_e2e.rs` 提供：测试启动原生可执行文件并清空子进程环境；`crates/radiust-core/tests/source_matrix.rs` 的本地 fixture 流程在 Rust Engine 内完成单源发现、获取、科学解码和 NetCDF 提交。原生 CLI 的 dry-run/本地 `cat`/cache 命令与 24 来源目录分别由进程合同覆盖。wheel 的 clean install、Mach-O 依赖、资源定位和四格式 Python 独立读回见 `validation-results/rust-migration-packaging.json`。实际 macOS 11 主机和 GitHub source-build workflow 尚未运行；未验收 provider 与科学路径须按本指南后续章节保持 blocked。

## 3. Multi-source discovery and cancellation (after implementation)

在可控离线回放中放入四个独立延迟目标、一个失败目标、一个不返回目标以及两个共享主机目标，分别运行：

```bash
target/release/radiust discover all --json
target/release/radiust discover au vn --json
target/release/radiust discover au au --json
target/release/radiust discover all au --json
```

预期：合法请求所有目标恰有一个最终状态，统计之和等于 total，输出按来源/产品/站点稳定排序；同主机、请求和发现活动数均不超配置上限。两个非法请求在发起请求前退出 2。使用可控悬挂目标验证整批 300 秒默认预算（测试可覆盖为更短正值）；运行中目标超时或取消，未派发目标 `not_started`，Ctrl-C 退出 130 且已完成结果保留。比较 `query.sources` 与 [CLI contract](contracts/cli.md)。

## 4. Batch, science and formal output (after implementation)

```bash
uv run pytest tests/integration/test_batch_cancel.py tests/integration/test_output_commit.py tests/contract/test_output_formats.py tests/contract/test_scientific_model.py -q
uv run python scripts/validation/compare_legacy_display.py --manifest tests/fixtures/legacy-display/manifest.json --report /tmp/radiust-legacy-display.json
```

新增原生离线合同测试必须覆盖 24 来源的目录/可用性与合法样本，逐一比较帧身份、原始 SHA-256 和既有科学许可；缺 raw 或科学证据路径记录 blocked，而不是通过。用相同帧的并行下载验证输入顺序、继续/停止、取消、同路径锁、manifest-last 和旧成果 skip/repair；网络/解码/帧上限按 [CLI contract](contracts/cli.md)。NetCDF4、GeoTIFF、PNG、Zarr v2 按 [persistence contract](contracts/persistence.md) 使用独立读者检查数值、质量位、地理信息和时间。真实 S3/OSS 验证仍须显式授权并单列结果。

`compare_legacy_display.py` 遇到缺少合法 raw、旧规则或未接受差异时会以非零状态退出，并在报告中标为 blocked；这代表证据仍缺，不等同于原生实现引入的新回归。验收时逐条区分旧有 blocked 与迁移后新增差异。

## 5. Python adapter and wheel (after implementation)

```bash
uv run maturin build --release --interpreter .venv/bin/python --out /tmp/radiust-rust-migration-wheels
uv run pytest tests/packaging/test_installed_wheel.py tests/contract/test_batch_sdk.py -q
```

在源码树外的新环境安装 wheel；基础包不得把 xarray 作为运行发现/获取的必要依赖。检查 `fetch()` 的绑定结果无需 xarray 即可读取身份/值/质量/坐标；安装可选科学依赖后显式 `to_xarray()`，比较数值、质量位、坐标、时间。验证 Python console entry 与原生命令的 JSON/退出行为一致，并按迁移说明检查旧 `radiust.sources` 插件用户与默认 xarray 返回值用户的替代路径。

## 6. Performance and release decision (after implementation)

对旧版和新版本使用同一台 macOS arm64、相同 fixture、配置、缓存冷热状态与输出位置；每场景至少 30 次，记录每次时间、进程树峰值 RSS、请求数和临时盘峰值。场景至少覆盖离线 `list`、`discover all`、四个独立延迟来源的多来源发现、代表性 `cat` 首帧和批量下载。报告 p50/p95、样本与版本指纹，并对照：list/discover all ≤旧版 p95 50%，四来源发现 ≤旧逐来源 p95 2/3，cat 首帧降低 ≥20%，峰值 RSS 不增加。现有 `benchmark_sources.py` 不能单独证明全部门槛；需在实施期扩展或新增统一对照入口。

完成前把未执行、失败和 blocked 的科学/格式/provider/平台矩阵项分别标明，不因 CLI 可用就宣称全源科学或真实云存储支持。只有 [spec.md](spec.md) 的 SC-001～SC-008 全部具备可复查证据时，才算本规格交付完成。
