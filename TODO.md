# TODO

记录使用 CLI 时发现的问题和后续修改项。未勾选项表示尚未实现或验证。

## CLI 问题

- [ ] **为 `cat` 增加 `--raw`**
  - 当前复现：`cat vn --station VIN --latest` 在来源没有科学解码器时失败：
    `source vn has no verified scientific decoder; preserve raw data or use raw-only mode`。
  - 目标：`cat SOURCE --raw --latest` 直接获取单帧 raw artifact，在没有科学解码器时仍可用。
  - 对 PNG/GIF 等图片 raw 使用 `text`、`ansi`、`kitty`、`iterm2` 渲染规则；不执行科学解码、重网格或正式 output 提交。
  - 接入旧仓库的 legacy gray 转换作为显示路径：来源专属颜色表/阈值/background mask/范围 mask/cookbook，最后编码为 `0..224` 灰度图。
  - `旧项目` 的 legacy `post_process` 编码是 `gray = clip(dBZ * 16 / 5, 0, 224)`，反算为 `dBZ = gray / 16 * 5`；新 decoder 已实现为 `LegacyGrayDbzDecoder`。
  - 只有明确确认 artifact 是该 `post_process` 生成的灰度结果时才使用这个反算；当前 provider 的 RGB/WMS 图不能仅凭亮度自动套用。透明像素仍保留为缺测质量，opaque black 可以表示 `0 dBZ`。
  - 仍要求明确选择单帧；多个站点时必须使用 `--station`，或者提供明确的 `--at`。
  - 增加离线回放、非 TTY、取消清理、凭据脱敏和图片协议测试，并更新 `docs/cli.md`。

- [ ] **迁移 `旧项目` 的灰度显示算法**
  - 旧实现位置：`core/parser/__init__.py::parse_img`、`core/scrapers/base.py::post_process`、`core/tiles/radar.py::_parse_value`，以及 FR/PT 的 `_png_to_map`。
  - 新 decoder 位置：`python/radiust/decoders/gray_dbz.py`；算法版本为 `旧项目-gray-dbz-v1`，公式为 `gray / 16 * 5`。
  - 迁移各来源 `conf/parse/**/base.yaml` 和产品专属 YAML 中的 `data_box`、`color_list`、`zero_color_list`、`cookbook`、`value_threshold`、`zero_threshold`、`apply_disk`、`inpaint` 与 resize 语义到新仓库的版本化资源/decoder 配置。
  - 覆盖旧仓库已有的 AU、CA、ES、ID、KR、MY、NZ、PH、SG、TH、TW、VN、FR、PT 及 RainViewer/Windy/OpenSnow/WU/BMKG tile 路径；没有对应合法 raw 或配置的来源保持 blocked。
  - 优先用本地 fixture 做新旧灰度输出的像素、尺寸、透明/背景和缺测比较；不读取旧仓库凭据，不把旧灰度结果直接升级为科学验收。
  - 记录每个来源的灰度算法版本、输入 raw hash、输出 hash、裁剪区域和已知差异；完成后更新 `migration/sources/*.json`、`validation-results/us2.md` 和 `validation-results/us6-inventory.md`。

- [ ] **将 `--latest` 作为需要帧选择的 CLI 命令默认选项**
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

## 已确认的当前行为

- `list sources` 只查看静态目录，不请求来源。
- `discover SOURCE --latest` 会请求来源并列出最新帧。
- `download SOURCE --raw-only` 可以在没有科学解码器时保存 raw；当前需要再使用 `cat --file PATH` 查看图片。
- `cat SOURCE` 当前调用科学 `fetch`，因此对 `vn`、`my` 等未完成科学解码验证的来源会 fail closed。
- `--latest` 是当前 CLI 已存在的正确选项拼写；`--lastest` 不是有效选项。
