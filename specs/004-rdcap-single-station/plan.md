# Implementation Plan: RDCAP 台湾、日本、菲律宾单站雷达支持

**Branch**: main（实际Git分支） | **Date**: 2026-10-01 | **Spec**: [spec.md](spec.md)

**Input**: specs/004-rdcap-single-station/spec.md。setup-plan返回的逻辑特性名为004-rdcap-single-station；仓库未配置创建分支hook，本次未切换分支。

**Status**: Phase 0/1设计完成；来源尚未实施，工具自身三国在线获取待验收。

## Summary

新增独立来源rdcap、默认产品reflectivity，公开站点为TWN/JPN/PHL国家/站码组合。原生Rust adapter刷新目录、发现实际秒/毫秒时次，以单读票据获取JSON字符串封装的CSR数值网格；保存原始响应及确定性帧绑定，解码为EPSG:4326、float32 dBZ/u16 quality，统一支持现有CLI、同步/异步SDK、科学预览及PNG/NetCDF/GeoTIFF/Zarr。

共享改动限于可选动态目录hook、RDCAP严格站点校验/路径编码、单次GET策略、来源错误分类、质量bit6、版本化palette及科学/regrid能力注册。其他来源继续既有缺省行为。首个实施门槛是三国原生HTTP完整链路，离线通过不能代替该门槛。

## Technical Context

**Language/Version**: Rust 1.92.0 / edition 2024；Python >=3.10薄SDK，沿用当前wheel声明范围。

**Primary Dependencies**: 当前workspace的reqwest 0.12/rustls、tokio、serde/serde_json、chrono、sha2、tempfile、image、netcdf、tiff、zarrs、PyO3 0.29/maturin。CSR采用受限Rust parser，不新增Python parser、curl或浏览器依赖。

**Storage**: 现有原始cache、临时文件生命周期、本地manifest-last及既有对象存储路径。新raw为原始file-response.json和确定性binding.json，复用v1 raw manifest；不继承未验收远端存储的通过结论。

**Testing**: Rust离线/loopback HTTP合同与Engine端到端、CLI报告、Python同步/异步等价验证；三国真实帧解码、四种输出读回、显式联网的三国自动获取。已有CSV是内容证据，原始HTTP保存需另留证。[quickstart](quickstart.md)定义分阶段验证。

**Target Platform**: 首轮沿用003的macOS arm64 CLI/wheel证据基线，保持当前Rust平台可构建性，不新增平台支持承诺。

**Project Type**: Rust库+原生CLI+PyO3绑定；Python仅查询/DTO/facade及显式科学互操作。

**Performance Goals**: 48站快照逐目标调度，一次发现只刷新一次目录，文件无需额外header请求。request/host/frame/decode默认并发16/4/2/2，发现worker4；全量发现在既有300秒预算内收尾。典型901²科学场约4.9MB values+quality，原始/解析/RGBA缓冲另计，按同时存活资源检查。无新增延迟SLO。

**Constraints**: 默认禁公网、TLS验证、同源artifact；request30秒/frame300秒及可配置字节/像素/临时盘上限。最多3张ticket、每张一次GET，期限包括刷新/等待。不得推断扫描高度/体扫、QC、雨强或长期历史；不依赖人工Orca会话。9999排除为版本化实测推断。

**Scale/Scope**: 一个来源/一个产品、48唯一快照站（13 TWN/20 JPN/15 PHL），在线可增补，数量非硬上限。总体24来源/26目标增至25/74，旧来源回归集合仍24/26。近期单站及既有工作流。

## Constitution Check

[宪章](../../.specify/memory/constitution.md)为未批准占位模板，示例不构成约束，宪章专属gate不适用。依据规格明确要求与当前原生架构检查，不修改宪章或roadmap。

| 检查 | Phase 0前 | Phase 1后 |
|---|---|---|
| Rust共用Engine、Python薄层 | 通过 | 通过：R01，无第二套provider pipeline |
| 独立rdcap，既有来源行为保持 | 通过 | 通过：默认hook为None，R02/R03 |
| 时间/原始字节/几何/质量/色标证据 | 通过 | 通过：R04/R06–R09、数据模型 |
| 网络/预算/取消/秘密/稳定身份 | 通过 | 通过：单次GET、同帧刷新、raw合同 |
| 离线与在线分别验收 | 通过 | 通过：上线须三国标准入口实证，当前live未通过 |
| 不冒充已知扫描高度/归档/雨强 | 通过 | 通过：metadata明确unknown/unsupported |

