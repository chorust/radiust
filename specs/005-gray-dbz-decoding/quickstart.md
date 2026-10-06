# Quickstart: 实施后的离线验证

**Status**: 部分实施和验收。US1/T021已运行并通过第2节本地gray/dbz场景，证据见 `validation-results/gray-dbz-local.json`；第3–6节仍是后续验收指南，未标通过。验收标准见 [spec](spec.md)，接口见 [CLI/SDK](contracts/cli-sdk.md) 和 [persistence](contracts/persistence.md)。

## 1. 准备

在仓库根目录，准备Rust1.92及项目锁定Python开发环境；科学读回使用已有science/zarr extras。本地验收不需开放网络，所有临时成果放/tmp，不修改历史fixtures。

```sh
uv sync --group dev --extra science --extra zarr
uv run maturin develop
cargo build --locked -p radiust-cli
mkdir -p /tmp/radiust-gray-dbz-validation
```

依赖尚未缓存时仅环境安装需要下载；产品验证仍离线。正式CLI用 `target/debug/radiust`，wheel验证另外安装构建wheel到干净环境，确保没有仓库PYTHONPATH替代wheel。

## 2. 全225码、透明和有效黑色

本节作为US1/T021验收，前置为T001–T020；本地--raw/--gray/--dbz与mode_schema_version=1/mode_info均在T020完成，不等待US2或数值文件保存。

以下只生成小型输入fixture：第一行覆盖0～224，第二行含有效黑色和透明隐藏超界值。

```sh
uv run python - <<'PY'
from PIL import Image
from pathlib import Path
root = Path('/tmp/radiust-gray-dbz-validation')
row = [(g, g, g, 255) for g in range(225)]
image = Image.new('RGBA', (225, 2))
image.putdata(row + [(0, 0, 0, 255)] + [(255, 255, 255, 0)] * 224)
image.save(root / 'declared-gray.png')
Image.new('RGB', (1, 1), (225, 225, 225)).save(root / 'out-of-range.png')
Image.new('RGB', (1, 1), (16, 17, 16)).save(root / 'not-gray.png')
PY
target/debug/radiust cat --file /tmp/radiust-gray-dbz-validation/declared-gray.png --raw --renderer text --json
target/debug/radiust cat --file /tmp/radiust-gray-dbz-validation/declared-gray.png --gray --renderer text --json
target/debug/radiust cat --file /tmp/radiust-gray-dbz-validation/declared-gray.png --dbz --renderer text --json
```

检查：默认/--raw不是科学声明；--gray显示原码；--dbz units=dBZ、local_gray_dbz/strict-v1，time/geolocation=unknown；第一行0/16/32/160/224对应0/5/10/50/70。第二行第0列有效0，其余NaN+missing（隐藏255不致错）。

## 3. 同步/异步与像素保存

完整执行本节须完成US4；单独同步/异步decode_gray_file读取可在US1验收。

```sh
target/debug/radiust download --file /tmp/radiust-gray-dbz-validation/declared-gray.png --dbz --format netcdf --output /tmp/radiust-gray-dbz-validation/nc --json
target/debug/radiust download --file /tmp/radiust-gray-dbz-validation/declared-gray.png --dbz --format zarr --output /tmp/radiust-gray-dbz-validation/zarr --json
```

正式文件地址取对应报告manifest/artifact路径。SDK显式解码无需FrameRef：

```python
import asyncio
import numpy as np
from radiust import Client, AsyncClient, to_xarray

path = '/tmp/radiust-gray-dbz-validation/declared-gray.png'
with Client() as client:
    sync = to_xarray(client.decode_gray_file(path))

async def read_async():
    async with AsyncClient() as client:
        return to_xarray(await client.decode_gray_file(path))

async_result = asyncio.run(read_async())
np.testing.assert_allclose(sync.values, async_result.values, atol=1e-6, rtol=0, equal_nan=True)
np.testing.assert_array_equal(sync.coords['quality'], async_result.coords['quality'])
```

另用xarray独立读NetCDF/Zarr，而非本SDK reader：

```python
import numpy as np
import xarray as xr

# 将nc_path / zarr_path设为上一步报告返回的实际artifact路径。
nc = xr.open_dataset(nc_path, engine='h5netcdf')
zs = xr.open_zarr(zarr_path)
expected = np.arange(225, dtype=np.float32) * (5 / 16)
np.testing.assert_allclose(nc['reflectivity'].isel(row=0), expected, atol=1e-6, rtol=0)
np.testing.assert_allclose(zs['reflectivity'], nc['reflectivity'], atol=1e-6, rtol=0, equal_nan=True)
np.testing.assert_array_equal(zs['quality'], nc['quality'])
np.testing.assert_array_equal(zs['encoding_adjustment'], nc['encoding_adjustment'])
assert 'time' not in nc and 'time' not in zs
assert not any(v in nc for v in ('crs', 'latitude', 'longitude'))
```

检查row/column含合法0、quality0可读、透明mask、编码声明/content digest、orientation、step=0.3125及limitations。再用client.read_dbz或cat --file FILE --dbz读回；数值不能再次乘5/16。不同绝对输入路径但同字节/声明应同身份，重复同格式skip且核对manifest完整性。

