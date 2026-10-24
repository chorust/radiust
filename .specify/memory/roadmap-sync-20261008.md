# Roadmap Sync Report: Radiust 下一步优先级与文档对账

**Date**: 2026-10-08
**Feature**: [项目规格](../../specs/)，001–005
**Roadmap**: [roadmap.md](roadmap.md)，v1.3.0
**Verdict**: ⚠️ PROCEED WITH UPDATES — 可开始 Desktop 规格与来源准入工作；先同步新增规格、后续证据与活动文档。

## Summary

项目已确认公众优先、本地获取、多源透明和观测先行方向。共享 Rust Engine、CLI、薄 Python SDK、原图及数值输出提供桌面建设基础；Desktop 尚无独立规格或实现。旧迁移规格的必需验收仍开放，但其全部收口不是 Desktop 的共同前置。下一步建议以有限地区和来源完成公众查看流程，同时按来源补齐在线、时间、权限和几何证据。

本次读取仓库文档、规格任务与已有验证记录；没有重新运行测试、访问公开上游或验证 registry/GitHub 的实时状态。发布结论仅转述本地发布记录。

## Reconciliation Scope & Configuration

- Loader 使用绝对项目路径确认：Roadmap 存在；`docs/adr/` 不存在；`max_findings=50`。
- 根 `specs/` 有五个已填写的规格目录，台账登记四个编号规格。
- 已登记的依赖：002 复用 001；003 复用 001/002；004 复用 001/002/003。目标存在，无已登记的依赖矛盾。
- 未编号产品主题属于方向预登记，没有对应目录不构成 phantom-entry。
- 本次只新增报告，未修改 Roadmap、规格、任务、源码或历史证据。

## Divergence Counts

| 类型 | 数量 | 说明 |
| --- | --- | --- |
| orphan-spec | 1 | 005 尚未进入 Spec Roadmap |
| roadmap-stale | 3 | RDCAP 后续证据、站点身份、平台/发布证据需要补充 |
| artifact-drift | 1 | 活动架构/维护文档仍含与后续记录冲突的实现描述 |
| phantom-entry | 0 | 001–004 均有已填写的规格目录 |
| status-lagging / status-drift | 0 | 001–004 任务统计与台账一致，必需验收仍开放 |
| abandoned-but-active | 0 | 未发现对应编号条目 |
| dependency-contradiction | 0 | 已登记依赖均可解析 |
| superseded-ADR | — | 无项目 ADR 目录 |

共五项发现；未超过配置上限。Q-04/Q-05/Q-06/Q-10 等既有问题继续保留，不重复计为新增发现。

## Current Delivery Evidence

| Spec | 当前任务勾选 | 剩余边界 |
| --- | --- | --- |
| 001 | 148/154 | MY 权限/时间/几何、代表来源科学、真实 S3/S3-compatible/OSS 与完整存储验收 |
| 002 | 66/67 | PH 合法同帧 raw/旧 gray 配对；不能以缺材料当通过 |
| 003 | 80/84 | 来源/显示配对、广泛科学/几何、真实远端 provider、最终迁移 gate |
| 004 | 55/58 | T028/T053/T058：三国正常 TLS 标准入口 raw、科学输出独立读回与最终 gate |
| 005 | 86/86 | 离线编码范围已有完成记录；不扩张为来源独立物理校准、地理定位或在线验收 |

005 的 [汇总证据](../../validation-results/gray-dbz.json)记录 15 条 gray 路径通过、8 条 blocked、3 条直接数值解码路径。所有任务勾选不能独立决定生命周期；建议先登记范围，再按对应 quickstart/验收证据判定状态。

## Findings

