# Legacy 显示与离线复核契约

## 启用边界

显示引擎接受已取得的 RawFrame/离线 fixture bundle、来源/产品/path_id 和经过验证的规则注册项；不联网，不调用 Source.decode，不生成 RadarField 或正式 manifest。普通文件没有可信来源关联时不启用规则。

同一规则的顺序、配置覆盖方式与旧行为逐项对应。灰度编码 `clip(dBZ * 16 / 5, 0, 224)` 及版本 `旧项目-gray-dbz-v1` 仅描述有来源证明的旧编码；反算 `gray / 16 * 5` 不赋予 RGB/WMS 或未知灰度图科学有效性。alpha=0 为缺测，不透明黑色可为零；不能把两者合并。

## 迁移台账

在 `migration/sources/*.json` 增加独立 display_migration 区域（同步对应 schema/audit），包含每产品/path_id 的状态和 data-model.md 定义的证据。保留现有获取及科学状态字段。台账覆盖纳入范围的旧代码和配置路径，包括产品覆盖、FR/PT 特殊映射及 RainViewer/Windy/BMKG 三类瓦片。OpenSnow/WU 仅保留采集记录，不进入本次legacy-display路径台账。

passed：至少一组合法输入与可定位基准完整复核；无有意变更须尺寸、像素、alpha、背景及缺测逐项完全一致。有意差异另列原因、影响及接受结论，不记录为“无差异”。difference_pending：比对产生尚未接受差异。blocked：缺 raw、许可/来源证明、配置、基准或完整瓦片；每项列具体缺失物，不自动借用其他来源。

## 离线验证入口（实施阶段新增）

```bash
uv run python scripts/validation/compare_legacy_display.py \
  --manifest tests/fixtures/legacy-display/manifest.json \
  --report validation-results/legacy-display.json
```

manifest 仅接受仓库内/显式 fixture 根下的文件、来源/产品/path_id、规则身份、输入 SHA-256、基准身份/hash、合法材料说明；拒绝 URL 下载和越界路径。输出逐路径证据与统计，不自动将差异接受或更改科学验证状态。退出 0 代表所有条目 passed；存在 blocked 或 difference_pending 返回非零，报告仍完整。实现者将结果合入 `validation-results/us2.md`、`validation-results/us6-inventory.md` 与 migration 台账。

缺材料条目仍需存在于 manifest/覆盖台账，不能因跳过执行而消失。当前没有该验证脚本和 manifest；这是待实现接口，不是本次已经运行的验收结果。
