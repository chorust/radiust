# 数据模型：CLI 行为与显示迁移

本文件是待实现设计，复用 `models.py` 的 Query/FrameRef、`raw.py` 的 RawFrame；不更改 SDK 默认时间语义。

## 查询与结果

| 实体 | 字段 | 约束 |
| --- | --- | --- |
| CLI 时间选择 | latest, at, start, end, base_time, max_age | 无 latest/at/start/end 时注入 latest；任意半个范围保留并报错，不能转 latest；base_time 保留原规则 |
| DiscoveryTarget | source: str, product: str/null, station: str/null | 三元组唯一；无站点产品用 null；来源无法展开时只保留来源级占位目标，不能与该来源完整目标重复计数 |
| DiscoveryItem | target 三字段、status、valid_time、error、frame、capabilities | 每目标恰有一个终态；未知值 null；success 必须有唯一 FrameRef；歧义候选只在安全摘要中展示 |
| SafeError | code, message, stage, retryable | 不含凭据、原始响应正文、带认证 URL 或终端控制字符；建议由 code 映射而非臆测 |
| DiscoveryReport | schema_version=1, command, run_id, query, counts, items, error, interrupted | 沿用外层；counts.total 等于 items 长度及所有状态计数之和；不复用 DownloadReport 的 written/skipped 状态 |
| ProgressEvent | stage, completed, total: int/null | 单调完成数；未知总数不显示百分比；事件不进入 stdout 最终报告 |

状态集合：`success`, `no_data`, `stale`, `missing_credentials`, `retired`, `network_restricted`, `upstream_failed`, `ambiguous`, `timeout`, `cancelled`, `not_started`。

内部状态流：planned → running → 终态。启动前的许可/目录检查可直接进入终态。deadline 到达时 running → timeout，planned → not_started；取消时 running → cancelled，planned → not_started，report.interrupted=true。已完成结果不可被覆盖，晚到结果丢弃。状态的优先归因：静态退役/缺凭据/网络许可检查优先于启动，任务一旦启动则按实际异常分类；预算与取消竞争时中断退出码优先。

排序键为 `(source, product, station, valid_time)`，每字段以 null 小于已知值排序，时间统一 UTC。能力限制与发现状态独立；没有科学解码器的成功发现仍为 success。

## 原始图片预览

`RawPreview` 是展示值，不是 RadarField：source/product/station 可空、file_identity、valid_time 可空、raw_hash、width/height、format、frame_index、display_mode、rule_id/version、reason、RGBA 载荷。GIF frame_index 固定 0，并标明首画面。未知物理单位及观测时间不能从文件名、像素或 mtime 推断。

`display_mode` 为 original 或 legacy；legacy 只在来源/产品/规则身份、样本格式及规则适用条件同时匹配且验证通过时启用。使用独立 RGBA 副本；RawFrame 生命周期由获取上下文持有，展示结束后释放自身临时目录，缓存和用户文件不归预览所有。

## 显示规则与证据

`LegacyDisplayRule`：source、product、path_id、rule_version、encoding_version=`旧项目-gray-dbz-v1`、legacy_reference、config_hash、ordered_steps、input_constraints、validation_status。

ordered_steps 明确合并后的基础/产品覆盖配置，包括 crop、palette、zero_palette、颜色预处理、valid/zero thresholds、background/range mask、disk、gap repair、resize；不存在的步骤也需依据，而非默认为通用规则。瓦片额外包含 tile identities/order、完整集合、尺寸和组合规则；不使用科学 regrid 补足显示输入。

`DisplayEvidence`：path_id、status（passed/difference_pending/blocked）、rule_version/config_hash、input_hashes、output_hash、baseline_identity/hash、crop、shape、pixel_diff_count、alpha/background/missing 对比、intentional_differences、review_conclusion、blocked_reasons、sample_provenance。

输入与输出指纹统一 SHA-256；多 artifact 指纹按稳定身份排序记录。passed 要求材料合法且齐全、至少一组基准比对，以及差异为零或每项有意差异已复核接受。difference_pending 不可用于自动 legacy 显示；blocked 必须指出缺失材料。规则或输入约束变更必须重新验证。显示证据与科学验证状态分别保存，绝不覆盖 scientific validation。

迁移覆盖采用 source/product/path_id，而非只按国家计数；AU/CA/ES/ID/KR/MY/NZ/PH/SG/TH/TW/VN/FR/PT 及 rainviewer/windy/bmkg 必须覆盖其全部纳入范围的旧产品分支，别名映射和排除依据可追踪。OpenSnow/WU 保持在采集清单中，但不纳入本次 legacy-display 覆盖。
