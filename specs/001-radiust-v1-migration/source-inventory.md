# Migration Inventory Seed

研究日期：2026-09-16。来源仓库：旧项目；基准 HEAD：`8d251601ca551fbd5c05451f1fb337fc4b75362c`。这是只读规划盘点，不是 M0/M4 完成报告，不代表在线可用。旧工作区未提交文件和 output 目录不属于该 commit 的可重现证据。

## 统计口径与范围

- `core/scrapers` 共 21 个 Python 文件，扣除 base/__init__ 后 19 个具体实现；导入 18 类，UkScraper 未导入。
- ALL_SCRAPERS 有 11 个启用、4 个注释项；另有 4 个未注册具体实现（tw-http、fr、pt、uk）。
- `core/tiles/radar.py` 有 5 个命名雷达 tile 实现，5 个在 SOURCES 启用。
- 以下保留 24 条 HEAD 实现路径记录，不据此宣称有 24 个独立数据源；台湾/印尼/泰国多路径不能凭地区名合并。
- 另外保留 2 条历史实现线索。它们不自动计入 HEAD 的 24 条，但归档旧项目之前必须说明去向或有证据排除。
- `core/tiles/satellite.py`、map.py 是非雷达通用实现，不列为迁移来源；通用获取/几何能力可借鉴。Mongo/Kafka/Apollo/监控发布链不迁移。

证据入口（以下路径均相对于旧项目仓库）：scraper 注册 `core/source_app.py`、导入列表 `core/scrapers/__init__.py`、tile 注册 `core/app.py`、tile 实现 `core/tiles/radar.py`。

## HEAD 实现路径

所有行当前迁移状态均为 `inventoried`；fixture_status 均为 `not_versioned`。表内 output 只是本地历史产物线索，未验证是否含原始资料、来源归属、许可、完整时间或可信科学参考值。除原始文件外，所有 product/网格/在线状态须在 M0 落地结构化证据。

| 目标 id（规划） | 旧实现（相对旧仓库） | 注册状态 | 产品/获取线索 | 特殊依赖或语义风险 | 本地历史产物线索 |
| --- | --- | --- | --- | --- | --- |
| kr | core/scrapers/kr_scraper.py | 启用 | CGI 站点 GIF | 自定义 crawl、dateutil、时间解析 | output/kr 20 文件 |
| tw | core/scrapers/tw_s3_scraper.py | 启用 | 匿名 S3 Observation、CV1_3600 | PNG+JSON、s3fs 行为迁到通用存储读取 | output/tw 22 文件，路径归属未分 |
| tw-http | core/scrapers/tw_scraper.py | 未注册 | Observe_radar.js、CV* PNG | brotli、JS 文本发现；与 tw 分开保留 | 同 output/tw |
| ph | core/scrapers/ph_scraper.py | 启用 | PAGASA timeline PNG | CSRF、BeautifulSoup、Playwright fallback | output/ph 空 |
| vn | core/scrapers/vn_scraper.py | 启用 | Hymetnet CMAX00 站点 PNG | HTML/inline JS | output/vn 60 文件 |
| my | core/scrapers/my_scraper.py | 启用 | 两区域 composite GIF | 响应头时间只作证据之一，需确认有效时间绑定 | output/my 12 文件 |
| id_sidarma | core/scrapers/id_sidarma_scraper.py | 启用 | CMAX LastOneHour/Latest | 本地 radarlist、JSON/header | output/id_sidarma 空 |
| es | core/scrapers/es_aemet_scraper.py | 启用 | AEMET national PNG | EPSG:3857 注释需验证；timeline/Referer | output/es 空 |
| ca | core/scrapers/ca_scraper.py | 启用 | ECCC CAPPI *RAIN.gif | 多级 HTML directory | 未见 output/ca |
| th_royalrain | core/scrapers/th_royalrain_scraper.py | 启用 | Royal Rain CAPPI PNG、移动站 | HTML 与时间正则；与 th 不合并 | output/th 46 文件，归属不明 |
| sg | core/scrapers/sg_scraper.py | 启用 | rain-area/DPSRI PNG | slideshowimages JS | output/sg 6 文件 |
| nz | core/scrapers/nz_scraper.py | 启用 | mobileRainRadar_rural_* | JSON、定制 header；不继承关闭 TLS 校验 | output/nz 空 |
| id | core/scrapers/id_scraper.py | 注释 | BMKG sidarmaimage 一小时图 | 自定义 crawl、radarlist；不继承关闭 TLS 校验 | output/id 352 文件，归属不明 |
| cam | core/scrapers/cam_scraper.py | 注释 | Cambodia slideshow | JS/HTML/JSON | 未见 output/cam |
| au | core/scrapers/au_scraper.py | 注释 | BoM FTP IDR*.T.*.png | aioftp 语义迁到 Rust；listing/时间/认证核对 | output/au 366 文件 |
| th | core/scrapers/th_scraper.py | 注释 | TMD cmp1、kkn240Loop live GIF | 注明无历史；禁止用当前图应答历史查询 | 同 output/th |
| fr | core/scrapers/fr_meteofrance_scraper.py | 导入但未注册 | FRCOMP 页面/WMS | source-specific luminance/recovery，不直接当物理值 | 未见 output/fr |
| pt | core/scrapers/pt_ipma_scraper.py | 导入但未注册 | PTST2 JSON timeline/PNG | source-specific luminance/post_process | output/pt 空 |
| uk | core/scrapers/uk_scraper.py | 未导入未注册 | UKCOMP3 DataPoint XML Rainfall | XML/API key，只迁移认证入口不迁移值 | 未见 output/uk |
| windy | core/tiles/radar.py | 启用 | reflectivity PNG tile | green channel 物理含义需证实；Playwright 可选 | 未见专用 fixture |
| rainviewer | core/tiles/radar.py | 启用 | 10 分钟 WebP tile | 自定义通道解码，不按普通 exact PNG 套用 | 未见专用 fixture |
| wunderground | core/tiles/radar.py | 启用 | wuRadarMosaic tile | API key 参数脱敏，时次绑定 | 未见专用 fixture |
| opensnow | core/tiles/radar.py | 启用 | RV-like PNG tile | 旧链仅 merge；必须验证配色与物理语义 | 未见专用 fixture |
| bmkg | core/tiles/radar.py | 启用 | BMKG PNG tile | TMS Y 翻转、IDCOMP palette、parse_img | 未见专用 fixture |

