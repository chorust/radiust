# Phase 0 Research: gray / dBZ

**Date**: 2026-10-02 | **Spec**: [spec.md](spec.md) | **Status**: 研究完成，必要澄清已解决；以下均为设计决策，尚未实施。

## 方法与依据

三名只读 subagents 分别研究灰度转换/质量、像素数据/格式保存、命名/CLI/SDK兼容；主代理核对身份/提交、资源限制、规则证据，并静态读取15条既有gray基准的可见像素范围。没有运行产品测试或联网验证来源。

主依据为现有 [gray实现](../../crates/radiust-core/src/legacy_display.rs)、[Engine](../../crates/radiust-core/src/engine.rs)、[model](../../crates/radiust-core/src/model.rs)、[identity](../../crates/radiust-core/src/identity.rs)、[commit](../../crates/radiust-core/src/storage/commit.rs)、[PyO3](../../crates/radiust-core/src/python_types.rs)、[CLI](../../crates/radiust-cli/src/lib.rs)、[Python Client](../../python/radiust/rust_client.py) 与 [23条路径证据](../../validation-results/legacy-display.json)。[constitution](../../.specify/memory/constitution.md)仍为占位模板，不从其示例推定约束。

## R1 — 编码范围与NZ来源例外

**Decision**: 固定编码ID `gray-dbz-v1`，有效整数0～224，`dBZ=gray*5/16`，步长0.3125。来源通过规则输出使用 `source-upper-clip-v1`：仅反算时以 `min(gray,224)` 计算，原gray不改；本地输入使用 `strict-v1`，可见超界即整帧拒绝。保持原quality，无效像素仍NaN。新增 `encoding_adjustment` 同形状u8数组（bit1=upper_clipped）和数量/策略，保存在数值成果、SDK和报告；它独立于既有quality bits，不冒用missing/recovered。

**Rationale**: 主代理静态检查NZ基准发现6个可见值超过224、最大229；用户明确选择“仅在dBZ解码时截断到224，保留原gray图”。既有bicubic核带负权重，两次输出仅裁到0～255，可产生上限过冲。这一代码机制与现象一致，尚未运行重放定位每个贡献点。

NZ基准零基(row,column,value)：`(128,274,228)`、`(153,198,226)`、`(182,154,227)`、`(212,174,225)`、`(404,257,225)`、`(405,288,229)`。核对来源：[old-gray.png](../../tests/fixtures/legacy-display/nz/old-gray.png)。其余14条本次读取的基准没有可见超界或非灰度；这只是基准范围检查，不能替代005数值/质量验收。6个截断位置中哪些实际有效，须由实施后的质量旁路确定；仅有效位置得到70 dBZ。

**Alternatives considered**: 整帧拒绝会无法满足全部15条来源验收；unknown/NaN部分继续用户未选；改原gray会破坏历史像素；对本地任意输入自动截断会放宽其声明而无来源依据。

## R2 — 质量旁路与像素保持

**Decision**: gray转换算法、处理顺序和输出字节保持不变，增加伴随质量数组及处理记录。在alpha被强制255、crop补边、background/disk、palette、inpaint和resize前后同步记录；同一Rust转换向gray预览与dbz提供结果，不重新在dBZ上inpaint。

quality沿用0=无已知异常、1=missing、2=outside_coverage、4=unknown_color、8=recovered、16=interpolated、32=below_detection、64=source_annotation。已证明成功修补的可恢复unknown，最终移除4加入8，原原因保存在 `origin_quality`；透明/站外/补边不能因写入code清除。`zero_invalid`不是recovered，ES的code0映射到gray224也不能把unknown变成70 dBZ。resize按相同非零权重支持域合并，包括负权重；有未解决无效贡献则NaN，多贡献加16，nearest只复制。不得对quality进行数值插值。

**Rationale**: 当前返回类型只有RGBA，`encode_gray`/resize会重写alpha，最终PNG已经无法证明原mask；仅从最终黑色不能判断缺测。现有PNG/regrid把1|2|4视作无效，保留旧原因与最终有效性需分开。`grid.rs`中的 `QUALITY_RECOVERED=32` 名称与既有契约矛盾，计划改为 `QUALITY_BELOW_DETECTION` 并保持数值和原过滤行为。

