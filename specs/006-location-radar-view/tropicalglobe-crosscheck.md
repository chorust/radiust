# Tropical Globe 站表交叉核对

**核对日期**：2026-10-09。范围为现有 RDCAP 48 条和 SIDARMA 47 条站址候选；只作目录差异审计，未修改生产站址、覆盖或可用性。

## 1. 导出范围与材料

- 来源：[Tropical Globe 导出页](https://tropicalglobe.com/radar_database/#show_export=1)，通过页面自身的 CSV 导出函数取得；用户给出的 blob URL 是浏览器临时对象，不作为长期下载地址。
- 采集时间：`2026-10-09T14:32:52.125Z`；文件名 `Tropical_Globe_Radar_Database_(downloaded_2026-10-09).csv`。
- 国家选择为 all countries。默认显示导出 1,615 条；本次对比使用完整导出 1,867 条，包含 unavailable、closed、private、USA TDWR、USA TV，并保留 review_date。导出后恢复原显示设置。
- 页面说明隐藏雷达会从导出中消失；默认导出缺行不能解释为实体站不存在。
- 完整 CSV SHA-256：`c4c090963285840972571fff4de6b32eceb76506148fd304b2de4fafa9ba047d`。37 列、1,867 条数据行均有完整列数，坐标值有限且在经纬度范围内。
- 关键列为 country_id、radar_site_name/alt_name、latitude/longitude、agency、range/range_unit、closed/not_available、review_needed/review_date、imagery_url。没有可供跨来源稳定连接的统一 station ID；wrd_page_id 多处为空或 old，不能作主键。
- 同次 JSON 导出头声明作者 Tropical Globe、许可 LGPL-3.0-or-later。该 JSON 的 radar_list 末尾有 trailing comma，严格 JSON 解析失败；本次只使用合法 CSV 比对，未修写上游原件。许可声明与目录复核均不能授予其链接的雷达图像使用权。
- 原始 CSV/JSON 留在本机 `/tmp/radiust-tropicalglobe-all.csv`、`/tmp/radiust-tropicalglobe-all.json`，不纳入内置资源或软件许可范围；此目录会被清理，复核时按上述日期、过滤条件与 hash 确认输入。

## 2. 初筛方法与统计

先按 country_id 限制候选，再列出最近三个坐标候选；结合名称、别名、观测机构及目录注释人工检查。距离本身没有把任何记录标为“确认同站”。表内行号绑定上述 CSV hash，换快照后不能当作稳定身份。

距离为标准库 Haversine、平均球半径 6,371,008.8 m 的近似值，单位 km，假定两表经纬度可比较。两表坐标 datum/精度尚未独立核实；这些值只用于差异初筛，不是椭球投影精度验收。100 m、1 km、5 km 是审核分桶，不是站点身份或雷达覆盖阈值。

| 材料 | 本地记录 | 外表同国家记录 | ≤100 m | 100 m–1 km | 1–5 km | 无 5 km 内候选 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| RDCAP 台湾 | 13 | 11 | 6 | 6 | 0 | 1 |
| RDCAP 日本 | 20 | 100 | 19 | 1 | 0 | 0 |
| RDCAP 菲律宾 | 15 | 28 | 5 | 7 | 3 | 0 |
| SIDARMA 印尼 | 47 | 44 | 37 | 1 | 3 | 6 |
| 合计 | 95 | 183 | 67 | 15 | 6 | 7 |

88 条有 5 km 内候选，包含下列名称冲突、共址和迁站歧义；不能报告 88 个已确认实体匹配。外表的额外记录也不等于 Radiust 新增可用站点。马来西亚外表 20 条实体站记录与本地两个区域选择器不是同一计数单位，不能互相替换。

## 3. 优先核对项

| 项目 | 发现 | 设计处理 |
| --- | --- | --- |
| JPTAKA | RDCAP 名称 Takamatsu，坐标附近的外表为 Kinki / Osaka，约 0.023 km | 名称/站址冲突，查 RDCAP 与 JMA 原始站码；不自动改名或确认同站 |
| JPMAKI / JPSEND | Makinohara 对外表 Tokai / Shizuoka；Sndai 对 Sendai | 保留原名和候选别名，需来源依据；不是无条件按字符串覆盖 |
| TWRCMD / TWRCWF | 两个 RDCAP 身份对应外表同一行 Wufenshan；外表注释明确提到 RCWF 与 RCMD 共址、不同雷达 | 保留两个 station identity，可关联同一设施；外表一行不能把两站合并。约 35 m 的布局属于外表声明，尚未独立测量 |
| SOR | 最近外表行 1357 为 closed 旧雷达，距本地约 0.019 km；行 1356 为另一新雷达，约 0.040 km；注释说明旧站拆除后旁建 | 同时保留迁站候选、年代及状态，不选最近旧坐标覆盖当前站址 |
| PHBALE / PHBASC | 本地 Baler Active/Inactive 冲突，Basco Inactive；外表 not_available=0 | 目录状态冲突待核；外表标记也不能替代当前原始获取 |
| TWRCCU / CGK / IWJ / MCRC / MAK / PWK / NGW | 无同国家 5 km 内候选 | 标为外表未近距对应；不能删除本地记录或宣布实体退役 |
| MCRC | 原名包含 Mobil Cuaca Respon Cepat，固定站址属性未确认 | 标为 kind 未确认，先核对是否移动雷达；没有当前帧位置时不进入固定站址排序 |
| 外表 range | 有缺值、不同比例的图像范围和估计值；JMA 注释说明并不知道全部哪些站进入地图，Kendari 注释也未确认最大范围 | 保存为 external_range_claim，仅作审计；不写 verified observation_coverage，不生成默认扫描圆 |

1–5 km 的六条差异：PHHINA≈1.241、PHILOI≈1.086、PHVIRA≈1.151、KEN≈1.265、MER≈4.206、SMG≈1.329 km。这些距离可能影响 1 km 视图定位，应在对应产品空间准入前核实。

无 5 km 内候选中，CGK 的最近行其实是本地 JAK 的候选；MAK 的机场名称与外表 Makassar 不同且相距约 8.518 km。两者均不能借邻站坐标补齐。NCU、IWJ、MCRC、PWK、NGW 同样保留缺口。

## 4. 全部 95 条初筛结果

下表一律为坐标候选，尚无实体匹配验收。≤100 m 记“近距待核”，100 m–1 km 记“坐标复核”，1–5 km 记“显著差异”，>5 km 记“无近距对应”；特殊身份问题单独标注。

| 来源/站码 | 原名 | 最近外表名称/别名 | CSV 行 | 近似距离 km | 审计状态 |
| --- | --- | --- | ---: | ---: | --- |
| rdcap/JPAKIT | Akita | Tohoku, North / Akita | 1380 | 0.021 | 近距待核 |
| rdcap/JPFUNC | Naze | Amami / Naze | 1394 | 0.014 | 近距待核 |
| rdcap/JPHAIG | Hiroshima | Shikoku / Hiroshima | 1390 | 0.001 | 近距待核 |
| rdcap/JPHAKO | Hakodate | Hokkaido, Southwest / Hakodate | 1379 | 0.011 | 近距待核 |
| rdcap/JPISHI | Ishigakijima | Miyakojima / Yaeyama / Ishigakijima | 1396 | 0.016 | 近距待核 |
| rdcap/JPITOK | Okinawa | Okinawa | 1395 | 0.055 | 近距待核 |
| rdcap/JPKASH | Tokyo | Kanto / Tokyo | 1384 | 0.010 | 近距待核 |
| rdcap/JPKURU | Nagano | Koshin / Nagano | 1383 | 0.104 | 坐标复核 |
| rdcap/JPKUSH | Kushiro | Hokkaido, East / Kushiro | 1378 | 0.017 | 近距待核 |
| rdcap/JPMAKI | Makinohara | Tokai / Shizuoka | 1385 | 0.018 | 近距待核 |
| rdcap/JPMISA | Matsue | Chugoku / Matsue | 1389 | 0.010 | 近距待核 |
| rdcap/JPMURO | Murotomisaki | Shikoku / Murotomisaki | 1391 | 0.013 | 近距待核 |
| rdcap/JPNAGO | Nagoya | Kinki / Nagoya | 1386 | 0.052 | 近距待核 |
| rdcap/JPSAPP | Sapporo | Hokkaido, Northwest / Sapporo | 1377 | 0.018 | 近距待核 |
| rdcap/JPSEFU | Fukuoka | Kyushu, North / Fukuoka | 1392 | 0.030 | 近距待核 |
| rdcap/JPSEND | Sndai | Tohoku, South / Sendai | 1381 | 0.062 | 近距待核 |
| rdcap/JPTAKA | Takamatsu | Kinki / Osaka | 1388 | 0.023 | 名称冲突 |
| rdcap/JPTANE | Tanegashima | Kyushu, South / Tanegashima | 1393 | 0.024 | 近距待核 |
| rdcap/JPTOJI | Fukui | Hokuriku, West / Fukui | 1387 | 0.019 | 近距待核 |
| rdcap/JPYAHI | Niigata | Hokuriku, East / Niigata | 1382 | 0.012 | 近距待核 |
| rdcap/PHAPAR | Aparri | Aparri | 1545 | 0.038 | 近距待核 |
| rdcap/PHBAGU | Baguio | Baguio | 1546 | 0.005 | 近距待核 |
| rdcap/PHBALE | Baler | Baler | 1547 | 0.170 | 坐标复核 |
| rdcap/PHBASC | Basco | Basco | 1549 | 0.029 | 近距待核 |
| rdcap/PHBOHO | Bohol | Alburquerque / Bohol Province | 1548 | 0.028 | 近距待核 |
| rdcap/PHDAET | Daet | Daet | 1553 | 0.005 | 近距待核 |
| rdcap/PHGUIU | Guiuan | Guiuan | 1554 | 0.835 | 坐标复核 |
| rdcap/PHHINA | Hinatuan | Hinatuan | 1555 | 1.241 | 显著差异 |
| rdcap/PHILOI | Iloilo | Iloilo City | 1556 | 1.086 | 显著差异 |
| rdcap/PHMACT | Mactan | Cebu City / Mactan Island | 1552 | 0.285 | 坐标复核 |
| rdcap/PHQUEZ | Quezon | Quezon, Palawan (province) | 1558 | 0.606 | 坐标复核 |
| rdcap/PHSUBI | Subic | Subic | 1559 | 0.464 | 坐标复核 |
| rdcap/PHTAGA | Tagaytay | Tagaytay | 1560 | 0.339 | 坐标复核 |
| rdcap/PHTAMP | Tampakan | Tampakan | 1561 | 0.847 | 坐标复核 |
| rdcap/PHVIRA | Virac | Virac | 1562 | 1.151 | 显著差异 |
| rdcap/TWRCAA | Taoyuan Airport | Taoyuan International Airport | 1680 | 0.020 | 近距待核 |
| rdcap/TWRCCG | Chi-Gu | Tainan City / Cigu District | 1670 | 0.006 | 近距待核 |
| rdcap/TWRCCK | Chin-Chuan-Kang | Taichung City / Daya District | 1679 | 0.009 | 近距待核 |
| rdcap/TWRCCU | NCU | Taoyuan International Airport | 1680 | 12.322 | 无近距对应 |
| rdcap/TWRCGI | Green Island | Green Island / Ludao Township | 1677 | 0.013 | 近距待核 |
| rdcap/TWRCHL | Hua-Lien | Hualien County / Hualien City | 1672 | 0.905 | 坐标复核 |
| rdcap/TWRCKT | Ken-Ting | Pingtung County / Kenting National Park | 1671 | 0.524 | 坐标复核 |
| rdcap/TWRCLY | Lin-Yuan | Kaohsiung City / Linyuan District | 1674 | 0.025 | 近距待核 |
| rdcap/TWRCMD | Maintenance Depot | New Taipei City / Wufenshan | 1673 | 0.862 | 共址关联；不得合并 |
| rdcap/TWRCMK | Ma-Kung | Penghu County / Huxi Township | 1678 | 0.527 | 坐标复核 |
| rdcap/TWRCNT | Nan-Tun | Taichung City / Nantun District | 1675 | 0.006 | 近距待核 |
| rdcap/TWRCSL | Shu-Lin | New Taipei City / Shulin District | 1676 | 0.431 | 坐标复核 |
| rdcap/TWRCWF | Wu-Fen-Shan | New Taipei City / Wufenshan | 1673 | 0.944 | 共址关联；不得合并 |
| id_sidarma/ACE | Aceh | Banda Aceh | 1323 | 0.008 | 近距待核 |
| id_sidarma/AMQ | Ambon | Ambon | 1321 | 0.010 | 近距待核 |
| id_sidarma/BIK | Biak | Biak | 1327 | 0.007 | 近距待核 |
| id_sidarma/BAN | Banjarmasin | Banjarmasin | 1324 | 0.002 | 近距待核 |
| id_sidarma/BKS | Bengkulu | Bengkulu | 1326 | 0.009 | 近距待核 |
| id_sidarma/BAL | Balikpapan | Balikpapan | 1322 | 0.024 | 近距待核 |
| id_sidarma/JAK | Tangerang | Jakarta / Tangerang | 1359 | 0.010 | 近距待核 |
| id_sidarma/DEN | Denpasar | Denpasar | 1329 | 0.012 | 近距待核 |
| id_sidarma/GNS | Nias | Nias | 1344 | 0.025 | 近距待核 |
| id_sidarma/GOR | Gorontalo | Gorontalo | 1330 | 0.006 | 近距待核 |
| id_sidarma/JAY | Sentani | Jayapura | 1332 | 0.021 | 近距待核 |
| id_sidarma/DJB | Jambi | Jambi | 1331 | 0.016 | 近距待核 |
| id_sidarma/YOG | Yogyakarta | Yogyakarta | 1364 | 0.002 | 近距待核 |
| id_sidarma/KEN | Kendari | Kendari | 1333 | 1.265 | 显著差异 |
| id_sidarma/KPG | Kupang | Kupang | 1334 | 0.048 | 近距待核 |
| id_sidarma/LOP | Lombok | Mataram / Lombok island | 1340 | 0.009 | 近距待核 |
| id_sidarma/LPG | Lampung | Bandar Lampung | 1335 | 0.007 | 近距待核 |
| id_sidarma/MED | Medan | Medan | 1342 | 0.012 | 近距待核 |
| id_sidarma/MAJ | Majene | Majene | 1336 | 0.007 | 近距待核 |
| id_sidarma/MND | Manado | Manado | 1338 | 0.005 | 近距待核 |
| id_sidarma/MAU | Maumere | Maumere | 1341 | 0.013 | 近距待核 |
| id_sidarma/MER | Merauke | Merauke | 1343 | 4.206 | 显著差异 |
| id_sidarma/MAS | Masamba | Masamba | 1339 | 0.004 | 近距待核 |
| id_sidarma/PDG | Padang | Padang | 1345 | 0.037 | 近距待核 |
| id_sidarma/PKN | Pangkalan Bun | Pangkalan Bun | 1350 | 0.006 | 近距待核 |
| id_sidarma/PKP | Pangkal Pinang | Pangkal Pinang | 1349 | 0.010 | 近距待核 |
| id_sidarma/PEK | Pekanbaru | Pekanbaru | 1351 | 0.265 | 坐标复核 |
| id_sidarma/PKY | Palangkaraya | Palangkaraya | 1346 | 0.002 | 近距待核 |
| id_sidarma/PLB | Palembang | Palembang | 1347 | 0.004 | 近距待核 |
| id_sidarma/PAL | Palu | Palu | 1348 | 0.016 | 近距待核 |
| id_sidarma/PON | Pontianak | Pontianak | 1353 | 0.007 | 近距待核 |
| id_sidarma/SBY | Surabaya | Surabaya | 1358 | 0.008 | 近距待核 |
| id_sidarma/SMG | Semarang | Semarang | 1354 | 1.329 | 显著差异 |
| id_sidarma/SOR | Sorong | Sorong | 1357 | 0.019 | 新旧站歧义；最近为 closed |
| id_sidarma/SIN | Sintang | Sintang | 1355 | 0.025 | 近距待核 |
| id_sidarma/TIM | Timika | Timika | 1362 | 0.032 | 近距待核 |
| id_sidarma/TTE | Ternate | Ternate | 1361 | 0.014 | 近距待核 |
| id_sidarma/TAR | Tarakan | Tarakan | 1360 | 0.006 | 近距待核 |
| id_sidarma/BMA | Bima | Bima | 1328 | 0.025 | 近距待核 |
| id_sidarma/BTH | Batam | Batam | 1325 | 0.008 | 近距待核 |
| id_sidarma/WAM | Wamena | Wamena | 1363 | 0.015 | 近距待核 |
| id_sidarma/CGK | Soekarno-Hatta Airport | Jakarta / Tangerang | 1359 | 6.431 | 无近距对应 |
| id_sidarma/IWJ | Iswahjudi | Yogyakarta | 1364 | 120.407 | 无近距对应 |
| id_sidarma/MCRC | Mobil Cuaca Respon Cepat | South Tangerang / Pondok Betung | 1352 | 160.591 | 无近距对应；固定站址未确认 |
| id_sidarma/MAK | Makassar Hassanudin Airport | Makassar | 1337 | 8.518 | 无近距对应 |
| id_sidarma/PWK | Purwakarta | South Tangerang / Pondok Betung | 1352 | 83.901 | 无近距对应 |
| id_sidarma/NGW | Ngawi | Yogyakarta | 1364 | 118.671 | 无近距对应 |

## 5. 下一步整理顺序

1. 优先核对 JPTAKA、RCMD/RCWF 和 Sorong，明确错误、共址设施与迁站三个不同关系；保留所有来源内 Query ID。
2. 对六条 >1 km 差异和 MAK/CGK 的站点身份取得原始机构/上游目录依据，分别记录坐标基准、年代与产品几何；不把新坐标硬套进旧 bbox。
3. 核实七条没有近距对应记录的性质，保留移动站/关闭/外表缺项的不同结果。用 unknown 暂存类型不确定项，不猜定实体类别。
4. 在已核实身份的记录上补名称别名、外部目录关联、range 原始声明及复核日期；生产坐标只在证据足够时更新，与 Python/Rust 镜像同步。
5. 按 [station-catalog.md](station-catalog.md)继续产品级几何/覆盖/默认用途准入。未接入的新站、私有站和关闭站不因出现在全球 CSV 而进入自动发现。
