# Implementation Plan: CLI 行为修复与终端体验优化

**Branch**: `main`（实际 Git 分支；setup-plan 特性标识为 `002-cli-experience`） | **Date**: 2026-09-22 | **Spec**: [spec.md](spec.md)

**Input**: `specs/002-cli-experience/spec.md`。本次执行到 Phase 1 设计，未实施功能，未生成 tasks.md。

## Summary

完整覆盖四项 TODO 与终端展示：CLI 默认 latest、来源 raw/本地 PNG-GIF 预览、全来源最新发现、全部指定来源 legacy 灰度显示迁移及安全可读报告。复用 Python SDK 的查询、获取和终端协议，分别新增聚合发现编排、raw 显示值、版本化显示规则和报告布局。科学解码、正式输出、SDK 时间默认保持既有语义。

批量发现采用统一300秒可配置预算及可终止worker边界，避免阻塞HTTP线程使取消无界。legacy逐路径盘点/比对，材料不全明确blocked；显示通过不改变科学验收状态。详见 [研究](research.md)、[模型](data-model.md) 与 [契约](contracts/cli.md)。

## Technical Context

**Language/Version**: Python >=3.10（现有元数据），Rust edition 2024 / MSRV1.85；本特性主要修改Python，不升级工具链。

**Primary Dependencies**: 复用Click >=8.1,<9，Pillow >=10,<13，NumPy >=1.24,<3，PyYAML >=6,<7及现有SDK/xarray/PyO3。进程/IPC/monotonic预算和基础布局使用标准库。旧显示操作若需SciPy/OpenCV保持现有recovery extra，缺extra提供清晰原因，不强制新增核心依赖。

**Storage**: 现有本地raw/cache/临时目录；本特性raw不提交正式output。显示规则存包内版本化资源，证据存migration及validation-results；无数据库迁移。

**Testing**: pytest、pytest-asyncio、Click runner、已有PTY测试、固定时钟/注入目录、进程级超时故障注入、逐像素golden比较；适用时运行既有Rust测试。

**Target Platform**: 既有Linux/macOS CLI和POSIX终端；不新增Windows支持承诺。

**Project Type**: Python library + CLI，私有Rust扩展；无常驻服务/全屏TUI。

**Performance Goals**: 默认整批300秒，耗尽后5秒内退出；取消后5秒内恢复终端并清理本次临时资料；40/80/120列信息完整；不做未经测量的吞吐承诺。

**Constraints**: frame_concurrency/request_concurrency/host_concurrency及来源限额继续生效；默认禁止公网；复用字节、像素、临时盘资源限制；JSON v1单报告及旧退出码兼容；不读取旧仓库凭据，不推测科学数值。

**Scale/Scope**: 当前目录26来源（含历史来源），运行时目录动态展开，不硬编码数量；14地区和3类纳入本次迁移的瓦片（RainViewer/Windy/BMKG）及其旧产品路径逐条记录。OpenSnow/WU只保留采集迁移记录，不纳入legacy-display范围。涉及list/discover/download/cat/doctor/config/cache展示；不扩展watch/动画、多语言或科学验证范围。

## Constitution Check

宪章仍为未填写模板，**无已批准条款可评估**；不把示例当规则，也不宣称正式宪章通过。spec Assumptions 已明确按规格和既有文档继续规划。以下为规格约束门禁：

| 门禁 | Phase 0 前 | Phase 1 后 |
| --- | --- | --- |
| 全部TODO/产品路径纳入，不以blocked冒充完成 | PASS：FR019–024 | PASS：legacy契约逐路径台账与证据状态 |
| raw不走科学decode/regrid/commit | PASS：FR001–005 | PASS：RawPreview独立模型及生命周期 |
| CLI默认latest不改变SDK/文件选择 | PASS：FR006–007 | PASS：共享CLI规范化，不修改Query默认 |
| 每目标唯一状态、全部失败可见、JSON兼容 | PASS：FR008–012/017 | PASS：独立报告、排序/counts/退出码契约 |
| 网络/资源/时间预算与取消 | PASS：FR013 | PASS：统一deadline、共享限额、可终止边界与进程测试 |
| 安全文本、stderr进度、终端恢复 | PASS：FR014–017 | PASS：布局/脱敏/PTY矩阵 |
| 原始资料不变、显示证据不升级科学结论 | PASS：FR019–024 | PASS：独立规则和display_migration证据 |

以上PASS表示设计满足门禁，不是实现或来源验收通过。无未解释的门禁违反。外部材料缺失按blocked处理，有明确交付状态，不作为待猜测的设计参数。