**Alternatives considered**: 重建最终PNG缺测掩码会猜测；全部修补目标标成功缺少依据；只OR `4|8` 会使修补结果继续被旧消费者拒绝；改变inpaint/resize算法或仅nearest传播mask会改变像素或遗漏真实贡献。

## R3 — 来源边界与直接数值优先

**Decision**: source/product/path/必要station/格式/尺寸/frame_index均承接原始资料和证据，仅匹配passed规则后启用gray→dbz。blocked/unmatched回退可用于raw预览，但dbz明确失败。RainViewer composite、TW grid、RDCAP reflectivity直接走既有数值分支，保留数据/质量/坐标；其他物理量继续generic科学入口，显式dbz要求reflectivity及dBZ单位。

**Rationale**: 证据为23条、15passed/8blocked。已有passed是gray像素匹配，未证明005质量保存或上游独立物理标定。CA/NZ产品rain不能按名字当雨强；FR/PT旧亮度×alpha约定只支持最终编码反算，其alpha已参与gray生成，不能反算时再次乘/除alpha。PT其他雨强产品保持原单位。

| path | 基准依据 | 宽×高 | 必须核对的限制 |
| --- | --- | --- | --- |
| au/composite | 历史配对 | 461×461 | disk、NS r1、bicubic |
| ca/rain | 固定旧代码重放 | 461×461 | crop、disk、NS r7、bicubic |
| es/composite | 固定旧代码重放 | 962×1079 | zero_invalid、code0不能自动变有效 |
| fr/composite | 固定旧代码重放 | 700×600 | 亮度×alpha、量化 |
| id/composite | 历史配对 | 461×461 | zero_invalid、bicubic |
| kr/composite | 历史配对 | 461×461 | crop、disk、NS r7、bicubic |
| my/composite/east | 历史配对 | 568×640 | 使用archive-old-gray、crop、NS r7 |
| my/composite/peninsular | 历史配对 | 568×640 | crop、NS r7 |
| nz/rain | 固定旧代码重放 | 461×461 | disk、NS r3、bicubic及6个已发现超界 |
| pt/composite | 固定旧代码重放 | 1500×1526 | 亮度×alpha、产品单位限制 |
| sg/composite | 历史配对 | 461×461 | NS r7、bicubic |
| th/composite/kkn240Loop | 固定旧代码重放 | 461×461 | crop含补边、GIF frame_index=0、NS r7 |
| th_royalrain/cappi | 固定旧代码重放 | 461×461 | takhli→THKL3、crop/disk、NS r7 |
| tw/observation | 历史配对 | 3600×3600 | zero_invalid、峰值预算 |
| vn/cmax | 历史配对 | 461×461 | disk、NS r7、bicubic |

8条blocked保持原原因：BMKG材料不足、SIDARMA旧路径未对应、PH缺配对、RainViewer旧tile不匹配、TH cmp1旧invalid门槛、TW HTTP身份未建立、TW grid缺旧显示证据、Windy同帧/通道依据不足。具体以证据表为准；RV/TW的blocked显示条目不禁用其直接数值能力。

**Alternatives considered**: 按source整体启用、仅检查gray外观、重新要求所有上游标定、把所有科学变量改成dBZ均缺少现有依据。

## R4 — 无时间/定位的像素模型

**Decision**: 新增 `PixelDbzField`，固定行列网格、可选有效时刻，含values/quality/origin_quality/原位深AlphaPlane U8/U16及alpha_bit_depth/encoding_adjustment；不放宽现有 `RadarField` 的强时刻及地理契约。用 `RasterResult` 包装Pixel与Native/nativeDataset，writer借用 `RasterView`。直接native以Arc或dataset owner+index承接，不复制Vec；NumPy/xarray按显式复制契约。

**Rationale**: 本地PNG无时间/CRS不应阻塞反算或数值保存；当前RadarField不可表达它。PyO3已有Arc和owner+index模式；现有write_science的 `.clone()` 会复制大数组，应在新入口避免。

**Alternatives considered**: 当前时间/epoch伪时间、行列冒充经纬度、随意补CRS违反FR-017；把所有RadarField时间改成optional扩大已有接口破坏面；每种来源复制整个dataset浪费预算。

## R5 — 适用格式与事务身份

**Decision**: Pixel NetCDF/Zarr写row/column、值及像素质量、完整处理记录，time只有已知时才写；像素PNG明确无地理。无完整几何拒绝GeoTIFF、bbox/geographic/resolution重网格。可信几何和处理映射完整才接既有native地理writer。

