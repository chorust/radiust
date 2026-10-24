# 现有站址与产品选择器整理表

**整理日期**：2026-10-09；内容取自当前仓库，未重新查询上游。此表是设计审计附件，不作为运行目录加载。

外部交叉核对另见 [Tropical Globe 报告](tropicalglobe-crosscheck.md)：已取得 1,867 条完整 CSV 并对下列 95 条站址候选逐项初筛。本表保留原始仓库值，未按外表修改坐标或身份。

**2026-10-10 补充**：[旧项目对照](legacy-reference.md)确认 SIDARMA 47 项 ID/坐标/bbox 迁移一致，新增/缺失/差异均 0。旧混合目录 704 项、印尼裁剪框差异与旧渠道 skip 单列为审核材料，不改变下表数量、坐标或当前 Query 键。

**实现核对**：Tropical Globe CSV 的 95 个最近候选现已作为不确定的外部目录引用写入既有 station/source resource，包含快照 hash、行号、同国家筛选、距离方法与未验证关系；资源不带入完整 CSV 或 imagery URL。外部 range 仍是独立未验证声明，不等于产品覆盖。RDCAP 的 48 个站点增加了 `coordinate_role=declared_station_location`、datum/精度 unknown 和未知覆盖状态。MY 的两个目录位置标记为区域代表点。

## 材料摘要

- 主目录 25 来源、26 产品条目、50 个 station 条目：RDCAP 48 + MY 区域 2。
- SIDARMA 来源资源另有 47 条历史站址；普通 id 的 47 个站码集合与它相同，表内关联两条查询渠道，不重复统计站址。
- 下文另列 10 个区域/产品选择器（其中 MY 2 已在主目录），合计 105 条站址/选择器整理行。这不是全球实体站数或当前可用数量。
- RDCAP 坐标是目录声明，坐标基准与站址精度仍须核实；SIDARMA 坐标/bbox 来自旧项目记录，覆盖与像元注册均未验证。
- 所有来源的默认自动准入仍待逐产品核对；本次不新增 live、许可或科学能力。

## RDCAP：48 个来源内站点

[主目录](../../python/radiust/resources/catalog.json)；[上游目录留样](../../validation-results/rdcap-analysis/catalog.json)，观察日期 2026-10-01。沿用当前两字母国家前缀站码，不恢复旧斜杠选择器。Owner 列原样保留历史机构名称。

