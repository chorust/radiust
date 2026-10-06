# Contract: gray encoding → dBZ

**Version**: 1（设计） | **Related**: [data model](../data-model.md)、[research](../research.md)

## 1. 授权依据与严格输入

接受已匹配passed来源规则生成的GrayFrame，或用户显式声明采用gray-dbz-v1的本地整数灰度资料。0～224与文件名/灰度外观不是自动识别依据。PNG/GIF/WebP按现有Rust image能力有界读取；保留原整数位深校验，不先to_rgba8/uint8截断。SDK整数/数值数组入口（GrayDbzDecoder.decode）同样先校验原值有限、整数、shape和通道。

可见RGB必须R=G=B，单通道直接作为gray；L/LA/RGB/RGBA承接alpha。alpha0直接missing，隐藏RGB越界不导致错误；alpha非零反算不乘alpha。8/16位整数输入以原整数校验0～224，不按16位满量程缩放。非整数、非有限、可见非灰度、损坏/不支持、维数不匹配均明确失败；不取亮度。来源规则允许其已有明确亮度生成步骤，但最终gray仍须三个通道相等。

透明性先按原位深判断，alpha16=1不能因转uint8成为missing。可用alpha以AlphaPlane原uint8/uint16及alpha_bit_depth保存/导出；无alpha表示opaque。规范数组alpha只接受同形uint8/uint16，位深由原dtype确定，读取值须在该位深范围内；其他dtype、负值/小数/超界明确失败，不猜测浮点归一化，也不先cast alpha再校验。8位预览采用[data model](../data-model.md)的非零保持映射，只影响显示；历史兼容profile仍沿用其原行为。边界验收包括alpha16的0、1、255、256、65535。

本地单帧直接读取。多帧GIF等须显式frame_index，默认拒绝歧义；来源用被验证规则/receipt绑定的frame_index。已知文件标注为raw彩色/dBZ着色预览或科学成果时，不把它当编码图反算；数值文件使用其真实变量/单位直接读取。

## 2. 数值、范围策略与质量

| Basis | Range policy | Effective gray |
| --- | --- | --- |
| local user declaration | strict-v1 | 0～224；任一可见越界整帧拒绝 |
| matched passed source rule | source-upper-clip-v1 | 可见合法整数灰度超过224，仅数值反算取224；原gray字节保留 |
| existing native reflectivity | native decoder policy | 不经过本编码；保持其原值范围/精度 |
| persisted numeric reflectivity | persisted processing record | 不再次gray反算；保留原记录 |

公式 `dBZ = effective_gray * 5/16`，f32；所有225个合法整数对应值在二进制f32可精确表示，验收容许绝对误差≤1e-6。native数值不受0～70范围限制。来源截断不清除任何既有无效mask；有效结果≤70，其余NaN。

截断记录 `encoding_adjustment[row,column] & 1` 及可见/有效计数，原gray本身仍可为229。范围策略是编码basis决定的固定策略，不提供任意本地clamp开关。严格局部输入不能因为文件路径含nz而自动套来源例外。

alpha0、背景/站外/未知色等已知无效原因按 [data model](../data-model.md)保存。Opaque0没有无效证据时为0 dBZ；below_detection/无雨不能靠0猜测。修补仅在有实际有效支持且可恢复原因时标8，origin保留原因；zero_invalid不标recovered。resize质量跟同一非零权重支持域，不插值quality整数。

## 3. 原生Core职责

- `apply_gray_for_source(raw, resolved_context, limits)` 返回GrayDecision，匹配source/product/path/station及输入约束并保留原字节和伴随质量。
- `decode_dbz(raw)`：先保留三类native直接reflectivity分派，其余仅匹配passed gray；不支持或applied=false明确失败。
- `decode_gray_file(path, frame_index=None)`：有界本地读取、digest、显式编码声明，返回PixelDbzField/RasterResult，不获取网络资料。
- `decode_gray_values(values, alpha=None, declared_encoding=gray-dbz-v1)`：固定编码严格校验的原生数组入口，供薄GrayDbzDecoder及小型离线验收，不实现另一套Python算术。

CPU转换共用decode许可与请求deadline。原始identity/receipt随结果承接，不重新取得同帧或由数组伪造raw revision。模型与源码具体组织见 [plan](../plan.md)。

## 4. 错误与限制

失败遵守既有ErrorCode/ErrorStage；新增 `invalid_gray_encoding` / `unit_mismatch` 使用结构化码，并在CLI/PyO3映射。输入范围/通道错误位于decode阶段，包含安全row/column/value/期望范围（不输出秘密locator）；选帧歧义/模式冲突validate；blocked/stale证据decode_unverified；未知能力unsupported；缺几何invalid_grid；超限resource_limit；取消保持cancelled。灰度回退只能报告actual=raw及reason，不能报告dbz成功。

provenance注明量化、clip、alpha衰减是否已参与gray、mask是否丢失、修补/resize、时间/几何已知程度。没有证据就标unknown，不能把user declaration标独立物理校准、没有pixel mask还声称已恢复背景。FR/PT仅支持旧亮度编码对应dBZ，真实其他物理量不改单位。