本地身份 `kind=local_gray` + content SHA +声明/编码 +可选time/geometry做域隔离digest；裸本地time/geometry=null、path/mtime不入身份。来源gray保留真实FrameRef logical_id及原始resolved revision，新增处理由processing_hash/output_id区分，不用science_revision把派生数组伪装原始revision。保留Manifest v1，新增经过校验的identity/receipt提交入口，与旧frame提交共用事务、锁、skip/overwrite、cancel fence、manifest-last。local默认raw_complete=false；created_at只表示创建时间。

**Rationale**: 现有Manifest v1只要求SHA身份和一致processing hash，未硬性绑定FrameRef；当前commit入口需要frame，可扩展入口后复用安全机制。NetCDF/Zarr读写须增加Pixel分支，旧read_selected_field→RadarField保持严格契约。

**Alternatives considered**: 为本地文件造FrameRef、升级所有manifest、绕过事务直接写最终文件、用PNG作数值文件均无必要或无法满足需求。

## R6 — CLI/SDK与机器输出兼容

**Decision**: cat新增规范--gray/--dbz；--legacy-display隐藏兼容gray。--decoded保留历史语义分派，反射率对应dbz，非反射率科学文件仍为generic。download来源新增--dbz，本地新增 `download --file PATH --dbz`；旧 `--raw` 保持“附带raw”，--raw-only是独立原始模式，不误改成cat的互斥raw参数。replay新增--dbz共用offline原始receipt。cat本地图片显式--dbz即编码声明，不再强制第二个encoding参数；SDK `decode_gray_file()` 同样构成声明。

同步/异步Client新fetch_gray/decode_gray、fetch_dbz/decode_dbz、decode_gray_file，fetch_many/stream/download增加显式mode并共用Rust调度；旧fetch/decode/write维持generic语义。新本地Pixel写入不要求伪FrameRef，已有其他field仍按旧规则要求ref。gray/dbz能力不足必须结构化失败。

机器报告保持Envelope v1及result已有类型，新增 `mode_schema_version=1` / `mode_info`：requested/actual/variable/units/method/encoding/rule/geometry/limitations。cat原result若是字符串保持字符串；download/replay按item增加。old display_mode等保留兼容读取，actual支持raw/gray/dbz/scientific/null失败。迁移提示只进人类stderr。桥接优先使用radiust_code/radiust_stage结构化属性，不依赖改名后的错误文本。

**Rationale**: 默认fetch是泛化科学读取，不能全部别名成dbz；本地旧科学文件允许rain_rate。下载--raw原本是附加raw，不等于预览模式。直接替换JSON旧result类型会破坏v1。

**Alternatives considered**: --decoded无条件别名dbz、所有fetch改单位、重写报告v1、Python循环重新调度均扩大破坏范围。新方法缺失时仅允许三类直接数值产品使用旧native方法并校验单位；校验/解码/资源/取消失败绝不触发fallback，local/gray无旧等价能力则报告扩展版本不足。

## R7 — 命名迁移、历史codec和wheel

**Decision**: 活动模块gray.rs、GrayPreview、gray resources/验证入口；old legacy_display入口和路径通过显式兼容层保留。历史参与指纹的legacy_reference、ordered_steps、operation名、旧编码wire版本原值不变，先用原序列化字段计算hash再归一活动名称。展示编码ID gray-dbz-v1同时保存历史wire version。ProcessingSpec历史output_kind="decoded"继续作为兼容wire语义，纯改名不重算既有output ID；新增gray→dbz的decoder/rule/quality/range策略版本入处理hash。tw-legacy-v1 locator、通用source/cache旧格式不做字面替换。