| station_id | 国家 | 原名 | 经度 | 纬度 | Owner 原文 | 目录状态 |
| --- | --- | --- | --- | --- | --- | --- |
| JPAKIT | JPN | Akita | 140.099444 | 39.717778 | Japan Meteorological Agency | Active |
| JPFUNC | JPN | Naze | 129.551944 | 28.394167 | Japan Meteorological Agency | Active |
| JPHAIG | JPN | Hiroshima | 132.593333 | 34.270278 | Japan Meteorological Agency | Active |
| JPHAKO | JPN | Hakodate | 140.781389 | 41.933611 | Japan Meteorological Agency | Active |
| JPISHI | JPN | Ishigakijima | 124.182222 | 24.426667 | Japan Meteorological Agency | Active |
| JPITOK | JPN | Okinawa | 127.764444 | 26.153333 | Japan Meteorological Agency | Active |
| JPKASH | JPN | Tokyo | 139.959722 | 35.859722 | Japan Meteorological Agency | Active |
| JPKURU | JPN | Nagano | 138.195833 | 36.103056 | Japan Meteorological Agency | Active |
| JPKUSH | JPN | Kushiro | 144.5175 | 42.960833 | Japan Meteorological Agency | Active |
| JPMAKI | JPN | Makinohara | 138.133611 | 34.742778 | Japan Meteorological Agency | Active |
| JPMISA | JPN | Matsue | 133.103333 | 35.541667 | Japan Meteorological Agency | Active |
| JPMURO | JPN | Murotomisaki | 134.177222 | 33.252222 | Japan Meteorological Agency | Active |
| JPNAGO | JPN | Nagoya | 136.964722 | 35.168333 | Japan Meteorological Agency | Active |
| JPSAPP | JPN | Sapporo | 141.009722 | 43.138889 | Japan Meteorological Agency | Active |
| JPSEFU | JPN | Fukuoka | 130.356944 | 33.434722 | Japan Meteorological Agency | Active |
| JPSEND | JPN | Sndai | 140.896667 | 38.262222 | Japan Meteorological Agency | Active |
| JPTAKA | JPN | Takamatsu | 135.656389 | 34.616389 | Japan Meteorological Agency | Active |
| JPTANE | JPN | Tanegashima | 130.978611 | 30.639444 | Japan Meteorological Agency | Active |
| JPTOJI | JPN | Fukui | 136.142222 | 36.2375 | Japan Meteorological Agency | Active |
| JPYAHI | JPN | Niigata | 138.816111 | 37.718611 | Japan Meteorological Agency | Active |
| PHAPAR | PHL | Aparri | 121.63 | 18.36 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHBAGU | PHL | Baguio | 120.559 | 16.35623 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHBALE | PHL | Baler | 121.6331 | 15.7502 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Inactive,Active |
| PHBASC | PHL | Basco | 121.9708 | 20.4277 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Inactive |
| PHBOHO | PHL | Bohol | 123.9509 | 9.6467 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHDAET | PHL | Daet | 122.98290799 | 14.12860677 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHGUIU | PHL | Guiuan | 125.75 | 11.04 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHHINA | PHL | Hinatuan | 126.33 | 8.36 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHILOI | PHL | Iloilo | 122.57 | 10.77 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHMACT | PHL | Mactan | 123.98 | 10.32 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHQUEZ | PHL | Quezon | 118.0 | 9.23 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHSUBI | PHL | Subic | 120.36 | 14.82 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHTAGA | PHL | Tagaytay | 121.02 | 14.14 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHTAMP | PHL | Tampakan | 125.03 | 6.41 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| PHVIRA | PHL | Virac | 124.33 | 13.62 | Philippine Atmospheric, Geophysical and Astronomical Services Administration | Active |
| TWRCAA | TWN | Taoyuan Airport | 121.217003 | 25.0786 | Civil Aeronautics Administration | Active |
| TWRCCG | TWN | Chi-Gu | 120.085968 | 23.146721 | Central Weather Bureau | Active |
| TWRCCK | TWN | Chin-Chuan-Kang | 120.6343 | 24.250901 | Air Force Weather Wing | Active |
| TWRCCU | TWN | NCU | 121.1842 | 24.9717 | National Central University | Inactive |
| TWRCGI | TWN | Green Island | 121.483459 | 22.652519 | Air Force Weather | Active |
| TWRCHL | TWN | Hua-Lien | 121.620079 | 23.990305 | Central Weather Bureau | Active |
| TWRCKT | TWN | Ken-Ting | 120.849998 | 21.9 | Central Weather Bureau | Active |
| TWRCLY | TWN | Lin-Yuan | 120.37969 | 22.5261 | Central Weather Bureau | Active |
| TWRCMD | TWN | Maintenance Depot | 121.77278 | 25.07278 | Central Weather Bureau | Active |
| TWRCMK | TWN | Ma-Kung | 119.634399 | 23.562901 | Air Force Weather Wing | Active |
| TWRCNT | TWN | Nan-Tun | 120.5795 | 24.14426 | Central Weather Bureau | Active |
| TWRCSL | TWN | Shu-Lin | 121.4 | 25.0 |  | Active |
| TWRCWF | TWN | Wu-Fen-Shan | 121.772 | 25.073 | Central Weather Bureau | Active |

PHBALE 状态冲突原样保留；Active 不代表当前有数据。台湾 13、日本 20、菲律宾 15。所有站的扫描半径均未由此目录确认；RCHL/ISHI/SUBI 历史帧域也不能推广成其他站的覆盖。

## BMKG/SIDARMA：47 条历史站址候选

