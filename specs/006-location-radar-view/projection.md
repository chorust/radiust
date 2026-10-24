# 用户居中的投影转换设计

**Status**: 设计与实现基线。Core 已接入 WGS84 椭球 AEQD 正反解、半径不超过 500 km 的局部网格和受支持源的逐点采样；独立参考/采样边界见 `validation-results/006-local-view-offline.json`。真实来源几何与在线准入仍逐产品开放。

## 1. 坐标定义

中心 `C=(longitude, latitude)` 使用 WGS84，经纬度单位度。目标投影为椭球 AEQD：

```text
+proj=aeqd +lat_0=<latitude> +lon_0=<longitude> +datum=WGS84 +units=m +no_defs
```

这是参数化 CRS，没有一个适用于所有用户位置的固定 EPSG 编号。内部 x 向东、y 向北，单位 m；展示转换为 km。报告必须保留中心与椭球参数，不能只写 `Cartesian`。

AEQD 保持从中心出发的距离与方位，不承诺等面积、任意点间距离或雨量守恒。方法依据见 [research R01/R02](research.md)。

## 2. 正反转换

采用成熟测地线库的椭球 direct/inverse；不自行实现其迭代算法。

```text
地理点 P → 局部点：
  (s, azi) = WGS84.inverse(C, P)   # azi 从北顺时针
  x = s * sin(azi)
  y = s * cos(azi)

局部点 → 地理点：
  s = hypot(x, y)
  azi = atan2(x, y)
  P = WGS84.direct(C, azi, s)
```

库调用的角度单位与 `sin/atan2` 的弧度单位显式转换。`s=0` 直接返回中心；拒绝 NaN/Inf。经度规范化到 `[-180,180)`，跨日期线按源轴的连续经度分支查找，不能对整个图像用 min/max 经度判覆盖。极点采用中心经度固定的方向约定，必须列入独立参考检查。

## 3. 目标网格与半径

设计默认 `radius_km=50`、`resolution_m=1000`、`resampling=nearest`。半径要求 `0<R≤500 km`；间距有限且为正，并受共享像素/内存限制。最大范围是首版设计约束，不是产品覆盖半径。

令 `R` 为 m，`d` 为请求间距：

```text
k = ceil(R / d)
n = 2*k + 1
effective_resolution_m = R / k
x[j] = (j-k) * effective_resolution_m
y[i] = (k-i) * effective_resolution_m
in_view[i,j] = hypot(x[j], y[i]) <= R
```

乘除、ceil、整数转换与 `n*n` 在分配前作有限性和 checked arithmetic 校验。默认得到 `101×101`、1 km 间距，中心像元正好 `(0,0)`；行从北向南、列从西向东。非整除范围实际间距小于等于请求间距，报告两者，不静默改半径。

两轴的像元**中心**为 `−R～+R`，像元边缘延伸半个实际间距。因此显示轴刻度与半径 mask 以中心定义；若输出 `affine`，必须写实际像元边缘：

```text
[-R-d_eff/2, d_eff, 0, R+d_eff/2, 0, -d_eff]
```

圆外不采样。`in_view=false` 与 `source_outside` 分开：前者是用户范围裁切，不能被标成雷达站外；数值可置 NaN/missing，渲染可透明，但必须保留独立视图 mask。

## 4. 输出点反查源像元

```mermaid
flowchart LR
    A[局部 x y] --> B[逆 AEQD 到 WGS84 经纬度]
    B --> C[转换到受支持的源 CRS]
    C --> D[按源注册找到像元]
    D --> E[采样数值或原图并保留质量]
```

源 CRS 准入表：

| 源定位 | 首版处理 | 边界 |
| --- | --- | --- |
| EPSG:4326，可信 x/y 或 affine | 直接使用逆 AEQD 的 lon/lat 查源轴 | 保留原像元中心与方向，支持非均匀但单调的轴 |
| EPSG:3857，可信 x/y 或 affine | 复用现有 WGS84 经纬度到 Web Mercator 变换 | 纬度域外拒绝；不是把 Mercator 米平移成局部米 |
| EPSG:3821/TWD67 | 返回 `unsupported_crs` | datum 转换待独立验证，TW grid 不能改标 4326 |
| SG 已记录的球面 AEQD | 返回 `unsupported_crs` | 后续按该来源中心和球半径实现并核对注册，不套 WGS84 椭球参数 |
| 无 CRS、坐标冲突、只有图片/站址 | 返回 `geometry_unverified` | 保留显式原图入口，不输出假投影 |
| 极坐标体扫、曲线或旋转网格 | 本轮不新增通用处理 | 先保留并解释不支持，不建立全投影插件系统 |

### 源像元注册

源几何必须声明 `registration`，不能从四个经纬角点猜测：

| 已有材料 | 构建源网格的规则 |
| --- | --- |
| 像元中心 x/y 轴 | 保留原轴、方向及其现有采样有效域；不自动扩半像元 |
| 可信像元边缘 bbox + width/height | 在源 CRS 中以跨度除以 width/height；中心加半步，affine 使用外边缘；边缘域内的 nearest 可取最边缘像元，不作域外外推 |
| 只有 affine | 校验 CRS、实际 shape、边缘约定、旋转项；轴对齐时构建中心轴，旋转映射首轮拒绝 |
| 网页 overlay 的经纬外框 | 同时查地图 CRS/图像映射，不能直接认定图像 y 轴线性于纬度 |
| 只有站点 bbox、扫描半径或两个角点 | 不构建像元注册，保持 geometry_unverified |