SDK规范GrayDbzDecoder只薄委托Rust；旧LegacyGrayDbzDecoder保留构造参数、tuple返回及历史有效输入的行为，包括旧浮点窄化与strict=False/max_gray选项，由独立Rust historical profile承接，不冒充canonical。规范入口不接受浮点小数，不因旧类兼容而放宽。wheel当前排除decoders/**，只重新纳入gray_dbz薄模块与最小/lazy __init，不恢复Python采集/解码流水线。resource打包同时覆盖规范与历史映射，Rust include_str与wheel同源可核对。

**Rationale**: 当前Python灰度decoder先cast uint8，会截断小数，不能作为严格新实现；旧配置支持max_gray255。指纹直接包含历史字段，批量改词会使既有验证失效。

**Alternatives considered**: 删除旧公开名、批量重命名JSON内部字段、复制完整Python业务、直接重新生成全部历史证据均不符合FR-009/020。

## R8 — 预算与验收边界

**Decision**: 复用现有Rust1.92/2024、PyO30.29、image0.25、netcdf0.12.1、zarrs0.23.14/Zarrv2等；无新运行时依赖。解码为O(n)，保留decode2/frame2/request16/host4及既有字节/像素/共享deadline/cancel规则。值+quality至少6B/px，gray RGBA、origin quality、alpha、adjustment、crop/inpaint/resize中间及writer缓冲都计峰值；在分配前校验预算和乘法溢出。

验收计划覆盖225整数、local拒绝与source截断、15路径、3直接路径、SDK/CLI、独立xarray数值读回、身份/取消/部分失败/兼容。现有gray passed、005 dbz、上游物理/几何、live四种结论独立；不关闭001–004待验证项，不宣称新平台或云验证通过。

**Rationale**: TW3600²大图与伴随质量使峰值明显增加，max_pixels只是上限不是内存承诺。当前代码与lockfile足以支撑选型，无需新增服务。

**Alternatives considered**: 无界分配、Python另开线程池、把静态gray检查当005通过、因005完成关闭旧spec验收均不成立。

## R9 — 2026-10-03 分析问题修订

**Decision**:

- I1：本地--raw/--gray/--dbz与cat模式schema1报告在US1/T020实现，T011/T021验证quickstart第2节；US2/T028–T030扩展来源/generic/旧拼写，不再承担本地MVP的隐含前置。
- U1：RasterInput增加NumericFile，domain=`radiust-local-numeric-v1`。真实content digest/format/变量/selection与NumericReadReceipt标识当前输入，不要求旧科学文件包含FrameRef/gray声明。原metadata仅作upstream provenance；read_dbz结果带经校验身份可write，method=file_dbz及真实再编码生成新成果身份，raw_complete=false，冲突ref拒绝。原单位/数值范围/适用时间几何条件不放宽、不重复反算。
- E1：SDK replay的keyword-only mode=None/dbz贯通Core、PyO3、bridge、Client/AsyncClient（T052/T054–T056）；T043/T061显式验证passed/direct/blocked、默认旧返回、receipt完整性和结构化错误，CLI writer接线仍在US4。
- A1：T001逐文件建立protected-artifacts.json，保护001–004已有全部文件、旧fixtures/resources/验证记录与引用的原raw/gray/规则证据。T086核对该清单；README/docs活动文档允许T036/T081–T083修改，不要求其摘要不变。基线取实施前现状，保留原工作区已有改动。
- U2：AlphaPlane区分U8/U16，先按原alpha判断透明；原值/dtype/alpha_bit_depth保存在SDK/xarray与数值文件。显示才派生非零保持uint8，不覆盖原alpha。边界0/1/255/256/65535包含读取、有效性、预算及独立持久化验收。

**Rationale**: 分析发现MVP验收存在后续阶段依赖、旧数值文件身份无法表达、SDK replay缺显式实施链、保护摘要范围与活动文档更新冲突，以及16位alpha窄化可能误判missing。当前源码的GeoTIFF reader还读取质量和provenance sidecar，因此NumericFile摘要须覆盖全部实际读取组件：自包含NetCDF单文件SHA；Zarr全树metadata/chunk清单；GeoTIFF按data/quality/provenance固定角色的SHA/size清单。多文件读取须校验期间一致性，不能只哈希主TIFF。

**Alternatives considered**: 截去MVP验收而保留隐含依赖、伪造FrameRef或gray声明、用嵌入元数据冒充raw receipt、仅列CLI replay、要求所有旧文档不变、将16位alpha无损保存替换成预览量化，均不能满足现有验收/追溯合同。未增加来源、科学公式或放宽本地strict政策。

## Phase 0 结论

编码、范围例外、质量/有效性、可选time、几何、格式、identity/commit、公开接口和历史迁移均已决策；NZ必需问题由用户回复解决并回写005规格。没有待定设计门槛。15条来源的005科学/质量验收仍是实施工作，未预填通过。
