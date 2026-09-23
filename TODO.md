# TODO

记录使用 CLI 时发现的问题和后续修改项。未勾选项表示尚未实现或验证。

## 2026-09-22 实施进展

已提供 CLI 缺省 `latest`、PNG/GIF 原图预览和来源 `cat` 默认原图模式、`discover all` 的离线状态报告，以及可读报告与终端进度。legacy 显示规则引擎、严格瓦片组合、离线逐像素比对、注册校验和审计已接入；旧灰度转换需显式 `--legacy-display`，不会自动改变原图预览。

## 2026-09-23 实施进展

此前 Task 002 完整套件重跑为 631 passed、25 skipped、2 failed、2 setup errors：本地 loopback server 测试在受限环境不能 bind，installed-wheel smoke 在本地 wheel 安装时失败，详见 `validation-results/002-cli-experience.md`。MY East 规则调整后的定向测试为 **82 passed**；Ruff 与 `git diff --check` 通过。当前 legacy display 对比为 **15 passed、0 difference_pending、8 blocked**。用户已确认 success、no_data、partial_failure 三份报告分别可在 30 秒内读懂，T028 已关闭。Spec Kit 来源批次 T037、T038、T040、T041、T042、T043、T044、T045 已按证据或用户确定的范围关闭；唯一未完成的是 **T039（PH）**，因为旧快照、当前采集和来源fixture都没有可配对 raw + old gray。T042/T043/T045 的对应路径仍在台账中保留 blocked，后续新匹配算法或新增数据源时再适配。TH cmp1 不阻塞 T040；OpenSnow/WU 已移出 legacy-display 范围但采集记录仍保留。旧快照配对样本的用户许可仅用于本次迁移验证；旧凭据未读取，合成样例没有被算作来源证据。

## CLI 问题

- [x] **为 `cat` 增加 `--raw`**
  - 当前复现：`cat vn --station VIN --latest` 在来源没有科学解码器时失败：
    `source vn has no verified scientific decoder; preserve raw data or use raw-only mode`。
  - 目标：`cat SOURCE --raw --latest` 直接获取单帧 raw artifact，在没有科学解码器时仍可用。
  - 对 PNG/GIF 等图片 raw 使用 `text`、`ansi`、`kitty`、`iterm2` 渲染规则；不执行科学解码、重网格或正式 output 提交。
  - 保留经过验证的 legacy gray 显示转换，但必须通过 `--legacy-display` 显式选择；默认及 `--raw` 预览保持原始像素，不调用科学解码、重网格或正式 output 提交。
  - `旧项目` 的 legacy `post_process` 编码是 `gray = clip(dBZ * 16 / 5, 0, 224)`，反算为 `dBZ = gray / 16 * 5`；新 decoder 已实现为 `LegacyGrayDbzDecoder`。
  - 只有明确确认 artifact 是该 `post_process` 生成的灰度结果时才使用这个反算；当前 provider 的 RGB/WMS 图不能仅凭亮度自动套用。透明像素仍保留为缺测质量，opaque black 可以表示 `0 dBZ`。
  - 仍要求明确选择单帧；多个站点时必须使用 `--station`，或者提供明确的 `--at`。
  - 增加离线回放、非 TTY、取消清理、凭据脱敏和图片协议测试，并更新 `docs/cli.md`。

- [ ] **迁移 `旧项目` 的灰度显示算法**
  - 旧实现位置：`core/parser/__init__.py::parse_img`、`core/scrapers/base.py::post_process`、`core/tiles/radar.py::_parse_value`，以及 FR/PT 的 `_png_to_map`。
  - 新 decoder 位置：`python/radiust/decoders/gray_dbz.py`；算法版本为 `旧项目-gray-dbz-v1`，公式为 `gray / 16 * 5`。
  - 迁移各来源 `conf/parse/**/base.yaml` 和产品专属 YAML 中的 `data_box`、`color_list`、`zero_color_list`、`cookbook`、`value_threshold`、`zero_threshold`、`apply_disk`、`inpaint` 与 resize 语义到新仓库的版本化资源/decoder 配置。
  - 覆盖旧仓库已有的 AU、CA、ES、ID、KR、MY、NZ、PH、SG、TH、TW、VN、FR、PT 及 RainViewer/Windy/BMKG tile 路径；OpenSnow/WU 只保留采集记录，不纳入本次 legacy-display 范围；没有对应合法 raw 或配置的纳入路径保持 blocked。
  - 优先用本地 fixture 做新旧灰度输出的像素、尺寸、透明/背景和缺测比较；不读取旧仓库凭据，不把旧灰度结果直接升级为科学验收。
  - 记录每个来源的灰度算法版本、输入 raw hash、输出 hash、裁剪区域和已知差异；完成后更新 `migration/sources/*.json`、`validation-results/us2.md` 和 `validation-results/us6-inventory.md`。

- [x] **将 `--latest` 作为需要帧选择的 CLI 命令默认选项**
  - 当前 `discover`/`cat` 在没有 `--latest` 或 `--at` 时拒绝执行。
  - 统一使用正确拼写 `--latest`；保留显式 `--at`、`--start/--end` 等查询参数的优先级和互斥校验。
  - 明确 `download` 的默认查询语义，避免默认查询多个站点造成歧义；多帧结果仍应返回完整报告或要求 `--station`。
  - 为默认行为、显式时间覆盖、无数据和多帧歧义增加 CLI contract tests。

- [ ] **让 `discover` 支持 `all`**
  - 目标命令：`radiust discover all --latest --json`。
  - 遍历目录中的所有来源、产品和站点，汇总每个来源的最新结果。
  - 没有数据、需要凭据、来源退役、上游失败或科学解码未实现时也保留结果行，记录状态和安全错误，而不是让一个来源中断整批查询。
  - 使用有界并发、共享超时和网络 opt-in；不得绕过来源的凭据、限额或重定向网络策略。
  - JSON 输出需要稳定的 source/product/station/status/valid_time/error 字段，并提供汇总 counts，方便终端和 CI 检查。
  - 增加 loopback/offline contract、部分失败、无数据和全量 source catalog 覆盖测试。

- [ ] **让 `discover` 支持组合查询多个指定来源**
  - 目标命令：`radiust discover au vn --latest --json`，允许一次指定多个来源/国家。
  - 对指定来源并发查询并汇总到同一份报告；单个来源失败、无数据或需要凭据时，其余来源仍继续查询。
  - 沿用 `discover all` 的结果行、汇总状态、有界并发、共享超时和网络 opt-in 规则。
  - 校验来源名称和参数歧义，并覆盖单来源兼容、多来源组合、部分失败及 JSON 输出的 CLI contract tests。

## 已确认的当前行为

- `list sources` 只查看静态目录，不请求来源。
- `discover SOURCE --latest` 会请求来源并列出最新帧。
- `download SOURCE --raw-only` 可以在没有科学解码器时保存 raw；可用来源模式 `cat SOURCE` 预览，也可用 `cat --file PATH` 查看本地图片。
- 来源模式 `cat SOURCE` 默认预览原始 PNG/GIF；使用 `--decoded` 才调用科学 `fetch`，没有已验证 decoder 的来源仍 fail closed。
- `--raw` 始终保留源像素；要显式查看已验证的旧灰度显示，使用 `--legacy-display`。
- `--latest` 是当前 CLI 已存在的正确选项拼写；`--lastest` 不是有效选项。
