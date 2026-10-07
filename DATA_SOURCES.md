# Radiust 数据来源政策与记录

Radiust 的软件采用 [Apache-2.0](LICENSE)。上游雷达观测、图像、瓦片及其加工产品保留各自的授权条件，Radiust 不统一重新许可这些数据。公共机构发布、公开 URL、无需登录或成功下载，都不自动证明允许商用或再分发。

本文建立截至 **2026-10-06** 的来源政策与初始权限台账。技术信息来自当前代码、目录及既有验证记录；除明确列出的条款核对外，不代表本日重新实测全部上游或完成法律审核。获取与科学能力的历史快照见[下文](#历史获取与科学验证记录)，逐源技术证据见[迁移记录](migration/sources/)和[验证材料](validation-results/)。[README](README.md#features)只保留能力概览。

## 接入与选择原则

- 官方来源优先，也接受可靠、可追溯的第三方产品。多源覆盖服务于稳定性、质量和用户选择。
- 默认由用户设备连接所选来源，按来源要求配置自己的凭据；获取与回退都遵守访问条款、主机限制、请求预算、缓存和重试限制。不得绕过授权、付费或访问控制。
- 接口、文件、对象存储和官方网页按实际条件评估。浏览器会话是获取方式之一，不是授权依据；维护成本也是长期支持的判断条件。
- 默认来源、用户选择和回退后的实际来源应可辨认。回退不消除备用来源的配置或授权要求，也不使两个产品的色标、覆盖和时次自动等价。
- 退役来源不作为默认选项；需要配置、暂时失败、无数据、过期和科学解码未验证分别说明。来源应按地区、站点、产品和具体能力声明支持。

## 每个来源需要保留的信息

| 记录项 | 要求 |
| --- | --- |
| 身份与覆盖 | 稳定 source ID、国家／地区、站点、产品、实际覆盖和分辨率；不以提供方名称推断全球覆盖 |
| 来源链 | 观测机构、产品提供方、分发渠道；第三方提供的上游说明与未确认部分分别记录 |
| 访问 | 原始页面／文档 URL、协议、端点模式、凭据要求、更新节奏、请求限额及查询时间支持 |
| 时间与处理 | 产品有效／观测时间、获取时间及必要的发布或生成时间；加工、拼接、裁剪、解码与重投影历史 |
| 权限 | 条款 URL 或许可文本、署名要求、商用、再分发、保留与缓存条件、适用产品及渠道 |
| 证据与状态 | 核对日期、依据、缺口；原始获取、图像展示、几何定位与科学解码分别说明 |

“未确认”意味着没有足够依据，不等于允许，也不等于禁止。发现明确限制时单独记录。对外启用或宣传具体产品前，应补齐该用途所需的访问与授权依据；商业集成者同样需要核实自己的用途。项目自身的获取、保存和分发行为也必须符合相应条件。

静态端点模式可以保留；凭据、cookie、临时签名 URL 和文件票据不能进入公开报告或长期身份。对用户展示的原始来源链接应安全且可核查。

## 当前原生来源与访问渠道

[当前目录](python/radiust/resources/catalog.json)登记 **25 个来源**。下表描述渠道和已有访问限制，不是实时可用性或合法使用的保证。官方提供方名称是来源归属信息；底层观测所有者或第三方加工链未明确时需要继续核实。

| ID | 国家／地区与产品提供方 | 分发渠道（不含秘密） | 访问记录／待办 |
| --- | --- | --- | --- |
| `au` | 澳大利亚 · BoM | `ftp://ftp.bom.gov.au/anon/gen/radar/`，官方匿名 FTP | 已有原始获取证据；色标与几何待验收 |
| `bmkg` | 印尼 · BMKG 瓦片 | `inasiam.bmkg.go.id`，官方瓦片路由 | 既有探测遇上游 403；不绕过限制 |
| `ca` | 加拿大 · ECCC | `dd.meteo.gc.ca`，官方按日期文件目录 | 已有原始获取证据；科学能力待验收 |
| `cam` | 柬埔寨 · 气象部门雷达 | `www.cambodiameteo.com`，公开 slideshow | 已有临时原图获取记录；许可与科学证据待补 |
| `es` | 西班牙 · AEMET | `www.aemet.es/es/api-eltiempo/radar/`，官方时间线与图像 | 已有原始获取证据；数值和几何待验收 |
| `fr` | 法国 · Météo-France | `rwg.meteofrance.com/geoservices/Radar-mapcache-WMS` | WMS 原图已留证；物理色标待验收 |
| `id` | 印尼 · BMKG legacy 产品 | `radar.bmkg.go.id:8090/sidarmaimage` | 需有效授权；当前在线能力未完整验收 |
| `id_sidarma` | 印尼 · BMKG SIDARMA CMAX | `api.bmkg.go.id/radar/v1/arsip` | 需用户 API key；原图获取与科学能力分开验收 |
| `kr` | 韩国 · KMA | `radar.kma.go.kr`，官方 CGI／时间查询 | 已有原始获取证据；科学能力待验收 |
| `my` | 马来西亚 · METMalaysia | `www.met.gov.my/data/radar_peninsular.gif`、`radar_east.gif` | 已有临时原图获取证据；授权、时次和几何缺口见下文 |
| `nz` | 新西兰 · MetService | `mobile-apps.metservice.com`，时间线／图像 | 已有原始获取证据；科学能力待验收 |
| `opensnow` | 第三方 OpenSnow 产品；底层观测归属未确认 | 旧 `opensnow.com/tiles/rvp/v2/radar/` 路由 | 原始证据与访问条件未齐；原生适配器保持受限 |
| `ph` | 菲律宾 · PAGASA 产品 | `www.panahon.gov.ph`，本地 Chromium 会话 | 需系统 Chromium 与网络许可；占位图和真实帧应区分 |
| `pt` | 葡萄牙 · IPMA Madeira 产品 | `www.ipma.pt/resources.www/transf/radar/`，官方时间线／图像 | 有序降雨强度类别有证据，不提供精确逐像素雨强 |
| `rainviewer` | 第三方 RainViewer 产品；底层观测机构按覆盖核实 | `api.rainviewer.com/public/weather-maps.json`、`tilecache.rainviewer.com` | 时间线／瓦片有证据；其产品生成时间不等同单站扫描时间 |
| `rdcap` | 台湾、日本、菲律宾 · CWA RDCAP 分发；观测机构按站点记录 | `rdcap.cwa.gov.tw`，目录／索引与临时文件票据 | 25 来源中新增的多国来源；目录不等于当前有数据，三国 live 能力仍标未验证 |
| `sg` | 新加坡 · NEA／MSS | `www.weather.gov.sg/weather-rain-area-240km`；另有 `api-open.data.gov.sg` 对照 API | 当前原图渠道与参考 API 分开记录授权；仅有序强度类别 |
| `th` | 泰国 · TMD | `weather.tmd.go.th`，官方 GIF | 读取图内时间；过期样本不可当最新降雨 |
| `th_royalrain` | 泰国 · Royal Rainmaking | `file.royalrain.go.th/opendata/radar_data/cappi` | 既有成功与失败记录；当前可达性需独立验证 |
| `tw` | 台湾 · CWA observation／grid | `cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation`，官方匿名对象存储 | 数值 grid 与 observation PNG 的科学／几何能力不同 |
| `tw-http` | 台湾 · CWA observation | `www.cwa.gov.tw/Data/js/obs_img/Observe_radar.js`、`/Data/radar/` | 官方 HTTP 原图；科学与像素定位待验收 |
| `uk` | 英国 · Met Office DataPoint | 退役渠道，原生发现先拒绝联网 | 不可在线获取；不作为默认来源 |
| `vn` | 越南 · Hymetnet CMAX | `hymetnet.gov.vn`，官方页面／图像 | 已有原始获取证据；科学能力待验收 |
| `windy` | 第三方 Windy 产品；底层观测归属未确认 | `rdr.windy.com`，瓦片；可选本地 Chromium | 已有原始瓦片证据；时间假设与物理色标待验收 |
| `wunderground` | 第三方 Weather Underground／Weather.com 产品；底层观测归属未确认 | `api0.weather.com/v3/TileServer/tile` | 需用户 API key；原生 raw 获取在证据齐备前受限 |

RDCAP 在同一 Engine 生命周期内保留上游设置的匿名会话 cookie，供目录、索引刷新及文件票据读取复用；仅 RDCAP HTTPS 主机使用此内存会话，不持久化 cookie，也不写入公开报告。RDCAP 默认验证 HTTPS 证书。遇到本站证书链验证失败时，可显式配置 `sources.rdcap.insecure_tls: true`；该例外仅用于 RDCAP adapter 对 `https://rdcap.cwa.gov.tw` 的目录、时间索引及文件请求，其他来源及其他主机仍正常校验。联网 opt-in、请求限额、超时、取消和票据重试规则保持不变。关闭校验会失去服务器身份验证，仅作为用户主动选择的访问方式，不代表标准 TLS 路径或三国科学能力已验收。 [2026-10-07 单站发现对照](validation-results/rdcap-tls-opt-in-20261007.json)记录了安装后的 Rust-backed CLI：默认校验时失败，显式启用例外后取得花莲最新帧引用；本次未验证 raw、science 或独立读回。 后续[同日 MAKI 会话验证](validation-results/rdcap-session-live-20261007.json)在禁用缓存后成功获取原始响应并生成 `--decoded` / `--dbz` 数值预览；它仅覆盖该站，不代表三国科学能力或独立输出读回全部验收。

端点的实际参数、主机白名单和访问行为以[原生 adapter](crates/radiust-core/src/source/)为准。RDCAP 的观测机构记录见[站点目录](python/radiust/resources/catalog.json)，研究与实施范围见 [004 spec](specs/004-rdcap-single-station/spec.md)。目录里的 `available` 或 `needs_configuration` 不替代逐国 live 验收、运行状态或权限判断。

## 历史获取与科学验证记录

以下保留原 README 的 **2026-09-21 验证快照**，涵盖当时迁移范围内的 26 条来源记录（含两条巴西历史来源），不包含后来新增的 RDCAP。这不是当前原生目录数量或实时可用性声明，也不替代上文的渠道记录与下文的权限核实。

“在线原始获取”描述当时留有证据的检查，不保证上游此后持续可用；“科学数据”仅指有证据支持的解码能力。适配器、离线 fixture 或图像下载成功不等于科学验收或再利用授权。

<details>
<summary>展开逐源获取与科学能力快照（2026-09-21）</summary>

| 数据源 ID | 提供方 / 数据 | 在线原始获取 | 科学数据支持 / 当前限制 |
| --- | --- | --- | --- |
| `au` | 澳大利亚 BoM 雷达 | 已实测（FTP） | 原始资料；色标与原生几何待验证 |
| `bmkg` | 印尼 BMKG 瓦片 | 上游 HTTP 403 | 原始获取受阻；科学解码未验收 |
| `br_cptec` | 巴西 CPTEC WMS（历史来源） | 历史帧已实测；最新帧返回上游错误 | 原始图像；物理色标与几何待验证 |
| `br_sipam` | 巴西 SIPAM（历史来源） | 已实测 | 原始图像；物理色标与像素定位待验证 |
| `ca` | 加拿大 ECCC | 已实测 | 原始资料；色标与原生几何待验证 |
| `cam` | 柬埔寨雷达 | 已实测 | 原始图像；科学解码与几何待验证 |
| `es` | 西班牙 AEMET | 已实测 | 原始资料；色标与原生几何待验证 |
| `fr` | 法国 Météo-France WMS | 已实测 | 原始图像；颜色到 dBZ 的映射待验证 |
| `id` | 印尼 BMKG 雷达 | 需有效授权，未完成在线验收 | 原始获取受限；科学解码未验收 |
| `id_sidarma` | 印尼 SIDARMA CMAX | 需 API Key；新 archive 接口与 JAK 原图已实测成功 | 支持 latest 原图；科学解码与像素几何未验收 |
| `kr` | 韩国 KMA | 已实测 | 原始资料；色标与原生几何待验证 |
| `my` | 马来西亚气象局 | 已实测 | 原始图像；时次、色标、几何与再利用许可待验证 |
| `nz` | 新西兰 MetService | 已实测 | 原始资料；色标与原生几何待验证 |
| `opensnow` | OpenSnow 瓦片 | 上游 HTTP 403；需授权访问 | 原始获取受阻；科学解码未验收 |
| `ph` | 菲律宾 PAGASA | 自动会话 / 系统 Chromium | timeline 已恢复；原始图返回占位 PNG，科学解码未验收 |
| `pt` | 葡萄牙 IPMA | 已实测 | **支持有序降雨强度类别**；不提供逐像素数值雨强 |
| `rainviewer` | RainViewer 瓦片 | 已实测 | **支持经提供方色表验证的 dBZ 解码** |
| `sg` | 新加坡 NEA | 已实测并对照官方 API | **支持有序降雨强度类别**；不提供定量 mm/h |
| `th` | 泰国 TMD | 已实测 | 原始 GIF；时次、色标与几何待验证 |
| `th_royalrain` | 泰国 Royal Rainmaking | 曾实测；近期请求失败 | 原始资料；色标与原生几何待验证 |
| `tw` | 台湾 CWA | 已实测 | **`grid` 支持原生 TWD67 数值 dBZ**；`observation` PNG 仅原始获取，色标与像素定位待验证 |
| `tw-http` | 台湾 CWA HTTP 图片 | 已实测 | 原始图像；色标与原生几何待验证 |
| `uk` | 英国 Met Office DataPoint | 上游已停运（保留例外） | 不支持在线获取 |
| `vn` | 越南 Hymetnet CMAX | 已实测 | 原始资料；色标与原生几何待验证 |
| `windy` | Windy 瓦片 | 已实测 | 原始瓦片；物理色标及时次绑定待验证 |
| `wunderground` | Weather Underground 瓦片 | 需有效 API key，未完成在线验收 | 原始获取受限；科学解码及时次绑定未验收 |

</details>

新增 RDCAP 的当前能力标记见[目录资源](python/radiust/resources/catalog.json)，范围与验收要求见 [004 spec](specs/004-rdcap-single-station/spec.md)。台湾 CWA 的数值网格与 observation PNG 产品区别见[产品说明](docs/cwa-radar-products.md)。

## 逐源授权与署名台账

以下“署名”列记录已知条件或待核实项。无论上游是否已确认强制署名，Radiust 的产品原则都要求保留真实出处；这个项目原则不代表我们能够替上游授予使用权。

多数 fixture 的 `license_basis` 只记录迁移验证用途。旧本地样本获准用于某次验证，不意味着可对外分发整个数据产品。未核实的条款 URL、适用范围、商用或再分发条件均保留为未确认。

| ID | 许可／条款依据与署名 | 商用 | 再分发 | 本地证据／待核实项 |
| --- | --- | --- | --- | --- |
| `au` | 条款与强制署名未确认 | 未确认 | 未确认 | [fixture 依据](tests/fixtures/sources/au/fixture.json)仅涉及迁移验证 |
| `bmkg` | 条款与强制署名未确认 | 未确认 | 未确认 | [获取阻塞记录](migration/sources/bmkg.json)；无可据此授权的原图证据 |
| `ca` | 条款与强制署名未确认 | 未确认 | 未确认 | [fixture 依据](tests/fixtures/sources/ca/fixture.json)仅涉及迁移验证 |
| `cam` | 条款与强制署名未确认 | 未确认 | 未确认 | [fixture 记录](tests/fixtures/sources/cam/fixture.json)说明未保留已授权 canonical raw |
| `es` | 已记录引用 AEMET 作者的再利用依据；完整条件及适用渠道待核实 | 未确认 | 有许可依据，范围未确认 | [fixture](tests/fixtures/sources/es/fixture.json)；[官方雷达页](https://www.aemet.es/es/eltiempo/observacion/radar) |
| `fr` | 条款与强制署名未确认 | 未确认 | 未确认 | [fixture 依据](tests/fixtures/sources/fr/fixture.json)仅涉及迁移验证 |
| `id` | 条款与强制署名未确认；访问另需授权 | 未确认 | 未确认 | [fixture](tests/fixtures/sources/id/fixture.json)仅为本地迁移证据 |
| `id_sidarma` | API 访问授权不等于数据再利用许可；署名未确认 | 未确认 | 未确认 | [fixture](tests/fixtures/sources/id_sidarma/fixture.json)、[迁移限制](migration/sources/id_sidarma.json) |
| `kr` | 条款与强制署名未确认 | 未确认 | 未确认 | [fixture 依据](tests/fixtures/sources/kr/fixture.json)仅涉及迁移验证 |
| `my` | 官网内容的复制、展示等用途要求事先书面同意；未建立本项目授权 | 需书面同意，未确认取得 | 需书面同意，未确认取得 | [版权声明](https://www.met.gov.my/en/info/kenyataan-hak-cipta/)于 2026-10-06 核对；[本地 blocker](migration/blockers/my.md) |
| `nz` | 条款与强制署名未确认 | 未确认 | 未确认 | [fixture 依据](tests/fixtures/sources/nz/fixture.json)仅涉及迁移验证 |
| `opensnow` | 条款、访问权与强制署名未确认 | 未确认 | 未确认 | [阻塞记录](migration/sources/opensnow.json) |
| `ph` | 条款与强制署名未确认；自动会话不构成授权 | 未确认 | 未确认 | [既有迁移记录](migration/sources/ph.json)不能替代当前授权核实 |
| `pt` | 条款与强制署名未确认 | 未确认 | 未确认 | [fixture 依据](tests/fixtures/sources/pt/fixture.json)仅涉及迁移验证 |
| `rainviewer` | 公开 API 与色表不是商业授权结论；条款和强制署名未确认 | 未确认 | 未确认 | [fixture](tests/fixtures/sources/rainviewer/fixture.json)、[产品渠道记录](migration/sources/rainviewer.json) |
| `rdcap` | 各国观测、产品与分发条款及强制署名未确认 | 未确认 | 未确认 | [004 规格](specs/004-rdcap-single-station/spec.md)、[研究记录](docs/rdcap-single-station-analysis.md)不构成许可 |
| `sg` | 参考 data.gov.sg API 有 Singapore Open Data Licence 依据；当前原页面图像的适用许可与强制署名仍待核实 | 当前渠道未确认 | 当前渠道未确认 | [fixture 区分](tests/fixtures/sources/sg/fixture.json)；[参考 API 许可](https://data.gov.sg/open-data-licence)于 2026-10-06 核对，不能自动套用到另一渠道 |
| `th` | 条款与强制署名未确认 | 未确认 | 未确认 | [fixture](tests/fixtures/sources/th/fixture.json)、[时间与过期说明](docs/tmd-source.md) |
| `th_royalrain` | 路径名含 opendata 不替代许可文本；强制署名未确认 | 未确认 | 未确认 | [fixture 依据](tests/fixtures/sources/th_royalrain/fixture.json)仅涉及迁移验证 |
| `tw` | observation 与 grid 的适用条款、强制署名分别待核实 | 未确认 | 未确认 | [fixture](tests/fixtures/sources/tw/fixture.json)、[产品区别](docs/cwa-radar-products.md) |
| `tw-http` | HTTP 原图适用条款与强制署名未确认 | 未确认 | 未确认 | [fixture 依据](tests/fixtures/sources/tw-http/fixture.json)仅涉及迁移验证 |
| `uk` | 历史 DataPoint 条款与署名未确认；不因退役改变旧授权 | 未确认 | 未确认 | [退役记录](migration/sources/uk.json)，无在线支持声明 |
| `vn` | 条款与强制署名未确认 | 未确认 | 未确认 | [fixture 依据](tests/fixtures/sources/vn/fixture.json)仅涉及迁移验证 |
| `windy` | 条款与强制署名未确认 | 未确认 | 未确认 | [fixture 依据](tests/fixtures/sources/windy/fixture.json)仅涉及迁移验证 |
| `wunderground` | API key 不等于所有产品使用权；条款与强制署名未确认 | 未确认 | 未确认 | [阻塞记录](migration/sources/wunderground.json) |

## 两条历史巴西来源

这两条记录属于历史迁移范围，未注册到当前原生目录，不能据此承诺 Desktop 已覆盖巴西。后续接入仍需适配、访问与权限依据、时间／几何验证和产品级验收。

| ID | 地区／提供方与渠道 | 访问记录 | 许可与署名 | 商用 | 再分发 | 依据 |
| --- | --- | --- | --- | --- | --- | --- |
| `br_cptec` | 巴西 · CPTEC／INPE；`data.inpe.br/big/geoserver/dissm/wms` | 既有历史帧成功、最新帧上游错误记录 | 条款与强制署名未确认 | 未确认 | 未确认 | [迁移记录](migration/sources/br_cptec.json)、[fixture](tests/fixtures/sources/br_cptec/fixture.json) |
| `br_sipam` | 巴西 · SIPAM；`apihidro.sipam.gov.br/radares/`、`siger.sipam.gov.br/radar/` | 既有原始获取记录；科学与几何待验证 | 条款与强制署名未确认 | 未确认 | 未确认 | [迁移记录](migration/sources/br_sipam.json)、[fixture](tests/fixtures/sources/br_sipam/fixture.json) |

## 更新与责任

新增来源或改变访问渠道、产品和条款时，同步更新本台账、目录、来源能力与验收记录。每次权限核实记录对应的条款 URL、日期、产品、渠道、用途及署名要求；尚未核实的保持未确认，不从同一机构的其他产品继承授权。已记录限制不得被仅用于验证的样本许可覆盖。

商业使用者负责核实其具体用途所需权限，并自行承担其对客户作出的额外承诺；Radiust 不代上游授权，也不自动为下游产品提供支持、担保或赔偿。项目自身也需审查自己的获取、保留和分发行为。软件的保证排除和责任限制以 [Apache-2.0 原文](https://www.apache.org/licenses/LICENSE-2.0.html)及适用法律为准，不能保证免于诉讼。

本地直连是默认架构，不意味着上游访问、版权或再利用条件消失。若未来运营数据代理、公共缓存或再分发服务，需要按新增的实际行为单独核实条款，而不能沿用客户端使用的结论。