没有待定设计选择或需豁免的宪章违反。原生HTTP可达性是待执行的实现/上线gate，不视其已解决。扩展配置没有before_plan/after_plan hook。

## Project Structure

### Documentation (this feature)

~~~
specs/004-rdcap-single-station/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── cli-sdk.md
│   ├── provider-protocol.md
│   └── science-persistence.md
└── checklists/requirements.md
~~~

### Source Code (repository root)

~~~
crates/radiust-core/src/
├── source/{mod.rs,catalog.rs,rdcap.rs}        # 新adapter/可选目录hook
├── transport/http.rs                       # 可选single-attempt策略
├── model.rs                                # 限定站点校验/目录DTO
├── engine.rs                               # 动态目标/科学分派/能力gate
├── science/rdcap.rs                         # 拟新增decoder子模块
├── science.rs                              # 现有decoder入口
├── output/png.rs                           # 版本化palette/统一预览
├── identity.rs,raw_manifest.rs,download.rs  # 身份/重放/安全模板
├── errors.rs,error_contract.rs             # typed来源错误与安全code
└── limits.rs,cache/,storage/                # 复用预算/缓存/提交
crates/radiust-cli/src/{lib.rs,commands/}    # 既有命令、目录metadata展示
crates/radiust-python/src/                  # 共用Engine、必要DTO字段
python/radiust/{models.py,registry.py}       # 元信息映射、严格站点校验
python/radiust/{rust_client.py,_bridge.py,errors.py} # 薄report/replay入口与code
python/radiust/resources/catalog.json        # 48站离线目录
python/radiust/resources/palettes/rdcap_reflectivity.json # 拟新增色标
tests/fixtures/sources/rdcap/                # 拟新增三国/异常回放
crates/radiust-core/tests/rdcap_contract.rs   # 拟新增合同验证
tests/test_rdcap_sdk.py                      # 拟新增SDK等价验证
scripts/validation/validate_rdcap.py         # 拟新增标准入口live/读回
docs/rdcap-single-station-analysis.md        # 已有研究证据
validation-results/rdcap-analysis/           # 已有研究样本
~~~

**Structure Decision**: 在现有三个crate集成，新增文件标为拟新增。研究probe不升级为生产adapter；后续speckit-tasks拆分任务，本命令不创建tasks.md。

## Implementation Sequence and Acceptance

1. 传输可行性：loopback验证单次GET/空内容/同key刷新；标准Engine路径验证三国HTTPS原始响应。任一国家未成功，记录live阻塞；离线工作可继续，不宣称全范围完成。
2. 目录/发现：48站、动态新站、BALE冲突、短码消歧、无资料站独立终态；UTC毫秒、精确at、半开range，全来源快照74目标。
3. 原始/科学场：原始JSON字节、确定性帧绑定、v1 manifest重放；CSR、翻转、中心坐标、9999质量/推断、资源边界。
4. 输出/入口：离散palette、四种输出读回、native/geographic能力、CLI/同步异步等价、cache、安全模板。
5. 最终验收：异常/取消/部分失败、旧TW/TW-HTTP/PH回归与当前计数，逐项记录SC-001–008；live、离线science、其他未关闭能力分别记账。

| 要求 | 设计/验证入口 |
|---|---|
| FR-001–003、019；SC-001 | R01–R03、cli-sdk、目录/48站 |
| FR-004–006；SC-002 | R04、provider-protocol、key/冲突/at/range |
| FR-007–011、020–021；SC-003、007 | R05/R06/R10、science-persistence、live/故障合同 |
| FR-012–016；SC-004–005 | R07–R09、数据模型、三国/8点/几何/质量 |
| FR-017–018；SC-006、008 | cli-sdk/science-persistence、四输出/生命周期 |
| FR-022–024 | 能力metadata/证据边界、旧来源回归/独立live |

## Complexity Tracking

不适用：没有宪章违反或新增架构层。必要共享契约加法及替代方案见[research](research.md)。
