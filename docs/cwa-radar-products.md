# CWA 雷达产品及证据边界

中央气象署整合雷达回波资料说明的「产品取得」章节列有以下公开资料集。不同产品的像素定位、色标或物理值需要分别验证。

| CWA 资料集编号 | 官方说明中的产品 | radiust 当前入口 |
| --- | --- | --- |
| `O-A0059-001` | 整合回波资料（数值） | `tw --product grid`，原生 dBZ 数值解码 |
| `O-A0058-001` | 较大范围／无地形回波图 | 尚未接入 |
| `O-A0058-002` | 较大范围／有地形回波图 | 尚未接入 |
| `O-A0058-003` | 邻近区域／无地形回波图 | 尚未接入 |
| `O-A0058-004` | 邻近区域／有地形回波图 | 尚未接入 |
| `O-A0058-005` | 较大范围透明回波图 | `tw --product observation --raw-only`，保存 PNG 和配套 JSON |
| `O-A0058-006` | 邻近区域透明回波图 | 尚未接入 |

CWA 说明将 PNG 描述为「依據 xml 資料所製之圖檔」。已留存的 `O-A0058-005.json` 明确给出 `LongitudeRange=115.00-126.50`、`LatitudeRange=17.75-29.25`、`ImageDimension=3600x3600`、`ProductURL` 和 `DateTime`。适配器把这些值作为**提供者声明的图像范围及尺寸**保存于 `FrameRef.metadata`，并在下载时核对 JSON 产品身份、URL、有效时间、范围和实际图像尺寸。它们不足以独立验证像素中心/边界注册、地理基准面及 RGB→dBZ 映射；PNG 的科学解码仍阻塞。如果上游在 PNG 与 JSON 的两次请求之间更新图像且最终 JSON 时间不变，仅凭这些公开资料仍无法证明二者为同一版本。

`O-A0059-001` 是独立的 921×881 数值网格：已留存原始 JSON 声明 TWD67（EPSG:3821）、0.0125 度、由西南角先向东后向北排列，`-99` 与 `-999` 具有不同的缺测含义。它的网格不能套用到 3600×3600 PNG，PNG 的声明范围也不能替代数值网格的坐标系。

`tw --product grid` 的实际下载地址为 <https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation/O-A0059-001.json>。2026-09-30 实测该地址仍返回 HTTP 200，但约 8.9 MB 的完整响应在慢速连接下会超过默认请求超时。发现阶段现在只读取最多 64 KiB 的文件头；获取阶段使用最多四路、每块 256 KiB 的 HTTP Range 请求，逐块核对 Content-Range、长度和 ETag，并用 If-Match 绑定同一版本。最终仍保存完整原始 JSON、验证时间和几何，再解码原生 dBZ 网格。

公开资料目录：<https://opendata.cwa.gov.tw/dataset/observation/O-A0059-001> 以及 `O-A0058-001` 至 `O-A0058-006` 对应的 observation 资料集页面。图像产品列表来自提供的 CWA 文件截图；本仓库的可重放原始资料与时次验证仅覆盖 `O-A0058-005` 和 `O-A0059-001`。

`tw-http --product observation --raw-only` 使用 CWA 雷达回波页面 <https://www.cwa.gov.tw/V8/C/W/OBS_Radar.html>，并将该页面作为索引和图片请求的 Referer。页面通过 `/Data/js/obs_img/Observe_radar.js` 提供最近时次，图片仍位于 `/Data/radar/CV1_3600_YYYYMMDDHHmm.png`；适配器选择较大范围、无地形的高解析度图像，并将台湾当地时间转换为 UTC。此网页入口支持索引内的历史时次，仍仅输出原始图像。
