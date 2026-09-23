# Phase 0 研究与决策

研究日期：2026-09-22。以当前仓库、特性规格及两位 Luna worker 的只读调查为依据，不需要查询公网来源或旧凭据。以下为实施决策，未将设计当成功能验证。

## R1：扩展现有 Python CLI

- Decision：保持 Python >=3.10 / Click 8、Pillow/NumPy 的现有体系；新增 CLI 编排及独立显示模块，不引入全屏 UI 或新的服务。
- Rationale：`pyproject.toml`、`docs/architecture.md` 显示 CLI/来源编排均在 Python；Rust 2024/MSRV1.85 负责部分 I/O。`cli/reporting.py` 目前直接 str 对象，是人类布局替换入口。
- Alternatives considered：重写 Rust CLI 会跨越现有 SDK 边界；引入终端框架不能直接解决 JSON、安全和图形协议兼容。先用现有 Click、标准库 unicodedata 和显式布局策略处理本次宽度矩阵，不新增依赖；若实施测试发现复杂字符宽度不足，再用测例说明需要专用宽度库。

## R2：latest 仅在 CLI 规范化

- Decision：新增共享 CLI 查询构造，只有 latest/at/start/end 全缺省才 latest=true；cat 来源路径复用，文件路径不进入此逻辑。
- Rationale：`cli/main.py::_query` 是现有入口，SDK Query 的调用者语义须保持。单来源产品默认和起报时间逻辑复用 query 层。
- Alternatives considered：修改 Query 默认值会影响 SDK；先填 latest 再解析会掩盖半范围或制造与显式时间的冲突。

## R3：独立聚合发现模型与调度器

- Decision：新增 `discovery.py` 负责目录目标展开/状态聚合；CLI 转发与报告。每目标一个终态，先建立可计数目标集合再调度；来源展开失败产生来源级占位记录。
- Rationale：`DownloadReport` 使用 written/skipped 等下载状态，不适合发现；`reporting.py` 可接收带 schema_version 的独立字典。目录有效组合必须按来源产品所属站点展开，而非所有产品×所有站点盲目笛卡尔积。
- Alternatives considered：依次调用 CLI 或直接套下载批处理会丢失先验失败、状态和全局预算。

## R4：统一 deadline 与可终止执行边界

- Decision：新增配置 `runtime.discovery_deadline=300.0`，正有限秒数；在目录处理开始记录单一 monotonic deadline。父进程负责账本、报告、终端、临时目录及统一限额；来源发现运行于可终止 worker 进程，使用有界并发与IPC回传安全结果。
- Rationale：`context.py` 目前是 threading.Event 协作取消，不提供强制中止阻塞调用。仅 asyncio.wait_for/to_thread 无法构成“不响应取消仍5秒内退出”的设计保证。
- Implementation boundary：协调器最多 frame_concurrency 个活动目标；网络全局/host/来源限额由父级共享令牌仲裁，worker transport 在请求前取令牌、结束归还，worker退出时回收租约。不可每进程各自使用完整request上限；对于不能接入仲裁的路径保守串行执行并限制其本地请求额度，验收峰值后才允许并发。进程启动使用可移植spawn；不在命令行/日志传递凭据，配置通过私有IPC传输，仅下发该来源必要配置。插件在worker内按ID加载，不要求pickle实例。
- Cleanup：deadline/取消后停止发新工作，协作取消短宽限（≤1秒），随后终止、必要时kill并有界join；总清理及报告预算≤5秒。父级拥有每任务临时根，可在worker退出后清理；不持有正式输出写入权。IPC数据限长且逐目标回传，结果账本终态不可覆写。进度结束和终端恢复在父级finally中执行。覆盖阻塞worker、崩溃、迟到消息和IPC背压测试。
- Alternatives considered：只用async取消较简单但不能满足强制退出；每来源独立deadline会使整批时间无界。此隔离仅针对CLI全量发现，不修改普通SDK生命周期。

## R5：raw 展示值独立于科学数据

- Decision：新增 RawPreview/RGBA 显示路径，复用 `terminal/api.py` 的编码器和 TerminalSession。文件格式用Pillow检测，PNG/GIF受限读取；GIF取首画面。`cli/cat.py` 在获取之前完成renderer和科学选项冲突检查。
- Rationale：现有 terminal.show 有PNG文件特例，其他对象会进入render_field；通用原图不能构造虚假的RadarField。现有PNG特例也需要统一资源检查和安全标题。
- Alternatives considered：raw先走decode仍被科学能力阻断；仅扩展后缀判断不能阻止超限图片或伪格式。

## R6：规则注册与科学状态分离

