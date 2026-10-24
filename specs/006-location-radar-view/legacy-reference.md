# radar-icu 只读参考与迁移边界

**日期**：2026-10-10。两个子代理分别收集投影和站表信息，主代理复核关键代码。未执行旧产品、修改两个仓库的实现、读取旧凭据或验证观测上游。

旧工作副本为 `caiyun-radar-icu`，提交 **O**=`54972debade9a8e98de672fd7db9ad902ca9e6ff`；当前 Radiust 代码基线 **R**=`fee15193ec9d69d08bf27c9405f7c501486d17e9`。下列旧路径及行号均针对 O，可在旧副本用 `git show <O>:<path>` 复核，不要求把旧仓库作为运行依赖。

## 1. 投影与图像注册

| 旧证据（O） | 行为 | 006 采用的规则 |
| --- | --- | --- |
| `core/utils/proj.py:4` | pyproj 4326→3857；包装接受 lat/lon、返回 y/x | 公共地理点统一 longitude/latitude，投影点 x/y；3857 米差不作用户地表距离 |
| `core/tiles/base.py:70` | 外框取首瓦片左上和末瓦片右下的 `end+1` | 瓦片外边缘、像元中心分别声明，图像行向南 |
| `core/utils/proj.py:19` | 以宽/高除外框跨度、整数截断裁剪；未存 affine | 由算法可推断为边缘注册，但注释有中心歧义；新几何必须明确 registration、半像元、shape 与 data_window |
| `core/tiles/base.py:181` | 请求时翻转 TMS y，拼接仍按 XYZ 行号 | 可作为方向 fixture；不改变获取策略或新增来源 |
| `core/tiles/base.py:277`、`:333` | 裁后固定 461×461 缩放 | 局部图逐输出点反查原数据窗口；不先固定缩放并沿用旧外框 |
| `script/process_tropicalglobe_radar_db.py:10` | 周长 40075 km 的球体近似，按纬度余弦生成矩形 | 历史派生框只保留来源；不用来认证 AEQD 半径或扫描覆盖 |
| `core/parser/__init__.py:40`、`:103` | 像素圆隐含方图，解析可把无效值置零或丢弃修补 mask | in_view、source_domain、missing/annotation/recovered 等分别保留 |
| `core/tiles/base.py:196`、`:241` | 缺瓦片为空画布，70% 成功阈值 | 下载完成比例不是覆盖率；缺块保持缺测 |
| `core/utils/xyz.py:42`、`:60` | 辅助输出全局像素；无日期线拆分或 180° 边界规范化 | 不作米制投影参考；日期线与 tile 域另测 |
| `core/scrapers/fr_meteofrance_scraper.py:52`、`:153` | 请求 3857 WMS，但后续采用站点配置框 | 新注册绑定实际请求 CRS/bbox/尺寸与同一 receipt，不借站表外框替代 |
| `core/scrapers/sg_scraper.py:18`、`tw_scraper.py:18` | 取得图片/时间，未提供已核验的源投影转换 | 不解除 SG 球面 AEQD、TW datum 的后续 gate |

旧代码没有用户中心椭球 AEQD。二维逆映射、椭球地表距离与源注册的验证方法见 [projection.md](projection.md)；独立参考仍按 [research R01–R04](research.md)建立。

## 2. 旧混合目录的字段计数

`conf/stations.json:2` 至末项 `:16685`：704 个目录键，62 个 `c_code` 分组；704 项有 bounding_box，694 项有 bounding_box_mymx，666 项有更新间隔统计，119 项有 origin_radarid，12 项有 source。没有独立 longitude/latitude 字段。键含 COMP 的有 26 项，含 Mosaic 的有 2 项。

这些是字段/字符串计数，不能解释为 704 个实体站或 62 个通过验收的地区。框中心也不能直接恢复实体站位置。额外记录按用途保留线索：

