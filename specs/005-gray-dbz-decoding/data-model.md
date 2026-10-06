# Data Model: gray / dBZ

**Status**: Phase 1设计；类型名称为实现契约，尚未增加代码。

## 1. 输入与编码依据

`RasterInput = Source { frame: FrameRef, resolved_revision, acquisition_receipt } | Local { identity: RasterInputIdentity, read_receipt } | NumericFile { identity: NumericFileIdentity, read_receipt, upstream_provenance? }`。

| Entity | Fields | Validation / relationship |
| --- | --- | --- |
| RasterInputIdentity | kind=local_gray、content_sha256、encoding_declared、valid_time?、geometry? | 裸文件后两项null；SHA为有界读取的实际字节；path/mtime不入身份。domain=`radiust-local-gray-v1`后规范化digest为logical_id，content SHA为revision |
| NumericFileIdentity | kind=local_numeric、format、content_digest、variable、selection、valid_time?、geometry? | domain=`radiust-local-numeric-v1`；不含gray声明或伪FrameRef。自包含单文件SHA；Zarr为按相对key排序的全树文件SHA/size清单摘要；GeoTIFF为data/quality/provenance固定角色组件SHA/size清单摘要。selection记录实际变量及必要time/field索引，未知time/geometry不推断 |
| EncodingBasis | kind=verified_source_rule / user_declaration；encoding_id=gray-dbz-v1；encoding_version=1；rule_identity? | 来源必须匹配passed规则及当前输入；本地decode_gray_file/--dbz就是显式声明；普通打开不声明 |
| GrayRuleIdentity | source/product/path/station?、rule_version、config_hash、evidence_ref、encoding_wire_version、input_constraints | 原序列化旧字段先核对hash再映射活动名；station/path/frame_index不可猜测 |
| InputReadReceipt | digest、size、media_type、shape、原dtype/channels、frame_index? | 扩展名不足以识别科学编码；校验before narrowing。多帧文件须有明确选帧，禁止取末帧并沿用首帧时间 |
| NumericReadReceipt | content_digest、format、size/inventory、selection、validated_schema/units | 有界读取及路径安全校验后绑定实际读取内容与选择；Zarr清单包含metadata和chunk，GeoTIFF包含实际读取的data/quality/provenance sidecar；校验多文件读取期间清单/摘要未变，不以嵌入的input_identity冒充取得receipt |

local_gray、local_numeric与来源FrameRef是不同域，不能造来源ID或观测时刻。可信来源时间仅承接被选择帧的真实时间；文件创建/读取时间只作运行信息。数值文件中的旧input_identity/processing保存为upstream_provenance；它可追溯历史处理，但不是已验证的source acquisition。旧科学文件没有这些元数据仍可按真实变量/单位读入，不补gray依据；选择不明确时仍拒绝歧义。

## 2. GrayDecision / GrayFrame

`GrayDecision = Applied(GrayFrame) | Unavailable { reason, original_preview? }`。gray预览可报告actual=raw回退；dbz不接受Unavailable。

GrayFrame包含：width、height、row-major RGBA（旧输出字节）、原输入alpha/可用性信息、最终quality[u16]、必要origin_quality[u16]、ProcessingRecord及EncodingBasis。所有像素数组须同形；透明度在force-alpha之前捕获。仅有裸gray文件时，已失去的来源mask/inpaint/resize信息记录为unknown limitation，不生成虚构数组。

## 3. Numeric raster

`RasterResult { input, data, processing, mode_info }`；`RasterData = Native(Arc<RadarField>) | NativeDataset { owner: Arc<RadarDataset>, index } | Pixel(Arc<PixelDbzField>)`。

| PixelDbzField field | Type / contract |
| --- | --- |
| values | f32[height,width]；variable=reflectivity、units=dBZ；有效值0～70，已知无效NaN |
| quality | u16[height,width]；采用下表既有bits |
| origin_quality | optional u16[height,width]；保存修补前及贡献域合并的原原因，不能当作最终有效性 |
| encoding_adjustment | u8[height,width]；0=none，1=upper_clipped；即使为全零也能独立恢复 |
| alpha / alpha interpretation | optional AlphaPlane=`U8(u8[height,width])`或`U16(u16[height,width])`，alpha_bit_depth=8/16；可用原透明度无损保存。无alpha表示opaque，不要求合成数组；如物化，按输入位深填255/65535。来源复杂重采样用处理记录及可用性旁路，不捏造单一“原alpha” |
| shape / orientation | [height,width]，左上原点，零基row向下、column向右，内存row-major |
| valid_time | optional真实RFC3339 UTC；裸本地None；无值不写time变量 |
| geometry | optional GeometryEvidence；None表示只有像素网格 |
| processing | 下节记录与输入identity；gray反算时不可省略编码basis/range策略，file_dbz以实际读取记录和upstream依据区分 |