[SIDARMA 站表](../../python/radiust/resources/sources/id_sidarma.json)的 `radar_list_origin` 指向旧项目 pinned commit；[普通 id 站码](../../python/radiust/resources/sources/id.json)集合一致。每行可对应 `id_sidarma/cmax/<id>` 与 `id/composite/<id>` 两个不同查询键；坐标不因此独立验证。bbox 不作为已验证扫描覆盖。

| station_id | 原名 | 经度 | 纬度 | 坐标/覆盖状态 |
| --- | --- | --- | --- | --- |
| ACE | Aceh | 95.500186 | 5.541272 | 历史站址待核实；bbox 未验证 |
| AMQ | Ambon | 128.09566999999998 | -3.71184 | 历史站址待核实；bbox 未验证 |
| BAL | Balikpapan | 116.897 | -1.259733 | 历史站址待核实；bbox 未验证 |
| BAN | Banjarmasin | 114.8411 | -3.4614949999999998 | 历史站址待核实；bbox 未验证 |
| BIK | Biak | 136.08801200000002 | -1.162992 | 历史站址待核实；bbox 未验证 |
| BKS | Bengkulu | 102.3412 | -3.8584330000000002 | 历史站址待核实；bbox 未验证 |
| BMA | Bima | 118.6994 | -8.5319 | 历史站址待核实；bbox 未验证 |
| BTH | Batam | 104.111755 | 1.124801 | 历史站址待核实；bbox 未验证 |
| CGK | Soekarno-Hatta Airport | 106.67983 | -6.12397 | 历史站址待核实；bbox 未验证 |
| DEN | Denpasar | 115.178855 | -8.739165 | 历史站址待核实；bbox 未验证 |
| DJB | Jambi | 103.6422 | -1.6309 | 历史站址待核实；bbox 未验证 |
| GNS | Nias | 97.70369699999999 | 1.16439 | 历史站址待核实；bbox 未验证 |
| GOR | Gorontalo | 123.0106 | 0.636 | 历史站址待核实；bbox 未验证 |
| IWJ | Iswahjudi | 111.43944 | -7.605277 | 历史站址待核实；bbox 未验证 |
| JAK | Tangerang | 106.646603 | -6.171344 | 历史站址待核实；bbox 未验证 |
| JAY | Sentani | 140.5278 | -2.5986 | 历史站址待核实；bbox 未验证 |
| KEN | Kendari | 122.46 | -4.059694 | 历史站址待核实；bbox 未验证 |
| KPG | Kupang | 123.624 | -10.218 | 历史站址待核实；bbox 未验证 |
| LOP | Lombok | 116.249444 | -8.753055999999999 | 历史站址待核实；bbox 未验证 |
| LPG | Lampung | 105.175192 | -5.208387999999999 | 历史站址待核实；bbox 未验证 |
| MAJ | Majene | 118.980543 | -3.5507760000000004 | 历史站址待核实；bbox 未验证 |
| MAK | Makassar Hassanudin Airport | 119.531021 | -5.062546 | 历史站址待核实；bbox 未验证 |
| MAS | Masamba | 120.32398 | -2.55456 | 历史站址待核实；bbox 未验证 |
| MAU | Maumere | 122.20131699999999 | -8.612133 | 历史站址待核实；bbox 未验证 |
| MCRC | Mobil Cuaca Respon Cepat | 108.150383 | -6.646921 | 历史站址待核实；bbox 未验证 |
| MED | Medan | 98.63716600000001 | 3.539708 | 历史站址待核实；bbox 未验证 |
| MER | Merauke | 140.416503 | -8.518552 | 历史站址待核实；bbox 未验证 |
| MND | Manado | 124.918517 | 1.500812 | 历史站址待核实；bbox 未验证 |
| NGW | Ngawi | 111.381 | -7.41049 | 历史站址待核实；bbox 未验证 |
| PAL | Palu | 119.8189 | -0.6685 | 历史站址待核实；bbox 未验证 |
| PDG | Padang | 100.3051 | -0.7862 | 历史站址待核实；bbox 未验证 |
| PEK | Pekanbaru | 101.4641 | 0.4541 | 历史站址待核实；bbox 未验证 |
| PKN | Pangkalan Bun | 111.64041599999999 | -2.736559 | 历史站址待核实；bbox 未验证 |
| PKP | Pangkal Pinang | 106.1365 | -2.1615 | 历史站址待核实；bbox 未验证 |
| PKY | Palangkaraya | 113.944833 | -2.25125 | 历史站址待核实；bbox 未验证 |
| PLB | Palembang | 104.702556 | -2.9174439999999997 | 历史站址待核实；bbox 未验证 |
| PON | Pontianak | 109.393257 | -0.08981 | 历史站址待核实；bbox 未验证 |
| PWK | Purwakarta | 107.424 | -6.60992 | 历史站址待核实；bbox 未验证 |
| SBY | Surabaya | 112.76043200000001 | -7.410264999999999 | 历史站址待核实；bbox 未验证 |
| SIN | Sintang | 111.45956000000001 | -0.0427 | 历史站址待核实；bbox 未验证 |
| SMG | Semarang | 110.3675 | -6.984167 | 历史站址待核实；bbox 未验证 |
| SOR | Sorong | 131.28578100000001 | -0.8903209999999999 | 历史站址待核实；bbox 未验证 |
| TAR | Tarakan | 117.58133600000001 | 3.3133169999999996 | 历史站址待核实；bbox 未验证 |
| TIM | Timika | 136.893269 | -4.529045 | 历史站址待核实；bbox 未验证 |
| TTE | Ternate | 127.347369 | 0.855758 | 历史站址待核实；bbox 未验证 |
| WAM | Wamena | 138.946304 | -4.088459 | 历史站址待核实；bbox 未验证 |
| YOG | Yogyakarta | 110.35430159999999 | -7.7316533 | 历史站址待核实；bbox 未验证 |