| 分组 | 旧记录 | 整理方式及证据（O） |
| --- | --- | --- |
| MY | 14 | 两个区域入口加 12 个派生裁剪框，不扩大现有 MY 两个查询 selector；`stations.json:8471` |
| PH | 21 | 产品入口加 20 个裁剪框；与 RDCAP 的来源内站点身份分别核实；`:9431` |
| JP | 21 | 含数字目录键和合成入口；不据数量或键名映射 RDCAP 20 站；`:7430` |
| TW | 2 | 产品目录，不能代替 RDCAP 13 站；`:11406` |

旧 CSV 脚本以国家前两位与站名前四位生成键，并只输出国家、province、派生框与 c_code，丢失原坐标/range 的单位与审核日期（`script/process_tropicalglobe_radar_db.py:36`）。新核对保留快照、原字段和审核关系，不继承该键生成/覆盖方式。

## 3. 印尼列表逐字段对照

比较输入及 SHA-256：

| 文件 | SHA-256 |
| --- | --- |
| O `conf/stations.json` | `07e8aede62c4efd7175489d1baf680d3a93e0e7672b5afbc4048e3eff34d77e1` |
| O `conf/static/id/radarlist.json` | `e2a596824ef602bef50019ae751071e19e6d1333199e4bddb034fd014520effe` |
| R `python/radiust/resources/sources/id_sidarma.json` | `77fd95a2a0fc25af2e8a106f3aa7337e2e4a827e12caf8ed716146b14361ec31` |

旧 `datas` 的 `kode/lon/lat` 对当前 `radars` 的 `id/longitude/latitude`；角点字符串转 float，旧 bbox 统一为 `[TLC.lon,BRC.lat,BRC.lon,TLC.lat]`（WSEN）。两表均为 **47 条、47 个唯一 ID**；顺序、每项 ID、lon、lat、bbox 完全一致，新增/缺失/数值差异均 **0**。当前资源引用的历史提交 `8d251601ca551fbd5c05451f1fb337fc4b75362c` 的列表与 O 字节相同；不改写该原始来源引用。

旧列表从 `radarlist.json:17` 的 ACE 至 `:799` 的 NGW；当前从 [id_sidarma.json](../../python/radiust/resources/sources/id_sidarma.json) 第 14 行至第 566 行。已有全部 ID/坐标见 [station-inventory](station-inventory.md)，不再建立另一份生产站表。字节/数值一致只证明迁移保留历史值，未证明年代、datum、精度或 coverage。

与旧裁剪目录另外比较：印尼有 **46** 项，44 项有显式 origin_radarid。按该关联统一 bbox 顺序后，**38 项不同、6 项相同**；相同为 WAM、CGK、IWJ、MCRC、MAK、NGW。MAJ、PKN、PWK 无显式关联，IDAJ2/IDAN2 缺关联键，不按框或短码猜配（`stations.json:5376`、`:5451`、`:5525`、`:6374`）。因此两份 bbox 必须保留各自用途和出处。

旧 SIDARMA 在载入 47 项后跳过 IWJ、MCRC、PKN，展开 **44** 项（`core/scrapers/id_sidarma_scraper.py:57`、`:73`），代码没有解释原因。新设计不复制此黑名单或据此判断退役；逐项保留待核状态，当前实际支持的 Query/访问结果另行判断。

旧列表 `Stasiun` 可补充 IWJ 的基地、PWK 的高速设施、NGW 的道路里程别名（`radarlist.json:730`、`:780`、`:797`）。MCRC 原名 `Mobil Cuaca Respon Cepat`（`:746`）也不能单凭名称确认其当前固定/移动属性，继续 unknown。列表的 Git 入库日期为 2025-12-29，不是站址测量或 provider 观测日期。

## 4. 合并为设计决定

1. 用户图采用椭球 AEQD；旧 crop/resize 仅提供注册边界和失败案例，不移植旧处理流水线。
2. 复用现有来源内 Query ID，物理站/区域/产品/unknown 分开；旧额外目录不扩大 all 或默认覆盖。
3. 保留 bbox 变体与 skip 的历史事实；没有新证据时不改坐标、状态或覆盖。
4. Tropical Globe 的近距候选仍须独立确认；旧表未解决 JPTAKA 名称冲突、RCMD/RCWF 共址和 Sorong 新旧站歧义，见 [crosscheck](tropicalglobe-crosscheck.md)。