read_dbz的当前input应为local_numeric/NumericFile，原local_gray或来源input/processing保留为upstream provenance。用不含原FrameRef的旧reflectivity/dBZ文件补充验证：直接读真实数值（允许native范围低于0或高于70），随后client.write(result, ...)无需ref；再保存身份来自当前文件digest/selection及实际再编码，raw_complete=false，不冒用历史output_id。显式冲突ref须拒绝；旧未绑定身份field继续要求ref。同内容Zarr搬到另一根目录身份相同，修改任何metadata/chunk或变量选择则身份改变；旧GeoTIFF读取的quality/provenance sidecar也必须计入身份，修改任一组件不能复用旧身份。

## 4. 严格拒绝与模式冲突

```sh
target/debug/radiust cat --file /tmp/radiust-gray-dbz-validation/out-of-range.png --dbz --json
target/debug/radiust cat --file /tmp/radiust-gray-dbz-validation/not-gray.png --dbz --json
target/debug/radiust cat --file /tmp/radiust-gray-dbz-validation/declared-gray.png --gray --dbz --json
target/debug/radiust download --file /tmp/radiust-gray-dbz-validation/declared-gray.png --dbz --format geotiff --output /tmp/radiust-gray-dbz-validation/rejected --json
```

前三者分别为invalid_gray_encoding、invalid_gray_encoding、validate模式冲突；最后invalid_grid，不能生成成功manifest。前三项在US1验收；最后保存/地理操作及新Pixel数值文件读回在US4验收。补充数组非整数/NaN/超界、16-bit PNG不被窄化、损坏、多帧及非reflectivity科学文件unit_mismatch案例。alpha16的0/1/255/256/65535按原值判断：只有0因透明missing，其余不乘dBZ；SDK/xarray/NetCDF/Zarr保留uint16、原值和alpha_bit_depth=16。预览派生uint8非零保持，不作为数值mask。所有错误核对stage/code，不按人类文本匹配。

## 5. 全15路径与NZ截断

复用 [现有golden列表](../../crates/radiust-core/tests/legacy_display_parity.rs) 的15条绑定，固定source/product/path/station/frame_index及输入约束。通过实施时新增的offline source-context验收入口运行；不从裸old-gray文件伪造来源身份或借文件名启用clamp。计划的Rust验收target可命名gray_dbz_paths，具体由tasks明确：

```sh
cargo test --locked -p radiust-core --test gray_dbz_paths
```

每条核对gray与原基准逐像素相同，数值有效点=`min(gray,224)*5/16`，quality/origin/adjustment可读回，limitations明确；报告单列gray/dbz/geometry/live状态。MY east用归档568×640基准，TH kkn绑定frame0/补边，ES zero_invalid不能标recovered，FR/PT注明亮度×alpha已参与旧生成。

NZ必须核对 [research](research.md#r1--编码范围与nz来源例外) 的6个(row,column,value)，clipped_pixel_count=6且原gray值仍225～229；这些位置若quality有效输出70，否则NaN并保留原无效原因，valid_clipped_pixel_count由实际mask计算。反算不把所有6点强行变有效。把NZ old-gray裸文件作为local --dbz应严格失败，体现来源证据与本地声明不同。

8条blocked仍逐条失败/不可用，不因新增公式启用；RV/TW旧显示blocked不影响native直接dbz。来源在线示例（另需显式网络授权与实际可用时刻）为：

```sh
radiust cat nz --product rain --latest --raw
radiust cat nz --product rain --latest --gray
radiust cat nz --product rain --latest --dbz
radiust download nz --product rain --latest --dbz --format netcdf --output ./data
```

这些在线命令仅示范入口，本阶段不执行、不据此升级live状态。离线已有raw-manifest可 `radiust replay RAW_MANIFEST --dbz --format png,netcdf,zarr --output ROOT`；必须从真实receipt取得来源identity。

US3/T061另核对同步及异步`client.replay_raw_manifest(raw_manifest_path, mode='dbz')`：passed gray/natively decoded reflectivity返回RasterResult，值/质量/来源identity与decode_dbz等价，blocked明确失败。缺省mode=None仍用旧科学分派/返回类型；SHA/绑定失败不重取资料，不把解码、单位或取消错误统一改成IntegrityError。CLI replay成果提交在US4/T078验收。

## 6. 直接数值、命名和事务兼容

- RainViewer composite/TW grid/RDCAP reflectivity用原离线数值基准，数值/质量/坐标及纯改名身份不变，不额外灰度量化。可信地理样本使用原适用四格式核对；native原值可低于0或高于70。
- 比较旧--legacy-display与--gray；--decoded反射率与--dbz等价，非反射率历史文件继续原单位。JSON result类型/退出码保持，mode_info schema1正确，stderr迁移提示不进JSON。
- 从实际wheel而非源码目录导入GrayDbzDecoder和旧兼容类，检查资源旧引用、原config hash、tw-legacy-v1 locator、历史manifest/cache读取；generic fetch/decode不被误改为dbz。
- 超限、取消、shape溢出、错误/中断、skip/overwrite、partial batch沿用已有fault/contract验证方法；没有成功manifest的半成品不可复用。取消后不可迟到commit，成功item不可被另一个失败删除。
- 保存JSON验收报告到新的gray/dbz活动路径，记录版本、输入SHA/来源约束、15条及8条、质量/clip/geo/单位结果。历史证据与001–004任务保持，不自动关闭缺材料/云/live/发布验收。

以上是需要实施后执行并留证的验收清单；005设计完成不表示任何产品场景已通过。