## Project Structure

### Documentation (this feature)

```text
specs/002-cli-experience/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
└── contracts/
    ├── cli.md
    ├── discovery-report.md
    └── legacy-display.md
```

### Source Code (repository root)

```text
python/radiust/
├── cli/main.py, cat.py, reporting.py       # 修改：入口、预检、输出
├── cli/query.py, progress.py              # 新增：CLI时间规范化/进度
├── discovery.py                          # 新增：目标账本与批量调度
├── discovery_worker.py                   # 新增：隔离执行/IPC/取消
├── display/                              # 新增：raw值、规则执行/注册
├── resources/legacy_display/              # 新增：经盘点版本化规则
├── models.py, config.py, context.py        # 扩展模型/预算/资源所有权
├── transport.py                          # 限额仲裁接入，单来源兼容
└── terminal/                             # 扩展图片摘要/GIF/会话恢复
scripts/validation/compare_legacy_display.py # 新增离线验证工具
migration/sources/*.json                    # 独立显示迁移证据
migration/*.schema.json                     # 按实际schema归属扩展
validation-results/us2.md, us6-inventory.md # 更新逐路径结果
tests/contract/                            # 扩展CLI与迁移契约
tests/integration/                         # 超时、取消、获取、资源
tests/terminal/                            # PTY、布局、安全输出
tests/fixtures/legacy-display/             # 新增合法基准manifest
docs/cli.md, README.md                     # 行为/离线示例
```

**Structure Decision**: 保留现有单包分层。新增display承载纯显示兼容，避免向scientific decoders塞入未验证数值语义。聚合发现调度不嵌入CLI命令体；普通SDK不依赖终端模块。规则/fixture分开，包资源包含规则，样本留测试目录。

## Phase 0：研究结果

R1–R8 已在 [research.md](research.md) 记录Decision/Rationale/Alternatives。已解决：默认注入位置、预算配置名称、硬取消策略、报告状态、显示启用及证据规则。Luna调查确认现有HTTP为to_thread阻塞I/O、取消存在等待线程结束的路径，故硬预算必须具备可终止边界。

## Phase 1：设计与实施依赖

1. 建立CLI查询规范化、报告模型与安全输出基础（FR006–007、011–012、014–017）。保留既有JSON golden及显式时间行为，先添加缺省/冲突/半范围契约测试。
2. 建立RawPreview、获取所有权和格式资源检查，接入终端renderer（FR001–005）。所有预检在获取前；已获取资料的时间/身份校验沿用严格绑定，最新地址变更不能冒充选中帧。
3. 实现all目标展开、错误归因与共享调度；统一预算/IPC/限额和中断报告一起交付（FR008–013）。完整目标账本先于工作派发；插件加载/目录展开的阻塞也须在预算边界内，无法展开时保留来源级记录。
4. 并行盘点旧规则与合法材料，逐来源/产品验证后接入display注册（FR019–024）。旧基础配置覆盖、特殊FR/PT及瓦片组合逐项对照；无材料留blocked。普通raw路径完成不能代替这一步。
5. 整合终端进度、宽度/NO_COLOR/quiet、更新文档及逐路径证据（FR014–018、024）。按 [quickstart.md](quickstart.md) 执行功能/进程/PTY与来源golden验收。

本节只给阶段依赖，不替代后续speckit-tasks。范围和验收一一对应：US1→raw，US2→latest，US3→all，US4→布局，US5→显示规则；SC001–010均在quickstart相应步骤或台账验证中落地。

## Complexity Tracking

没有宪章违反。额外复杂性是发现worker进程和父级限额仲裁：现有阻塞线程无法保证硬退出，单纯async取消不足以满足FR013/SC004。限制隔离范围为全量发现，保留已有单来源SDK行为；进程启动开销以有界worker复用控制。必须用阻塞/崩溃/取消及并发峰值证据验收，未通过不能宣称时间预算已实现。

## 完成与后续门槛

本计划产物齐全后可进入speckit-tasks；不需要先补写宪章或取得全部真实上游资料。实施交付必须公开blocked清单，不能以规划完成更新TODO完成状态。Hook检查：规划前和完成前均未发现`.specify/extensions.yml`，无before_plan/after_plan hook需执行。

实施材料风险：当前仓库仅记录旧基准commit和相对实现路径，未给出可用旧仓库位置；旧显示配置快照与合法raw需按research补充证据逐项获取。MY许可、FR编码来源及缺raw瓦片不能由现有测试素材推断通过。