## 区域和固定产品选择器：10 条

缺少实体站址的字段不补坐标。PTST2 和 TMD 名称可能有关联雷达，但当前材料不足以在本表确立物理站身份。

| source | selector | 关联产品 | 当前分类 | 坐标/几何依据 |
| --- | --- | --- | --- | --- |
| my | peninsular | composite | region_selector | 目录代表点 102,4；不是已验证站址 |
| my | east | composite | region_selector | 目录代表点 114,4；不是已验证站址 |
| es | ESCOMP | composite | product_selector | 来源资源，geometry control_points_pending |
| pt | PTST2 | composite | product_selector | 官方 Leaflet 页面以 EPSG:3857 ImageOverlay 显示全图；edge 注册/尺寸按官方边界推导，属于网页显示几何；实体站位置和观测覆盖未知 |
| sg | SGCOMP | composite | product_selector | 来源资源有球面 AEQD 模型，不能当实体站址 |
| tw | CV1_3600 | observation / grid | product_selector | observation 定位未验证；grid 为 TWD67 数值网格 |
| tw-http | CV1_3600 | observation | product_selector | 与 tw 分渠道，原图定位未验证 |
| ph | PHCOMP4 | composite | product_selector | adapter 的 mosaic 选择器，不能当四号雷达站 |
| th | cmp1 | composite | product_selector | adapter URL 选择器，geometry unverified |
| th | kkn240Loop | composite | product_selector | adapter URL 选择器，名字不能证明 240 km 覆盖 |

## 来源/产品级空间材料：26 条

其余产品可无 station，按产品定位处理。geometry 列引用资源声明，不替代本轮注册/投影验证。native 列只表示当前 Engine 直接数值分派，gray 编码能力另见 005；原图定位和物理量分开。