| ID | 类别 | 发现 | 建议 |
| --- | --- | --- | --- |
| F1 | orphan-spec | [005](../../specs/005-gray-dbz-decoding/spec.md)已实施并有离线验收，但 roadmap 编号条目与汇总仍只有 001–004 | 登记 005 的 outcome、实际复用依赖和限定验收范围；不能顺带关闭其他规格 |
| F2 | roadmap-stale | 004 当前台账主要停在 10 月 1 日 discovery 失败；[10 月 7 日 MAKI 记录](../../validation-results/rdcap-session-live-20261007.json)已有无缓存 raw 获取和数值预览 | 增补日本单站、显式 insecure_tls、未独立输出读回的证据；保留正常 TLS 和三国 gate 开放 |
| F3 | roadmap-stale | C-13/004 条目仍将 `TWN/<站码>` 等写为公开身份；[v0.1.6 发布记录](../../docs/releases.md#v016-rdcap-站点-id)及 DATA_SOURCES 已改为 `TWRCHL`/`JPMAKI`/`PHSUBI` | 保留历史身份并明确当前身份及破坏性兼容变化，上游三字母国家代码不混入公开选择器 |
| F4 | roadmap-stale | Q-07/003 仍以早期“GitHub source-build 未运行”描述证据缺口；[发布记录](../../docs/releases.md)已有 Linux/macOS crate 打包、wheel 与公开安装验证 | 核对对应 tag、平台、Python 版本后更新证据映射；实际 macOS 11 主机仍不能由 wheel tag 推定 |
| F5 | artifact-drift | [架构文档](../../docs/architecture.md)仍写 renderer 只有 auto/text、科学路径主要 RainViewer/TW；[输出维护](../../docs/output-maintenance.md)仍说远端未串接和旧 entries 一律拒绝，与后续验收/架构段落冲突 | 维护活动文档中的当前事实，历史日志保持原日期与范围；同步更新旧版本措辞，避免误导桌面设计 |

## Recommended Actions

以下是建议，未自动应用。Roadmap 变更通过 `/speckit.roadmap.write` 落地。

1. **先同步项目事实。** 处理 F1–F5；将当前实现、历史证据、未验证能力和未来目标分开。配置优先级、quality 位和 range 计数等既有问题在新调用方依赖前明确契约；不能仅凭某份实现文档重定义旧格式。
2. **下一项主要产品规格：Desktop 第一阶段。** 明确首发平台、界面与 Engine 集成方式。以一个地区和少量通过准入的产品起步，复用 Engine，分步实现默认查看、时间/出处/状态、实际帧播放、手动切换和可见回退。最终仍以 ROADMAP 八类场景验收，不因完成原图窗口就声明 Desktop 第一阶段已完成。
3. **与 Desktop 并行：来源准入与可靠性。** 候选可从 TW HTTP/observation、TW grid、RDCAP 或 RainViewer 中按区域与用途筛选；本报告不指定已获准的默认来源。逐产品核对访问/授权、实际时间、最新与历史范围、稳定性、过期阈值、图例、覆盖和能力。原图不强制等待科学解码；地图叠加须先有可靠几何。
4. **单列 004 在线收口。** 补三国标准链路、同帧科学输出和独立读回；当前 insecure_tls 单站成功无法满足既有正常 TLS 门槛。如确需改变验收条件，应明确修订范围和风险，而非静默勾选。
5. **历史收口按当前价值推进。** PH 配对、MY 授权、科学/几何、真实 provider、BR 迁移去向继续保留。共用的时间、质量、来源和读取契约问题优先；远端 provider 验收不阻塞本地桌面查看。若调整旧规格范围，单独记录决定，不能以新产品方向自动豁免。
6. **预测与跨来源拼图保持 deferred。** 当前保留可读取观测与元信息；没有验证的时间、几何和科学语义时，不提前建设完整模型插件或跨来源拼图系统。

## Suggested Next Work Order

| 顺序 | 可评审交付 | 完成证据 |
| --- | --- | --- |
| 1 | 台账与活动文档同步 | 五项漂移有明确处理，旧验收边界保留 |
| 2（并行） | Desktop 独立规格 + 首批来源准入表 | 平台/范围/集成决定明确，每个默认和备用产品有实际准入依据 |
| 3 | 默认查看的完整流程 | 普通用户从首次打开到真实观测，时间/来源/状态可理解，复用共享 Engine |
| 4 | 实际帧播放、切换与回退 | 缺帧和来源变化可见；时间、图例、覆盖同步；取消和预算有界 |
| 5（持续） | 来源质量与旧规格证据收口 | 按来源、产品、国家和 gate 独立记录，不按 adapter 数宣传覆盖 |

**限制**：本报告未作代码审计或实时可用性检查，也未重新执行既有测试；来源候选与优先级属于基于当前文档的建议。当前主要风险是把历史 raw 成功、gray 编码数值、目录标记或单站 TLS 例外误认为可直接承诺的公众默认能力。