边缘注册的 bilinear 沿用既有要求，不借缺少的外侧邻点填值；有效结果范围及方法写入处理记录。边缘/中心注册区别只用于已认证源材料，不能改变旧 regrid 的有效域。
边缘域内若缺少 bilinear 所需邻点，保持 `source_domain=true`，输出 NaN 与 missing 质量；只有源物理范围外才增加 outside。缺少可插值样本不等于没有观测覆盖。


原图必须明确 `data_window=(column,row,width,height)`，以及该窗口在原图还是裁后图中的坐标。裁掉标题/图例时相应平移 affine 原点；若缩放，则更新步长及注册版本，重新核对实际尺寸。首版本流程直接采样原数据窗口，不先固定缩放。WMS 栅格的定位绑定实际请求 CRS、bbox、尺寸及返回帧，不能用站点 bbox 替代请求范围。保留 frame/artifact 摘要与几何版本，尺寸或版本不符即拒绝。

PT 的现有 ImageOverlay 外框与线性 4326 解释需按 [research R11](research.md)补核，不能仅凭 `geometry_status` 字符串准入。旧项目的边缘注册、方向与裁剪陷阱见 [legacy-reference](legacy-reference.md)。

## 5. 数值、图像与质量

- 数值模式先在源网格解码，再投影。最近邻保留取样值和 quality；分辨率变细只是重采样。
- 可选 bilinear 仅作用于连续数值；dBZ 在 `Z=10^(dBZ/10)` 域插值后反算，复用当前已验证规则。类别、质量、origin/alpha/adjustment 等伴随平面采用最近邻，不能平均 bit mask；新增插值位记录处理发生。
- 涉及编码派生数据时保留全部已存在的伴随平面；若当前内核不能传递某平面，该路径保持不可用，不能为了画图丢失来源质量。
- 原图只做几何最近邻采样，保留提供方 RGBA 与图例，不进行颜色到物理量推断。初版不为原图开放 bilinear。
- 源范围外为 NaN + 已有 outside/missing 语义；源缺测和图面标记继承原质量。对源范围外样本不能借用边界像元的 recovered 等标记。
- `in_view`、`source_domain`、观测 coverage 证据及当前 quality 分别保留。投影后的有效像元比例称 `valid_sample_fraction`，不能当作扫描覆盖率或有雨比例。
- 整帧 missing 或低于显示阈值不自动等于 provider 无数据，也不触发按颜色切换来源。

## 6. 复用与资源

在 [grid.rs](../../crates/radiust-core/src/grid.rs) 中提取 nearest/bilinear 的查轴与采样内部函数，源轴方向只预处理一次；AEQD 增加二维坐标映射，旧同 CRS/Web Mercator 路径保持原契约。局部入口明确指定上述 registration 及域外质量，不直接调用现有分轴 regrid。源 `GeometryEvidence` 的可信输入判定与派生 AEQD 输出分开，不修改源 EPSG 身份，也不为了通过已知 CRS 检查把目标改标 4326。

按输出行或有界块计算地理坐标并立即采样，不保存完整二维 lon/lat 副本。默认 `101×101` 的 values+quality 至少 61,206 字节；视图 mask、RGBA、源帧、解码与渲染缓冲另计。在 local operation 开始时调用一次 `Limits.raster_memory_budget()`，所有阶段共享其 clone 的计数器；不能各阶段重新调用后声称预算共享。现有 native decoder 在原头部/shape 检查及分配点接收该预算，预留源+输出同时存活缓冲再分配；不从 max_pixels 推定内存足够。输出以内部 owner/lease 维持真实缓冲生命周期，绑定/渲染额外复制另计，不将 lease 放进公开 JSON。

一次 local_view 使用一个绝对期限和取消 token。源获取/解码/投影/渲染都受剩余时间限制；CPU permit 等待可取消，worker 内按有界块检查取消和期限，受控取消目标为 5 秒内收尾。worker 真正退出前保持 permit 与缓冲 lease，不能超时后提前释放预算继续堆积后台任务；取消或期限之后的结果不得发布。

## 7. 处理记录与验证目标

记录 `local-aeqd-wgs84-v1`、中心、半径、请求/实际间距、CRS 定义、源注册、采样方式、真实 valid_time 和源 identity。源 raw 缓存 identity 不变；派生 identity 绑定上述参数和输入 receipt。当前只设计内存视图/cat，不新增正式 projected writer；已有 writer 未支持的 CRS 继续拒绝。

实施验收使用独立 PROJ/GeographicLib 参考，而非只做正反转换自洽：半径 10/50/200/500 km、四方位及斜向参考点、高纬度、日期线、极点方向、升降序/非均匀源轴、中心/边缘注册、半像元偏移、非方形原图、data_window 裁剪、WMS 范围冲突、missing/annotation/范围 mask。测地坐标误差目标 ≤1 m；原始几何误差单列，不能由算法精度覆盖。nearest 逐值与质量精确比较，bilinear 的数值与位标记按原契约核对。
