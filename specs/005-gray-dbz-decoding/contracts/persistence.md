# Contract: Numeric persistence and identity

**Version**: pixel-dbz-v1（设计）；旧Manifest v1与地理/原始格式保持可读。

## 1. 适用输出

| Result | PNG | NetCDF | Zarr v2 | GeoTIFF / geographic regrid |
| --- | --- | --- | --- | --- |
| Pixel dBZ, no geometry | 标dBZ的预览、无CRS | 行列数值profile | 行列数值profile | 明确拒绝 |
| dBZ with complete trusted geometry | 既有地理语义 | 既有坐标+伴随质量 | 既有坐标+伴随质量 | 既有支持范围及质量合同 |
| Native generic other units | 保留原单位 | 原native profile | 原native profile | 原适用契约；不能改标dBZ |

PNG是着色/显示成果，不是可独立恢复值的数值容器；JSON/元数据明确mode=dbz、units=dBZ、geolocation。gray预览使用原gray字节；其文件保留旧范围和编码标识，不因数值截断被改写。

## 2. Pixel NetCDF/Zarr profile

固定dimensions `row`, `column`（height,width），零基整型坐标；首行在上、column向右。不存在geometry时不写CRS/grid_mapping/lat/lon，也不对row/column标degrees/metres。`time`仅valid_time已知时写真实时间；未知时变量缺省，`time_status=unknown`，不写NaT/epoch冒充观测。

| Variable | Type | Contract |
| --- | --- | --- |
| reflectivity(row,column) | float32 | units=dBZ，NaN fill；不要对gray再做scale_factor |
| quality(row,column) | uint16 | 同形bits/flag_masks/flag_meanings；fill不得混同合法quality0 |
| origin_quality(row,column) | optional uint16 | 存在可证明修补前/贡献原因时保存并标语义；缺省表示没有像素级历史 |
| encoding_adjustment(row,column) | uint8 | 同形，bit1=upper_clipped；all-zero可压缩但readback仍正确 |
| alpha(row,column) | optional uint8 / uint16 | 无损保留可准确映射的透明度，alpha_bit_depth=8/16与dtype一致；注明输入/输出关系，不能伪称resize前alpha。无alpha表示opaque，不要求合成数组 |

global attrs：`radiust_raster_schema_version=1`、`raster_profile=pixel-dbz-v1`、coordinate_space=pixel、orientation、time_status、geometry_status、variable/units、input_identity JSON、processing_record JSON。这些JSON格式版本显式为1，记录编码/规则/decoder/quality/range策略、步骤、限制和截断统计。xarray/独立reader无需本SDK即可读取全部像素值与伴随数组。

Zarr沿用v2、现有压缩/chunk/budget策略，变量 `_ARRAY_DIMENSIONS` 与shape一致；坐标0和quality0不能被fill masking吞掉。NetCDF沿用静态netcdf库和正确native整数类型。writer在分配/编码前校验shape、overflow、size、mask/value一致及目标预算，不按dimension<=max_pixels推定足够内存。

新增 `read_raster_result(path, variable=None)` 识别Pixel profile与原native科学文件；缺time的Pixel合法。旧 `read_selected_field` 的RadarField严格合同保留。多变量无明确选择仍报歧义；--dbz只接受reflectivity+dBZ，可识别既有合法变量别名依据，但不能对rain_rate换单位。保存时已截断的数值读回不再截断，processing/adjustment原样保留。

每次读取以实际数值文件建立`RasterInput::NumericFile`与NumericReadReceipt，适用于NetCDF、Zarr及既有可读GeoTIFF。自包含NetCDF单文件以实际字节SHA为content digest；Zarr以全部metadata/chunk文件清单（相对key排序、每项SHA/size）产生规范摘要；既有GeoTIFF reader实际读取的数据TIFF、quality TIFF与provenance JSON按固定角色组成SHA/size清单摘要，不能只哈希主TIFF。有界安全读取，拒绝不安全路径/符号链接及多文件读取期间变化。format/变量/实际field或time选择参与域隔离身份，根目录绝对路径/mtime不参与。文件中原input/processing保留为upstream_provenance，不当作取得receipt；旧native文件没有FrameRef或历史编码记录也可按真实单位读取，不伪造gray声明，不应用0～70限制。原生reader原本要求的实际time/geometry条件继续有效。

Pixel读回原alpha dtype/值及alpha_bit_depth，先按原alpha判断有效性；原quality/origin/adjustment和历史processing保持。uint16的0/1/255/256/65535须独立读回一致，显示派生uint8不能替代持久化alpha。旧native没有这些历史字段时明确unknown/缺省，不宣称已知无截断或已恢复mask。

## 3. 事务提交

新增校验后的 `RasterCommitIdentity/receipt` 入口，旧FrameRef入口与新local_gray/local_numeric入口汇入同一LocalStore事务：stage→size/SHA核验→锁/幂等与overwrite→取消fence→manifest最后提交。失败/取消清理staging，半文件/孤儿不能当成功；已成功item在其他item失败时保留。

来源gray结果logical_id与revision来自原始取得FrameRef/resolved revision；不能用derived values的science_revision替代。local_gray logical_id为 [data model](../data-model.md)域隔离digest，revision为原图content SHA；没有FrameRef，裸图valid_time/geometry为null。默认raw_complete=false；若来源显式附带完整raw，沿用既有raw-completeness校验。本地原图不会被改写，created_at仅成果创建时间。

read_dbz得到的NumericFile结果携带经校验的文件身份/receipt，可不提供ref再次write；新成果采用当前文件content digest/selection、method=file_dbz及实际再编码writer/options派生身份，upstream output_id仅作历史引用。真实time/geometry仅承接文件中的有效证据；默认raw_complete=false，嵌入原来源信息不证明raw完整。显式ref与附带输入身份冲突时拒绝，不能覆盖为伪来源；旧未附带可信身份的单独RadarField仍遵循原ref要求。新增文件读取/再保存不会改变旧native来源调用纯改名的身份。

processing_hash包括新增method/decoder、rule/config、canonical encoding及历史wire version、range/quality策略、pixel/geo writer profile、真实grid/format/options/input declaration。raw/gray/dbz成果可区分；旧ProcessingSpec.output_kind="decoded"及历史versions继续原wire语义。相同旧native调用仅改入口名称时不改变身份；新增gray→dbz与旧显示成果不共享output_id。

Manifest v1继续保存既有字段与artifact receipts，新处理信息放ProcessingSpec.options；skip须验证完整性和processing hash，不能只看文件存在。资源别名/CLI旧拼写、路径/mtime不入新算术身份；切换strict/source-upper-clip、quality或writer profile一定改变新处理身份。

## 4. 输出失败与历史读取

--format geotiff / grid geographic / bbox / resolution在geometry不足时明确invalid_grid；格式列表逐项处理，适用成功项仍提交并保留，不以raw或PNG替代被拒绝格式。默认既有格式行为保持，指南对像素场显式选择netcdf/zarr以避开地理要求。

历史raw-manifest加载仍核对schema/FrameRef/time/artifact SHA/路径安全、TW/RDCAP binding，不能为了barefile修改其合同。历史规则JSON字段/版本与manifest/cache保留原字节语义；新活动gray资源通过兼容读取层访问，不篡改旧证据。
