# Contract: CLI / SDK modes and compatibility

**Version**: 新模式schema 1（设计）；CLI report envelope仍为v1。

## 1. CLI

### cat

```text
radiust cat SOURCE [existing source selectors] [--raw | --gray | --dbz]
radiust cat --file IMAGE [--raw | --gray | --dbz] [--frame-index N]
radiust cat --file SCIENCE [--variable NAME] [--dbz]
```

SOURCE与--file互斥。source/IMAGE缺省raw；科学文件缺省按原generic读取单位，不自动改标dbz。不同模式（含旧拼写归一后）在获取前拒绝；重复同一语义拼写可归一，不得触发重复解码。既有时间/最新帧/站点规则和renderer选择不变。

source --gray使用已验证显示规则，Unavailable可按既有预览回退raw并在mode_info明确actual=raw/reason。source --dbz先直接native reflectivity，再匹配passed gray；失败不能raw回退为成功。IMAGE --gray只显示已有灰度、不作物理声明，不取彩色亮度伪装编码；IMAGE --dbz明确声明gray-dbz-v1并严格校验。--frame-index只适用本地多帧图像，未明确选择时dbz拒绝多帧歧义；source使用receipt/规则已绑定帧。裸图无可信time/geo，报告unknown。

SCIENCE --dbz只读真实reflectivity/dBZ，不再次乘5/16；其他单位unit_mismatch。科学文件--gray不从着色/物理值生成历史gray编码，明确unsupported。不把终端截图/着色PNG自动当编码。

--legacy-display为隐藏兼容gray；--decoded保留历史generic语义：反射率路径对应dbz，新支持来源gray也可等价处理；合法旧非反射率科学调用保留原变量/单位，mode_info.actual=scientific，不能作无条件dbz alias。

### download / replay

```text
radiust download SOURCE [existing selectors/options] [--dbz] [--raw | --raw-only]
radiust download --file IMAGE --dbz --format netcdf|zarr|png --output ROOT
radiust replay RAW_MANIFEST [--dbz] --format png,netcdf,zarr [--output ROOT]
```

download SOURCE缺省保持原generic数值入口；新增--dbz启用反射率专用分派。--raw表示附带经过校验的原始资料，合法与--dbz共用；--raw-only与dbz数值请求冲突。该命令不新增--gray保存模式，gray是cat/SDK显示入口，原始raw和dbz数值正式保存沿用原格式。source/file二选一；local必须显式dbz，不能同时指定product/station/at/latest等来源选择参数或raw-only。local可用--frame-index选择多帧，声明进入provenance。几何相关选项按 [persistence](persistence.md)检查。

replay读取真实raw-manifest、receipt与绑定信息，--dbz使用同一Core分派，默认原native/generic行为保持。各格式结果单独记录；无几何GeoTIFF失败，其余适用格式可成功，不能将格式失败删除。dry-run/模式冲突/已知能力错误尽量获取前拒绝；输入实际约束与可信geometry在取资料后检查。

## 2. SDK

以下新增Client方法，AsyncClient同名协程、语义与返回值一致：

| Method | Result / contract |
| --- | --- |
| fetch_gray(query_or_ref) | GrayDecision/GrayFrame；选择/获取/转换均Core，带实际mode和质量记录 |
| decode_gray(raw) | 相同GrayDecision，不重复获取 |
| fetch_dbz(query_or_ref) | RasterResult，native direct或passed gray结果 |
| decode_dbz(raw) | RasterResult，保留取得identity/receipt |
| decode_gray_file(path, frame_index=None) | RasterResult，PixelDbzField，明确本地编码声明；不用FrameRef |
| replay_raw_manifest(path, *, mode=None) | None保持旧科学分派/原返回类型；dbz返回RasterResult（passed gray或native reflectivity），其他mode拒绝；复用raw receipt，不网络重取 |
| read_dbz(path, variable=None) | RasterResult，真实reflectivity/dBZ校验，Pixel/native读回；当前input=NumericFile，附文件content/selection身份及经校验receipt，原处理信息作upstream provenance |
| write(value, output=..., format=..., ref=None, ...) | Source/Local/NumericFile结果携带经校验receipt可省ref；read_dbz后可直接再保存；显式ref冲突拒绝，旧无绑定身份field仍要求原ref规则 |

Convenience函数新增fetch_gray/afetch_gray、fetch_dbz/afetch_dbz、decode_gray_file/adecode_gray_file，保持包root lazy导出。旧fetch/afetch/decode仍generic，不强制变量/单位。

fetch_many/iter_fetch、异步对应入口及download增加keyword `mode=None|gray|dbz`（download仅None/dbz，raw-only按旧参数）；不改已有位置参数。默认None保持旧语义；mode=dbz/gray调用同一Rust批量/stream调度和共享限制，不在Python循环模拟并发。FrameResult/BatchResult保留既有状态/error/data，新增mode_info；partial成功/空/失败逐项保留，on_error语义不变。