| source/product | 主目录 station 数 | 现有空间材料声明 | Engine native 数值 | 空间自动选择待办 |
| --- | --- | --- | --- | --- |
| [au/composite](../../python/radiust/resources/sources/au.json) | 0 | unverified | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [bmkg/composite](../../python/radiust/resources/sources/bmkg.json) | 0 | web_mercator_locator_only | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [ca/rain](../../python/radiust/resources/sources/ca.json) | 0 | unverified | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [cam/composite](../../python/radiust/resources/sources/cam.json) | 0 | unverified | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [es/composite](../../python/radiust/resources/sources/es.json) | 0 | control_points_pending | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [fr/composite](../../python/radiust/resources/sources/fr.json) | 0 | WMS 请求 CRS/bbox；显示处理裁剪后的映射须核对 | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [id/composite](../../python/radiust/resources/sources/id.json) | 0 | 站码列表；原图几何未验证 | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [id_sidarma/cmax](../../python/radiust/resources/sources/id_sidarma.json) | 0 | 旧站址与 bbox；当前原图像元几何未验证 | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [kr/composite](../../python/radiust/resources/sources/kr.json) | 0 | unverified | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [my/composite](../../python/radiust/resources/sources/my.json) | 2 | 区域图及代表位置；时间/权限/几何未闭合 | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [nz/rain](../../python/radiust/resources/sources/nz.json) | 0 | unverified | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [opensnow/composite](../../python/radiust/resources/sources/opensnow.json) | 0 | web_mercator_locator_only | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [ph/composite](../../python/radiust/resources/sources/ph.json) | 0 | unverified | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [pt/composite](../../python/radiust/resources/sources/pt.json) | 0 | 官方 Leaflet overlay 显示几何 EPSG:3857、edge、1500×1526；非独立像元控制点或观测覆盖 | 无 | 当前 raw 链路与默认准入未验收；不提供定量雨强 |
| [rainviewer/composite](../../python/radiust/resources/sources/rainviewer.json) | 0 | Web Mercator 瓦片中心，decoder 输出 EPSG:4326；全球域不等于覆盖 | 有 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [rdcap/reflectivity](../../python/radiust/resources/catalog.json) | 48 | 逐帧 EPSG:4326；目录坐标不含覆盖 | 有 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [sg/composite](../../python/radiust/resources/sources/sg.json) | 0 | 球面 AEQD，R=6371000；不在首轮源转换白名单 | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [th/composite](../../python/radiust/resources/sources/th.json) | 0 | unverified | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [th_royalrain/cappi](../../python/radiust/resources/sources/th_royalrain.json) | 0 | unverified | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [tw/observation](../../python/radiust/resources/sources/tw.json) | 0 | PNG/JSON；像元定位未验证 | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [tw/grid](../../python/radiust/resources/sources/tw.json) | 0 | EPSG:3821 数值网格；datum 转换未验证 | 有 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [tw-http/observation](../../python/radiust/resources/sources/tw-http.json) | 0 | unverified | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [uk/rain](../../python/radiust/resources/sources/uk.json) | 0 | 退役，不能默认启用 | 无 | 退役排除 |
| [vn/cmax](../../python/radiust/resources/sources/vn.json) | 0 | unverified | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [windy/reflectivity](../../python/radiust/resources/sources/windy.json) | 0 | xyz_tile_geometry_known_provider_frame_binding_unverified | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |
| [wunderground/composite](../../python/radiust/resources/sources/wunderground.json) | 0 | 未提供可准入的空间材料 | 无 | 访问/自动准入、当前链路、覆盖及投影核对 |

## 材料版本

以下摘要绑定本次审阅输入，便于后续发现目录变化；不绑定生成的设计文档或取得新的来源授权。

| 文件 | SHA-256 |
| --- | --- |
| [python/radiust/resources/catalog.json](../../python/radiust/resources/catalog.json) | 5d42c9c1905ade87b8c812aedc81a67da28cbb05041c8c65f955c8844041bbad |
| [python/radiust/resources/sources/id_sidarma.json](../../python/radiust/resources/sources/id_sidarma.json) | 77fd95a2a0fc25af2e8a106f3aa7337e2e4a827e12caf8ed716146b14361ec31 |
| [python/radiust/resources/sources/id.json](../../python/radiust/resources/sources/id.json) | 08c42a3c26e9ab9900bf911c0ee14e48168d3fe30842a875336672988c8eab4f |
| [validation-results/rdcap-analysis/catalog.json](../../validation-results/rdcap-analysis/catalog.json) | 1235f26c7b47a73c0c2178398fcb4b4184d9f85234203318a1fd172f3f58a317 |