- Decision：按 source/product/path_id/version 注册显式有序显示规则；通过证据验证才自动启用。无规则原图回退，匹配规则执行失败报错。旧资料缺失是已定义的blocked证据状态，不是可用猜测算法替代的未知决策。
- Rationale：规格FR019–024要求逐路径、可追踪的像素级兼容；已有灰度编码身份不能证明其他RGB/WMS也使用此编码。复用已确认算法时仍须检查配置、掩码及裁剪顺序。
- Alternatives considered：统一亮度转dBZ违背来源证据；借用其他来源palette、拼接不完整瓦片或合成样本冒充迁移基准均不接受。

## R7：安全布局与单一报告

- Decision：人类报告按命令建展示投影，JSON保持既有字段及类型；共同递归脱敏/安全文本层处理URL凭据、令牌和控制字符。40列用记录，80/120列表格，关键字段换行；进度仅stderr交互终端。
- Rationale：`reporting.py` 的str和通用异常str直接输出需要统一防护；config.redacted只覆盖配置，不能保护上游错误。`terminal/capabilities.py` 已存在NO_COLOR/TTY策略，应一致使用。
- Alternatives considered：只给str对象加颜色无法改善可读性；截断原因、将进度写stdout或JSON分块都会破坏规格。

## R8：治理与验证边界

- Decision：宪章占位模板不作为已批准原则；按spec明确约束做研究前/设计后检查。保持main，不自动创建分支；setup-plan返回002-cli-experience是特性定位结果。
- Rationale：spec已明确上述两点。现有文档、contract/integration/terminal测试提供回归基线；新增测试是实施工作，规划不预报通过。
- Alternatives considered：擅填宪章、修改旧科学通过状态或勾选TODO会越过本次规划范围。

## 研究结论

时间默认、聚合报告、预算配置、终止边界、raw生命周期、显示启用和证据验收均有具体决策。缺合法raw/规则/基准的来源必须在实施盘点中blocked，不阻止形成设计，也不允许宣称该来源迁移通过。

## Luna 只读调查证据补充

- CLI/调度调查：`models.py::Query` 当前强制时间选择；`sources/base.py`、`sources/legacy.py` 已有按站点latest逻辑，但实现阶段须增加同时间歧义检测，不能仅保留现有选首项行为。`registry.py` 提供目录。`transport.py` 使用阻塞urllib加asyncio.to_thread，最多3次尝试；`pipeline.py::_run_sync_worker` 取消后等待线程。因此R4的隔离决策有现状依据，不能用wait_for声称问题已解决。
- raw调查：`raw.py`/`raw_replay.py` 已有hash/size、manifest路径边界与临时资源管理；复用这些设施，不另造绕过完整性的文件读取路径。现有`cli/cat.py`不接收GIF和--raw，来源走fetch；`terminal/api.py`已有PNG特例及协议输出。
- 旧规则依据：`TODO.md`记录`core/parser/__init__.py::parse_img`、`core/scrapers/base.py::post_process`、`core/tiles/radar.py::_parse_value`及FR/PT `_png_to_map`。`migration/inventory.json`和前一特性的`source-inventory.md`记录旧基准commit `8d251601ca551fbd5c05451f1fb337fc4b75362c`，未记录可用旧仓库绝对路径。实施时须取得该基准的无凭据代码/配置快照；在此之前不能声称规则迁移通过，也不扩大读取旧凭据。
- `decoders/gray_dbz.py`只是有来源依据的灰度解码公式；`LegacyImageSource`实际来源未接入legacy_gray_dbz，仅测试GraySource使用。`resources/palettes/sg_rain_intensity.json`与`pt_rain_intensity.json`是provider ordinal分类，不能替代旧base/product YAML cookbook。
- `validation-results/migration-audit.json`记录raw-backed与blocked路径，但“存在raw”不证明合法再分发或旧规则一致。MY blocker明确缺可验证upstream raw及许可；FR WMS RGBA未证明旧post_process编码；SIDARMA缺授权和已验证palette；ID/PH/WU/OpenSnow/BMKG等缺输入仍须逐路径blocked。旧审计是既有证据快照，不作为本特性display通过清单。
- 已有测试入口：`tests/integration/test_raw_modes.py`、`tests/contract/test_raw_replay.py`、`tests/contract/test_legacy_gray_source.py`、`tests/terminal/test_renderers.py`、`tests/terminal/test_capabilities.py`、`tests/sources/test_tile_adapter_replay.py`。
- Luna执行基线：`uv run pytest -q tests/contract/test_cli_reports.py tests/contract/test_cli_cat.py tests/integration/test_batch_cancel.py tests/terminal/test_capabilities.py`，结果14 passed in 0.85s。这只证明现有回归基线，不验证新设计功能。