SDK replay的keyword-only mode贯通Client/AsyncClient、bridge、PyO3和Core；None保持原API，dbz共用decode_dbz分派。raw schema/SHA/路径/TW-RDCAP binding错误保留完整性分类，解码/单位/取消错误保留结构化stage/code，不全部包装成IntegrityError。旧扩展无mode参数时仅None可直接调用原方法；dbz按已定义的direct能力检查提供受限兼容，缺gray能力明确失败，已有方法失败不fallback。

RasterResult.data为Pixel或native shared backing，另含input/processing/mode_info；Pixel bound对象暴露shape、values/quality/origin/adjustment及原alpha/alpha_bit_depth显式转换接口，不假装原RadarField。`to_xarray(PixelDbzField或其RasterResult)` 产出reflectivity DataArray(row,column)，quality/origin_quality/encoding_adjustment/适用原alpha为同形坐标、编码信息在attrs，alpha保持uint8/uint16和位深，只有真实已知time才附带。native包装委托原DataArray/Dataset转换。导出显式复制计入预算，普通取资料不强依赖NumPy/xarray。

旧科学文件没有FrameRef/gray编码记录也可read_dbz，但必须有真实变量/单位及原格式要求的time/geometry；不补gray声明，不乘5/16或限制到0～70。再保存以当前NumericFile身份、实际读取/再编码处理生成新output_id；历史processing/adjustment可用时原样保留在upstream，不伪造来源取得receipt。

`GrayDbzDecoder`（规范）薄委托Core严格数组入口，decode返回(values float32, quality uint16)以便旧数组使用者迁移；不能截断小数。`LegacyGrayDbzDecoder(strict=True,max_gray=224)`旧类名及原构造参数、tuple返回、ValueError/UnknownColorError分类保持。所有合法历史输入（包括原允许的浮点窄化、strict=False未知mask、max_gray≤255）由独立Core historical profile承接；类文档明示兼容语义。满足规范整数条件的默认旧调用与新入口算术等价，其他历史配置/窄化不能标为canonical或用于--dbz严格本地工作流。历史profile不更改规范range/quality合同。wheel只纳入gray_dbz.py与最小lazy __init，不恢复旧Python业务实现。

跨新Python facade/旧扩展，只有新方法不存在才可对三类直接native reflectivity调用旧decode_science并核对单位；灰度/本地能力报告扩展版本不足。存在方法而失败时不fallback；资源/取消/无效输入不允许被旧分支吞掉。

## 3. JSON report additive schema

保持既有Envelope.schema_version=1、command/status/counts/items/result/error类型。新增 `mode_schema_version=1` 和 `mode_info`。cat添加在envelope层，避免改变旧字符串result；download/replay/batch逐item增加，跨不同模式的聚合不造单一actual。

实施分期：US1完成本地IMAGE三模式及上述cat报告，供quickstart第2节独立验收；US2扩展source/generic/旧参数及逐item报告；US3完成来源与SDK replay读取；US4完成新数值文件读回/保存及CLI replay成果提交。

```json
{
  "mode_schema_version": 1,
  "mode_info": {
    "requested": "dbz",
    "actual": "dbz",
    "variable": "reflectivity",
    "units": "dBZ",
    "method": "source_gray_dbz",
    "encoding": "gray-dbz-v1",
    "encoding_wire_version": "历史原值",
    "rule_version": "匹配的原版本",
    "config_hash": "匹配的原摘要",
    "range_policy": "source-upper-clip-v1",
    "clipped_pixel_count": 6,
    "valid_clipped_pixel_count": 0,
    "geolocation": "unknown",
    "time_status": "known",
    "limitations": ["quantized_encoding", "upper_clipped"]
  }
}
```

上例仅字段形状演示，valid_clipped_pixel_count=0不是NZ验收结论；实际值由quality计算。actual=raw/gray/dbz/scientific；完全失败为null并带reason/error。native encoding/rule/clip字段null或0，不伪填gray版本。gray units=gray_code；raw变量单位null；generic科学保留真实单位。geolocation=known/unknown，time_status=known/unknown。

JSON不用携带整张像素数组，像素截断位置通过SDK及数值文件恢复；结果/manifest提供artifact paths。历史display_mode等deprecated字段维持v1类型和旧可读值，canonical读mode_info；不能看到旧decoded字符串就推定dBZ。迁移提示只在人类stderr，不污染JSON/pipe；状态、退出码、quiet/verbose及取消仍按现有合同。

## 4. 活动与历史名称

| Current | Canonical activity | Compatibility boundary |
| --- | --- | --- |
| --legacy-display | --gray | 隐藏alias、人类迁移说明 |
| --decoded reflectivity | --dbz | historical generic保持真实单位 |
| legacy_display.rs / LegacyDisplayPreview | gray.rs / GrayPreview | 旧模块与类型reexport/wrapper |
| resources/legacy_display | resources/gray | 旧引用映射和原hash codec，旧资源内容可核对 |
| compare_legacy_display验证入口 | compare_gray | 旧wrapper、历史报告原样可读 |
| 灰度相关legacy docs/errors | gray | 历史spec/evidence及通用旧格式兼容不批量替换 |

结构化错误以radiust_code/radiust_stage为准。原型API、旧cache/source locator、generic science对象不按词面重命名。README/docs/SDK使用新命名，所有保留旧名标compatibility/historical。