## 历史来源线索

| 项目 | 历史证据 | 规划去向 |
| --- | --- | --- |
| br_cptec | br_cptec_scraper.py 曾存在于 commit 887cbdc3160adc087ff366bddeaf409fc432b907；CLOUD.md 仍引用；本地 output/br_cptec | M0 恢复只读上下文、判断是否替代/删除/仍有产品价值，形成迁移或有证据排除记录 |
| br_sipam | br_sipam_scraper.py 曾存在于 commit 39f07beff4528f2334803c1ee2aedc9b87c9c38e；本地 output/br | M0 同样记录，不自动称 retired 或在线可用 |

## 代表源与迁移批次

| 家族 | 首个代表 | 后续扩展依据 |
| --- | --- | --- |
| HTTP 单图 | my | kr、sg、vn 等；先验证真实时间绑定，不能仅用当前时间 |
| palette/image | id_sidarma | id、bmkg；调色板乱序、未知色、透明和无雨分离 |
| tile | rainviewer | windy、wunderground、opensnow、bmkg；源专属通道与裁剪 |
| FTP/special | au | tw 匿名 S3 acquisition 额外单测 |
| 浏览器/反爬 | ph | windy 浏览器获取分支；登录/CSRF/取消清理 |
| recovery/WMS | fr | pt 与其他存在 source-specific recovery 的实现 |

代表只是验证框架的顺序，不降低剩余来源的真实样本要求。若代表因上游访问不可得，使用可追溯历史 raw；仍无 raw 时记录阻塞与例外，不能用合成图冒充真实来源。

## M0 交付与 M4 关闭规则

1. 固定 commit 和工作区证据来源，为每行补全 product/station、cadence、发布延迟、max_age、历史/预报能力、可变性和时间绑定策略。
2. 获取至少一个真实 raw 集合，记录 SHA-256、来源、采集时间、使用依据、脱敏和参考值/容差。区分 raw 与旧 `_map.png`；旧 `value*3.2 → clip → uint8` 不作新 canonical truth。
3. 将 palette/station/product/static resource 转成随包版本化资料，记录未覆盖和未知地理定位，不静默补造。
4. 逐来源执行 discover/acquisition/decode contract；随后执行对应 CLI、SDK、格式和缓存故障场景，差异写 migration note。
5. retired/upstream_unavailable 的结论需要有日期的证据。获取失败或未运行 live 测试不能直接等同 retired。
6. 所有行完成或有已接受例外，所有非例外 fixture 回归及代表 live smoke 通过后，才提出旧仓库归档；本次规划不执行归档。

`jma` 在原始计划中作为调用示例出现；本次 HEAD 盘点未发现对应实现，不凭示例声称 JMA 已在迁移清单。
