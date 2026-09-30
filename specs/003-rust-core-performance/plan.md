# Implementation Plan: Rust 核心迁移与性能优化

**Branch**: `main`（当前 Git 分支；`setup-plan.sh` 以特性目录推导为 `003-rust-core-performance`） | **Date**: 2026-09-24 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/003-rust-core-performance/spec.md`; detailed migration boundary in repository-root `migration.md`.

## Summary

交付无需 Python 的 macOS arm64 原生命令行程序，并将内置来源、科学处理、预览、持久化与报告汇入可复用的 Rust 核心。Python 保留薄绑定及可选 xarray/Zarr 适配。先冻结旧 CLI、来源、科学数据和成果契约，再迁移核心与来源，最后切换入口和发布包。多来源发现与批量下载共享有界资源预算；验收以离线回放、旧成果读回、干净安装和同机 p95/内存对照为准。

## Technical Context

**Language/Version**: Rust 2024 edition；仓库锁定工具链 1.92.0，工作区 `rust-version` 仍为 1.85，迁移时需对齐并验证；Python SDK 当前支持 3.10–3.13。

**Primary Dependencies**: 现有 Tokio、Reqwest、OpenDAL、Rusqlite、PyO3、maturin；NetCDF/HDF5 静态构建、bundled PROJ 与资源文件、GeoTIFF 写入栈、Zarr v2 库及 CDP 客户端按 [research.md](research.md) 的互操作与分发门槛评估后锁定到 `Cargo.lock`。外部 Chromium/Tesseract 为个别来源的可选系统依赖。

**Storage**: 原始缓存为本地文件加 SQLite；正式成果为本地文件或 S3/OSS，包含 v1 manifest 与远端 generation/pointer 协议。

**Testing**: `cargo test --workspace`、现有 pytest 离线回放/合同测试、格式跨语言读回、macOS arm64 干净安装、同机至少 30 次的 p50/p95 与进程树内存测量；真实 provider 测试保持显式 opt-in。

**Target Platform**: 首发 macOS arm64；其他平台可继续构建测试，但不是首发门槛。

**Project Type**: 可嵌入 Rust library + 独立 CLI + Python wheel 科学适配；本轮无桌面 UI、后台服务、IPC 或 Swift 绑定。

**Performance Goals**: 离线 `list` 与 `discover all` p95 ≤旧版 50%；四个独立延迟来源的多来源发现 p95 ≤旧逐来源流程 2/3；`cat` 首帧 p95 至少降低 20%。

**Constraints**: 相同场景的进程树峰值 RSS 不高于旧版；全部任务遵守共享请求/主机/帧/解码限额，凭据脱敏与网络 opt-in 不放宽；科学证据边界及 manifest-last 不变；发布二进制无开发机专用动态库路径。

**Scale/Scope**: 24 个内置来源；七组 CLI 命令；四种正式输出格式；旧 Python 来源插件和默认 xarray 返回值为文档化破坏性变更。

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

`.specify/memory/constitution.md` 仍是未填写的 Spec Kit 模板，没有可执行原则或治理门槛。前置检查：PASS，无可判定冲突。本计划继续遵守 [spec.md](spec.md)、`migration.md` 及现有输出/科学证据契约。设计完成后复查：PASS；没有因接口或存储设计引入与已批准约束冲突的内容。若后续项目宪章被正式填写，应在实施前重新评估。

## Project Structure

### Documentation (this feature)

```text
specs/003-rust-core-performance/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── cli.md
│   ├── core-sdk.md
│   └── persistence.md
└── checklists/requirements.md
```

`tasks.md` 由后续 `$speckit-tasks` 生成，本阶段不创建。

### Source Code (repository root)

```text
Cargo.toml                         # 工作区：增加原生 CLI crate
crates/
├── radiust-core/src/              # Engine、Source、model、处理、transport、cache、storage、output
│   └── python.rs                  # 现有 PyO3 暴露迁至薄绑定边界
└── radiust-cli/src/               # 新增原生命令入口及报告/终端呈现
python/radiust/                    # 兼容入口与可选 xarray/Zarr 科学适配
python/radiust/resources/          # 现有目录与规则，迁移期保持唯一来源并纳入原生包
tests/                             # 现有来源 fixture、合同、集成、包装测试
migration/                         # 逐来源科学证据台账
validation-results/                # 现有迁移、科学与性能验证记录
scripts/validation/                # 现有离线审计及基准入口
```

**Structure Decision**: 继续使用现有 Cargo 工作区并增设 `radiust-cli` crate；核心库保持不依赖 Python。迁移中的 Python 实现只作对照/兼容，最终 CLI 和基础 SDK 共用核心结果。每个来源仍以目录和迁移台账作为覆盖依据；不在 CLI 与绑定中复制业务逻辑。

## Design Sequence

1. **冻结契约与性能基线**：从当前 CLI、SDK、缓存、manifest、来源 fixture 采集 golden 结果；对科学未验收路径标记 blocked，而非补造输出。记录同机 30 次冷/热启动、发现、首帧和下载场景。
2. **建立核心模型与操作上下文**：统一查询、帧身份、原始资料、科学数据、报告和错误；共享网络许可、连接、容量、时间预算、取消和进度事件。核心在无 Python 情况下可独立回放。
3. **迁移来源与有界调度**：按协议和证据状况迁移 24 来源；把 `discover all` 与 `discover SOURCE...` 放进同一公平调度，最终稳定排序；批量下载按帧限额并行、失败时停止派发和清理运行中任务。每个来源用现有 fixture 比对身份、原始摘要与科学许可。
4. **迁移处理、预览和格式**：保留原图、legacy 显示和科学输出的边界，按格式逐个实现并通过独立工具读回；`cat` 只处理选中帧，优先原图首帧路径。
5. **迁移缓存、提交与入口**：兼容现有缓存和 v1 正式成果，继续本地/远端 manifest-last；原生 CLI 和 Python 薄绑定共用核心。完成迁移文档、干净安装、发布依赖检查与性能门槛。

## Risk and Verification Gates

- **旧格式兼容**：Python `entries` 与现有 Rust `objects` 缓存索引及启动修复不兼容，完成跨版本读写/租约/修复验证前使用独立缓存根目录；本地输出 Python `flock` 与当前 Rust `create_new` 锁协议也须统一。输出身份、manifest、原始摘要和跳过规则先做双实现读回；并发写入、故障注入与远端回读通过后方可切换写路径。
- **科学正确性**：24 来源都须有覆盖状态；只有现有证据已验收的路径可声明科学解码通过。NetCDF4/GeoTIFF/Zarr v2 的跨工具读回与质量/地理信息检查分别设门槛。
- **可分发性**：系统 Chromium/Tesseract 只按需使用；系统浏览器采用隔离临时 profile。原生二进制和 wheel 在干净 macOS arm64 环境验证，检查动态库路径、`proj.db` 等资源定位与实际格式功能，不以本机编译成功代替发布通过。
- **性能**：一致的样本、机器和环境，每个场景至少 30 次，报告 p50/p95、进程树峰值 RSS、请求数与临时盘峰值；达不到门槛时先定位热点并继续优化，不放宽正确性条件。