GeometryEvidence包含原始几何出处、可信Grid和crop/resize映射。只有真实CRS/坐标/extent及映射完整才能提供地理view；补边、非线性映射不能仅凭站心建立几何。crop/resize改变坐标时按真实像素边界/中心约定同步，不能只修改shape复用旧extent。没有证据时维持Pixel。

旧RadarField/RadarDataset继续表示已有native结果，不改其必需时刻/网格契约。RasterView借用values/quality及可选伴随数组，writer取得共用backing；包装与提交不复制native Vec，编码和显式NumPy/xarray转换仍计缓冲。

## 4. 像素有效性与质量

| Bit value | Meaning | Authority |
| --- | --- | --- |
| 1 | missing | alpha0、明确背景排除/缺测；不能只凭黑色 |
| 2 | outside_coverage | 有明确覆盖排除证据 |
| 4 | unknown_color | 匹配失败且未成功恢复 |
| 8 | recovered | 实际成功修补且有有效支持；保留origin原因 |
| 16 | interpolated | 多个实际非零权重输入贡献 |
| 32 | below_detection | 有阈值依据；编码零不提供依据 |
| 64 | source_annotation | 明确注记依据 |

既有native有效性继续原合同。gray反算中alpha0、quality含1/2/4或其他明确禁止原因输出NaN；finite与mask必须一致。成功恢复可恢复unknown时移除最终4、加8，origin保留4；永不可恢复的透明/站外/补边不清除。below_detection按既有适用的科学处理合同，不能把它改名recovered。encoding_adjustment是独立像素处理标志，截断不改变quality有效性。

alpha的透明性始终在原位深判断：16位1与65535均非零，不乘进dBZ。保存和xarray导出保留原dtype/值；仅显示需要8位alpha时派生`a8=0 if a16==0 else max(1,(a16*255+32767)//65535)`，不覆盖原alpha或用于数值有效性。预算按实际1/2B每像素alpha及派生预览缓冲计算。

nearest复制选择点；bicubic复用原start/weights，包含负但非零贡献，各步组合quality/origin；未解决无效贡献使结果NaN。图像保持原算术，来源输出超224的可见整数在反算前设置adjustment=1，`value=min(gray,224)*5/16`仅应用于有效像素。`clipped_pixel_count`统计可见超界位置，`valid_clipped_pixel_count`另外统计输出有效的截断位置，避免把原图6个超界都预言为有效70。

## 5. ProcessingRecord / PathCapability

ProcessingRecord包含schema_version=1、method(native_dbz/source_gray_dbz/local_gray_dbz/file_dbz)、input identity、decoder_version、quality_policy_version、裁剪/修补/resize/alpha步骤及alpha_bit_depth、质量统计、geometry/time来源和limitations。gray反算时encoding basis、range_policy(strict-v1/source-upper-clip-v1)、rule/config/wire适用版本、formula、quantization_step=0.3125及截断统计必填；native/file_dbz不伪填gray字段，不对原值施加0～70范围。file_dbz记录数值读取/选择和再编码步骤，并原样保留可用upstream processing/origin/adjustment；旧文件缺少历史记录时标unknown，不能编造零截断结论。

新增gray反算decoder=`gray-dbz-encoding-v1`，quality_policy=`gray-quality-v1`，pixel writer profile=`pixel-dbz-v1`。同形origin/adjustment须持久化；仅摘要或计数不能替代像素位置。历史规则本身不改变版本和摘要；这些新解释版本参与005processing identity。

PathCapability分别记录gray_evidence_status、dbz_encoding_status、native_science_status、geometry_status、live_status及原因。005计划中15条dbz_encoding_status=planned，8条blocked原因继续；不能因为gray passed自动标dbz verified或live verified。

## 6. 状态与成果身份

```mermaid
flowchart LR
  I[Source receipt / local declaration] --> V[Validate input and budgets]
  V --> N[Native direct / gray conversion]
  N --> D[Values + quality + processing]
  D --> S[Stage applicable outputs]
  S --> C[Verify receipts and cancellation fence]
  C --> M[Commit manifest last]
```

每步错误/取消均终止该item，batch保留其他已成功item；未提交不能标成功。gray Unavailable无法进入反算。各format独立能力检查，成功适用format不因另一不适用format丢失。

来源gray的logical_id/revision承接原始取得身份；local_gray由已校验声明与content digest计算；local_numeric由format/content digest/实际selection计算，revision为content digest。ProcessingSpec保留历史wire output_kind="decoded"等不变；新增gray→dbz的method/decoder/rule/quality/range策略、writer和相关grid/format/options进入processing_hash。file_dbz再保存由当前数值文件身份、实际选择/再编码与writer/options产生新output_id，不冒用upstream的取得身份或output_id。别名拼写、绝对文件路径/mtime不入hash。Manifest v1、raw cache和历史资料通过原接口及新identity入口共用事务。

更多格式/接口限制见 [persistence](contracts/persistence.md)、[gray-decoding](contracts/gray-decoding.md)、[CLI/SDK](contracts/cli-sdk.md)。
