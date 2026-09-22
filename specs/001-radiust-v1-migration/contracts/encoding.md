# Scientific Encoding Contract v1

输入必须先通过 `RadarField/RadarDataset.validate()`，数据与空间定义见 [data-model.md](../data-model.md)。所有 encoder 仅写 staging，最终发布由 [storage](storage.md) 负责。format、encoder/resource 版本和参数参与 processing identity。

## 能力矩阵

| 格式 | 变量/网格 | 文件组 | 默认/依赖 |
| --- | --- | --- | --- |
| NetCDF4 | 单/多变量；原生 geographic/cartesian/polar，必要时 curvilinear | 主 nc，外部 output manifest | 默认；xarray+h5netcdf+h5py 核心 |
| GeoTIFF | 单变量，规则 affine 栅格 | data.tif、quality.tif、provenance.json | geotiff extra / rasterio 1.4 线 |
| PNG | 单变量，规则二维栅格渲染 | image.png、render.json（含 palette/range/georeferencing/provenance） | 核心 Pillow/NumPy |
| Zarr v2 | 单/多变量，各原生网格 | 每帧独立 store，清单列全部文件 | zarr extra；不共享时间 append |

多变量导出 GeoTIFF/PNG 必须 variable；不支持的网格只能经用户显式 regrid，不可默默改变数据。缺依赖在原始资料下载前失败，提供对应 extra 安装指令。raw-only 不初始化 encoder。

## NetCDF 与 Zarr 元数据

- 连续量 float32，缺测 NaN；类别整数与独立 nodata code；quality uint16 精确保留所有位，`_FillValue=None`，0 不变成缺测。
- 数据变量附 units、适用时 standard_name、ancillary_variables；质量附同 dtype flag_masks 与 flag_meanings。无对应标准名就省略，不编造。
- 时间保存 UTC 有效时间；forecast_reference_time 与 lead time 分开，retrieved_at 在来源信息。固定可往返的 epoch/time units 与 calendar，避免依环境时区变化。
- 坐标为中心坐标，CRS scalar grid mapping + CRS WKT，投影属性按 CF-1.8 映射。完整 ProcessingRecord 以严格 JSON 文本存储，读取恢复并验证；不把任意 Python 对象塞 attrs。
- Polar 保留 azimuth/range、站点高度/位置、elevation、beam_model，必要时补辅助地理坐标；未验证 CF 表达的扩展用明确 radiust 命名空间，不夸称所有极坐标元数据为标准 CF。
- NetCDF 显式 `engine=h5netcdf, format=NETCDF4, invalid_netcdf=False`，不自动 fallback；HDF5 写通道串行，文件句柄关闭后才进入提交。
- Zarr 显式 v2，二维 chunk 默认逐轴 min(512, axis_length)，data/quality 共布局；时间是单帧 scalar，不 append。先完成元数据与全部 chunk，再提交 manifest。全缺测 chunk 是否省略由 encoder 参数固定，清单按实际文件记录。

## GeoTIFF

CRS、shape、affine 与像元中心一致，控制点验证半像元偏移和 Y 方向；主文件 nodata 与模型一致。quality.tif 同 CRS/transform/shape，保留 uint16 多位信息。二值 mask 不能替代 quality。provenance.json 保存帧/修订、变量/单位、起报/有效时间、转换历史和质量定义；三者全部写入成功才可发布清单。

## 渲染/PNG

Pillow/NumPy 共享 render pipeline，固定版本化 palette/range；类别离散。输出 RGBA、标题、legend 与显示属性，区分无雨/缺测/站外。科学数据不得因缩放或配色变化而改变。`render.json` 保存 palette 版本、vmin/vmax、像素尺寸、范围、CRS、原始与展示方向及必要坐标/transform，不能仅凭 bbox 假定不存在的精确定位。

RGBA tile acquisition 在 Rust 只做无损 palette lookup、拼接和裁剪，不复用科学图像渲染的缩放操作。indexed PNG 的 index 不跨不同 palette 直接拼接。终端缩放可发生于副本，不能回写 acquisition cache 或 Field。

## Round-trip 验收

- NetCDF/Zarr：逐变量数值在预先设定容差内；quality/类别精确相等；坐标、时间、CRS、units、provenance、维度顺序一致；无效与零值不混淆。
- NetCDF 除项目自身读取，还用独立 netCDF4 reader/CF checker 验证代表文件，避免 writer/reader 共享错误自证；测试依赖不进入核心安装。
- GeoTIFF：主值、quality、CRS、affine 和参考控制点验证；sidecar 缺失必须被完整性校验拒绝。
- PNG：不是科学数值 round-trip；验证固定图像/色阶/方向/透明和配色侧车，并验证 render 前后科学数组不变。
- 全缺测、无雨、无回波、未知颜色、多变量、极坐标与不同预报起报时间都有独立 fixture。
