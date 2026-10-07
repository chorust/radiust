# RDCAP 科学值、显示与持久化合同

## 解码与数值

输入为经确认同country/station/key绑定的原始JSON string artifact。decoder先读有界UTF-8 envelope，再验证CSR头/数组，输出reflectivity/dBZ、[H,W]北→南、f32值/u16质量及EPSG:4326。原始default/invalid=-999为NaN+missing；有效值raw×0.1；弱/负值完整保留。

9999为rdcap-annotation-v1实测标记候选，适用协议/头/transform/legend须与本次验证组合一致。将其排除为NaN、quality=65（bit0|新增bit6 source_annotation），留原始内容及empirical_inference说明。既有质量bit0–5不改，writer的flag_masks/meanings增加64/source_annotation。unknown_color、outside_coverage不能代替此角色。未验证标记模式/科学含义明确失败，原始合法内容可保留。

结构非法为invalid_grid，未知CRS/registration/transform/legend为decode_unverified；全缺测或仅annotation的合法结构仍能形成全NaN场，不伪造零或无雨。先checked arithmetic/limits，再dense分配，计入原始文本、token/CSR、values、quality及临时RGBA缓冲峰值。所有CPU任务持有实际生命周期的decode permit，取消后结果禁止提交。

## 几何与读回

采用[data-model](../data-model.md)的当前帧anchor/affine/中心坐标；左上角T注册、原始南→北行序，翻转数据及quality同时完成。native不能偷偷变为EPSG:3857，网页显示投影不作为源CRS。坐标误差≤原生格距×1e-6；有效数值误差≤0.01dBZ。explicit regrid延续quality/NaN和线性反射率域插值规则。

| 成果 | 必须保留/验证 |
|---|---|
| NetCDF | reflectivity float32/dBZ、quality uint16及flag含义、时间、中心x/y、CRS、affine、provenance，独立h5netcdf/xarray读回 |
| GeoTIFF | float科学波段、NaN/nodata、EPSG:4326/仿射；沿用质量与provenance配套文件，rasterio独立读回，不能将quality混入科学值 |
| Zarr v2 | 数值/quality/坐标/单位/时间/CRS/provenance，Python zarr/xarray独立读回 |
| PNG | 科学渲染RGBA、identity/geometry/palette/透明规则sidecar；PNG是显示成果，不承担可逆连续科学数值保存 |

## RDCAP 显示资源

palette ID=rdcap-reflectivity-v1，15个lower-inclusive档，阈值5、10、…、75dBZ。对应颜色：

```text
#635273 #736384 #9c9c9c #00ce00 #00ad00
#009400 #ffff00 #e7c600 #ff9400 #ff6363
#ff0000 #ce0000 #ff00ff #9c31ce #ffffff
```

≥75取最后档，<5透明但科学值仍有效；缺测/annotation透明。依据已验证rdcap decoder provenance/资源版本选择来源默认palette，科学preview和PNG writer使用同一render函数；其他source的default渐变不变。sidecar记录palette/rule/decoder版本、阈值、单位、完整frame身份与当前几何。当前通用PNG仅六色梯度，新增离散选择必须真正接入writer/preview。

既有--palette/vmin/vmax若显式提供，遵守其既有自定义显示合同并记录实际选择；缺省RDCAP结果必须符合上述基准。不得把用于网页对照的marker恢复到默认科学预览。

## raw与manifest

原始artifact为file-response.json：单次GET返回payload字节未经JSON解析重编码；transfer解压后的HTTP实体作为原始文件，非抓包级TLS/压缩流。首读成功即落盘，后续decode/preview只读该文件，不重复读取ticket。

binding.json为确定性派生记录，包含country/station_code/key/UTC、file_sha256、provider_origin和协议版本；明确是工具生成的绑定，而非提供者原始metadata。原始响应具有内嵌头，无需额外下载header。原始manifest v1的ref包含safe locator，公开文件不含ft/cookies/完整ticketURL。

读取manifest必须经过既有schema/raw_complete、safe path、symlink、size/sha256验证。离线decoder只用artifact、safe frame与binding，无公网、ticket或Orca依赖。浏览器CSV重建的测试envelope标为reconstructed，不当作生产原始响应证据。

## 身份、cache与提交

logical ID沿用source/product/完整station/实际UTC/safe locator/locator_version；刷新ticket不改变ID。revision由内容摘要而非JWT决定；同key内容改变不能静默复用旧revision。decoder_version=rdcap-csr-v1、resource/palette/annotation version进入ProcessingSpec，规则变化提升版本；原有资源/encoder版本仍参与output identity。

cache只复用摘要验证完整raw；并发获取同logical frame应coalesce保存后的结果，不能两个消费者各读取同ticket。正式raw不是可回收cache；已验证完整输出重复请求skipped。--raw补充只能相同revision，不能把新内容补入旧科学成果。

RDCAP模板{station}直接以无斜杠公开站点ID（如TWRCHL）作为单个安全分量输出。默认hash目录、root containment、manifest-last、overwrite、锁、取消fence与未知远端提交结果规则不变。native输出不依赖Python writer。

## 验收证据分层

1. **内容/离线science**：三国CSV及重建envelope、8参考点、几何、弱/负值、annotation、资源/异常合同；已有研究结果不能标为adapter通过。
2. **工具在线raw**：标准Engine/CLI或SDK，全程正常TLS、无手工ticket，三国各当前有资料站，记录日期/identity/原始响应摘要及自动重试统计。
3. **在线端到端**：上述实际取得文件完成science与至少一个科学格式独立读回；四格式完整矩阵可同三国帧离线重放，注明出处。

任一国家缺第2/3层，在线支持为未验收，不用offline通过或skip补齐。记录首读成功/同ticket二读空是研究依据，正常生产获取不得以二读再验证来消耗ticket。
