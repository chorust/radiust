# RDCAP 台湾、日本、菲律宾单站雷达请求与解码分析

实测日期：2026-10-01，约 14:03–14:30，台湾标准时间 UTC+8。使用用户指定的 orca-cli 内置浏览器读取页面、运行时状态及公开网络请求，并独立解码实际文件。这里只分析 RDCAP 的公开显示产品；没有接入 radiust 的生产适配器。

## 结论

三国共享一套接口及数值绘图协议：**POST 获取时间索引 → GET 签名文件票据 → JSON 字符串中的 CSV → CRS 稀疏矩阵 → dBZ → 分级着色 → Leaflet 地图图层**。单站回波并非只能获取预先渲染的 PNG，也不是浏览器直接读取雷达极坐标体扫数据。公开文件已经是 EPSG:4326 经纬度网格。

地图入口：[radar_map](https://rdcap.cwa.gov.tw/data_access/radar_map)。代表性单站：[花莲](https://rdcap.cwa.gov.tw/data_access/radar_display/TWN/RCHL)、[石垣岛](https://rdcap.cwa.gov.tw/data_access/radar_display/JPN/ISHI)、[苏比克](https://rdcap.cwa.gov.tw/data_access/radar_display/PHL/SUBI)。

| 国家 | 网站代码 | 目录记录 / 唯一站码 | 当前有回波索引的唯一站点 |
| --- | --- | --- | --- |
| 台湾 | TWN | 13 / 13 | 9 |
| 日本 | JPN | 20 / 20 | 19 |
| 菲律宾 | PHL | 16 / 15 | 3 |

菲律宾目录出现两个 `BALE` 记录，状态分别为 Inactive、Active；URL 只用站码，所以检查时按站码去重。`Active` 是目录字段，不能当作实况可用性：例如 APAR 标为 Active，但此时 `get_radar_data` 返回 HTTP 400 `['No Radar Data']`。

## 1. 请求如何构建

基址 `https://rdcap.cwa.gov.tw`。Ajax 接口需要 `X-Requested-With: XMLHttpRequest`；实测不带该头返回 `No direct script access allowed`。POST 使用表单编码，不是 JSON 请求体。

| 接口 | 方法与参数 | 用途 |
| --- | --- | --- |
| `/data_access/get_country_list` | POST，空表单 | 国家名称到 TWN/JPN/PHL 的映射及地图中心 |
| `/data_access/get_radar_list` | POST，空表单 | 完整站点目录、站码、坐标、状态等 |
| 同上 | POST，`radar_name[]=RCHL` | 单站元信息；这是页面 jQuery 数组参数的实际编码 |
| `/data_access/get_radar_data` | POST，`country=TWN&radar_name=RCHL&datetime=` | 单站回波时间线、网格头链接、各帧文件链接 |
| `/file?ft=<返回的票据>` | GET | 文件内容，返回 JSON 编码的 CSV 字符串 |

最小索引请求已在浏览器之外用 curl 验证，无需用户登录、API key 或目录内的站点 token：

```sh
curl --compressed 'https://rdcap.cwa.gov.tw/data_access/get_radar_data' \
  -H 'X-Requested-With: XMLHttpRequest' \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data 'country=TWN&radar_name=RCHL&datetime=' \
  -o index.json
```

本机无法验证该站的证书链，实测 curl 添加 `--insecure` 后取得索引。浏览器访问正常。这是本次环境记录，示例默认仍启用证书验证。

响应结构示意：

```json
{
  "header": "https://rdcap.cwa.gov.tw/file?ft=<header-ticket>",
  "list": [
    {"key": "1790834708000", "url": ["https://rdcap.cwa.gov.tw/file?ft=<frame-ticket>"]}
  ]
}
```

`header` 是三行网格头；回波帧 `url` 是长度为 1 的数组，帧文件自身也包含三行头。下载帧即可获得完整解码信息，不必重复下载 header。

```python
import json
import subprocess

index = json.load(open('index.json'))
frame = max(index['list'], key=lambda item: int(item['key']))
# subprocess 参数数组避免票据 URL 中的 & 被 shell 解释。
body = subprocess.check_output([
    'curl', '--compressed', '--fail', '--silent', '--show-error',
    frame['url'][0],
])
open('file-response.json', 'wb').write(body)  # 先留存首读响应
csv = json.loads(body)                       # JSON 字符串解包一次
if not isinstance(csv, str) or not csv:
    raise ValueError('票据无有效内容；重新获取索引后重试')
open('frame.csv', 'w').write(csv)
```

**文件票据实测具有一次读取行为**：同一个新 header URL，第一次 GET 返回非空字符串，紧接着第二次 GET 返回 HTTP 200 的 `""`。对已经由页面读过的 header/帧 URL，再读也得到 `""`。因此不要先 HEAD/预读，再拿同一票据下载；读取成功后缓存原始响应，失败重试应重新申请索引。不硬编码、修改或自行生成 `ft`。JWT 外层可见 HS256、`iat`、audience 等信息，但文件部分是不透明内容；没有证据支持固定有效期、跨会话可复用性或自行拼文件路径。

传输验证边界：Orca 浏览器中三国的索引及文件链均成功；独立 curl 的索引请求成功。独立 Python/urllib 连接被上游关闭，改用 curl 后 PHL/SUBI 的文件 GET 又发生 40 秒超时，所以本报告不声称浏览器外的文件下载已端到端验证。脚本保留原始响应并显式报错；浏览器路径和本地解码可重放。

## 2. 时间线

`list[].key` 是 Unix 毫秒时间戳，含秒级扫描时间。例如 `1790834708000` 对应 `2026-10-01T06:05:08Z`，页面显示台湾时间 `14:05`。页面只显示 HH:mm，采集时必须保留原始毫秒键，不能据显示文本重建时刻。国家为日本时，网页仍采用本站参考时间 UTC+8；接口时间键本身不受显示时区影响。

单站脚本把 `key` 转成 Number 形成时间条；索引按时间升序返回，默认选最后一帧。第一次只加载所选帧，点击/播放旧时刻时按需下载；已有帧缓存直接切换，正在加载时播放等待。轮播默认 1500 ms，界面可选 0.5–2.5 秒；这是动画间距，与雷达扫描周期无关。脚本未见单站时间索引的定时刷新逻辑；持续采集需要主动再次请求索引。

| 样本 | 帧数及时间范围（UTC+8） | 实测相邻帧间距 |
| --- | --- | --- |
| TWN/RCHL | 21 帧，12:03:54–14:05:08 | 363–365 秒左右，约 6 分钟 |
| JPN/ISHI | 12 帧，12:07:45–13:57:48 | 约 600 秒 |
| PHL/SUBI | 10 帧，12:10:10–13:40:10 | 600 秒 |

最新接口覆盖约两小时；没有固定帧数。台湾不同站点可能约 6、7.5 或 10 分钟，日本 AKIT/MAKI/SAPP 还出现相隔 15–17 秒的邻近时刻，SEFU 有长缺口。应使用实际返回列表，不能按整分钟或统一步长猜 URL。菲律宾 SUBI 的最新帧落后检查时间约半小时；DAET 只有 1 帧。

`datetime` 默认空字符串时为近期索引。非空参数触发历史查询分支，但本次未验证成功的历史日期语法或保留期：12/14 位日期数字、Unix 毫秒及日期字符串得到 `No History Radar Data`；带空格的日期时间返回 HTTP 200 的 HTML `Request Rejected`，不是 JSON。不要将这些试验当成“历史资料不存在”或已确认的日期格式。处理接口响应需同时验证 HTTP 状态、Content-Type 和 JSON 类型。

## 3. CSV、稀疏矩阵与 dBZ

花莲完整帧前三行：

```text
901,901,T,117.12,19.49,126.12,28.49,int16,-999,-999,EPSG:4326
linearTransform(0.1000,0.0000)
99,82,115,50,115,99,132,100,...,255,255,255,750
```

第一行依次为：宽、高、像元定位方式、起始经度、起始纬度、末端经度、末端纬度、数值类型、默认填充值、无效值、坐标系。第二行明确给出 `physical = raw × 0.1 + 0`，这里的物理值为 dBZ。`-999` 必须先作为缺测识别，不能当成 `-99.9 dBZ`。低于 5 dBZ 的真实有效值可存在（例如 -1.5），只是在网页色标下透明。

第三行是重复的 `(R,G,B,raw_lower_threshold)` 四元组；阈值同样要应用值变换。例如 `(255,255,0,350)` 对应 35 dBZ 黄色。网站原始值先分类，再转换取值。

之后是四行 CRS（Compressed Row Storage，也称 CSR）稀疏矩阵：

```text
ndv:4718
colIdx:424,425,...
rowPtr:0,0,...,4718
vals:9999,9999,...
```

此处 `ndv` 是**保存的元素数量**，不是 NoDataValue。`len(colIdx)=len(vals)=ndv`，`len(rowPtr)=height+1`，`rowPtr[0]=0`，`rowPtr[-1]=ndv`，列索引从 0 起。

```python
raw = np.full((height, width), default_raw, dtype=np.int16)
for r in range(height):
    for k in range(rowPtr[r], rowPtr[r + 1]):
        raw[r, colIdx[k]] = vals[k]
if first_lat < last_lat:
    raw = raw[::-1]  # 原始文件南→北；图像/网页北→南
dbz = raw.astype(float) * scale + offset
dbz[raw == invalid_raw] = np.nan
```

三国样本中的 `9999` 有特殊用途：它们绘成白色站心/范围圆，而网页直接取值会给出 `999.9`、颜色索引 14。这一“图形标记”判断来自数值空间分布和重建图像，**不是官方定义的缺测代码**。花莲样本 4718 个保存值中，4712 个为 `9999`，真正非标记值仅 6 个；石垣岛 4419 个中 4272 个、苏比克 8061 个中 4968 个为 `9999`。用于科学回波数组时需单独排除这些标记，并保留原始 CSV，不能把范围圆当作极端降水。参考脚本在 `dbz` 数组中将 `9999` 设为 NaN，在用于复现网页的 PNG 中保留白圈。

色标为以下下限，区间采用左闭右开，低于 5 为透明，最高档 ≥75 为白色：

| dBZ | 颜色 | dBZ | 颜色 | dBZ | 颜色 |
| --- | --- | --- | --- | --- | --- |
| 5 | #635273 | 30 | #009400 | 55 | #ff0000 |
| 10 | #736384 | 35 | #ffff00 | 60 | #ce0000 |
| 15 | #9c9c9c | 40 | #e7c600 | 65 | #ff00ff |
| 20 | #00ce00 | 45 | #ff9400 | 70 | #9c31ce |
| 25 | #00ad00 | 50 | #ff6363 | 75 | #ffffff |

## 4. 地理定位：T 不能当作像元中心

网站绘图引擎 `KXDviz-e.min.js` 对 T 使用行/列索引 floor。先把纬度统一成北→南，再将东边界扩展一个格距、南边界扩展一个格距；C 则采用中心注册及半格扩展。这个区别可直接由运行时 `getHeaderInfo()`、`getDataInfo()` 和代码核实。

```text
dx = (last_lon - first_lon) / (width - 1)
dy = abs(last_lat - first_lat) / (height - 1)
west = first_lon
north = max(first_lat, last_lat)
east = west + width * dx
south = north - height * dy
geotransform = [west, dx, 0, north, 0, -dy]
pixel_center_lon(c) = west + (c + 0.5) * dx
pixel_center_lat(r) = north - (r + 0.5) * dy
```

| 样本 | 网格 | 头中端点，经度 / 纬度 | 网页实际图层边界 W,S,E,N |
| --- | --- | --- | --- |
| RCHL | 901×901 | 117.12→126.12 / 19.49→28.49 | 117.12,19.48,126.13,28.49 |
| ISHI | 900×900 | 119.69→128.68 / 19.93→28.92 | 119.69,19.92,128.69,28.92 |
| SUBI | 900×900 | 115.87→124.86 / 10.33→19.32 | 115.87,10.32,124.87,19.32 |

三者格距均为 0.01°，不是固定公里分辨率。网格原生 EPSG:4326，Leaflet 地图显示时转换到 Web Mercator。若简单把这张经纬度等间距 PNG 贴到 EPSG:3857 地图而不重投影，纬度方向会有误差；科学输出可保留 EPSG:4326 GeoTIFF/数组，地图显示则应正确重投影。

定位验证：对与网页显示完全相同的 RCHL `1790834708000` 帧，本地独立解码与网站 `pickData(lng,lat)` 的 8 个位置一致，包括 14、13.5、39、41、-1.5 dBZ 和缺测。另在 `121.355E,28.105N` 白圈像元核实网站返回 `999.9`。

这些证据确认网页网格的单位、投影声明、行方向、像元注册和显示算法；**没有确认上游从体扫到该网格的科学处理算法**，例如最低仰角 PPI、组合反射率、CAPPI、高度、质控或波束遮挡处理。网页另有“最低三层观测场”，这与默认回波产品应分别处理，不能据名称推断默认产品高度。

## 5. 相邻产品与复现文件

单站源码还有 `/data_access/get_observ_field_options`（`radar_name`），以及 `/data_access/get_radar_observ_data`（`radar_name,layer,header_type`），默认参数为 `ref`；台湾另有 `/data_access/get_single_wind_data`（`radar_name,layer`）。回波选项直接走 `get_radar_data`，不需要自己填写仰角。其他产品未在本次按三国全面验证。

源代码证据：[radar_display.js](https://rdcap.cwa.gov.tw/pages/data_access/radar_display/radar_display.js)、[radar_map.js](https://rdcap.cwa.gov.tw/pages/data_access/radar_map/radar_map.js)、[KXDviz-e.min.js](https://rdcap.cwa.gov.tw/packages/KXD-vue-map/3.0.0/KXDviz-e.min.js)。实测页面脚本版本参数为 `v=0415063121`。

本地资料：

- `scripts/validation/probe_rdcap.py`：独立索引请求、首读文件留存、CRS 解码、坐标、色标、标记排除，以及 PNG/NPZ 输出；支持已保存文件离线重放。
- `validation-results/rdcap-analysis/`：去除目录 token 的站点目录、全站时间索引摘要、三国原始 CSV 和 PNG、网格元数据、8 点网页对照和请求行为记录。

```sh
.venv/bin/python scripts/validation/probe_rdcap.py \
  --decode-file validation-results/rdcap-analysis/SUBI/frame.csv \
  --out /tmp/rdcap-subi-replay

# 在线调用：正常环境省略 --insecure；本机实测文件 GET 超时，见上方边界说明。
.venv/bin/python scripts/validation/probe_rdcap.py \
  --country JPN --station ISHI --out /tmp/rdcap-ishi --insecure
```

不要以同一票据重新请求已消费文件；不要以目录 Active 替代最新索引；不要合并所有国家的固定时间步长；不要把 9999 范围标记当作 dBZ 回波；不要对 T 网格套用中心注册。

## 附录：全部站点实测

以下全部时间按 UTC+8 显示，是一次实测快照。站点有索引不等于每帧都有降水回波。


### TWN

| 站码 | 名称 | 目录状态 | 实测帧数 | 首帧→末帧 |
| --- | --- | --- | --- | --- |
| RCWF | Wu-Fen-Shan | Active | 21 | 12:10:36→14:11:10 |
| RCMD | Maintenance Depot | Active | 0 | HTTP 400，无当前回波 |
| RCHL | Hua-Lien | Active | 21 | 12:09:58→14:11:12 |
| RCCG | Chi-Gu | Active | 21 | 12:11:22→14:11:23 |
| RCKT | Ken-Ting | Active | 21 | 12:10:07→14:11:32 |
| RCLY | Lin-Yuan | Active | 17 | 12:06:44→14:07:36 |
| RCNT | Nan-Tun | Active | 17 | 12:08:49→14:08:21 |
| RCSL | Shu-Lin | Active | 17 | 12:09:05→14:06:59 |
| RCCK | Chin-Chuan-Kang | Active | 12 | 12:09:28→13:59:30 |
| RCMK | Ma-Kung | Active | 0 | HTTP 400，无当前回波 |
| RCGI | Green Island | Active | 13 | 12:06:30→14:06:31 |
| RCAA | Taoyuan Airport | Active | 0 | HTTP 400，无当前回波 |
| RCCU | NCU | Inactive | 0 | HTTP 400，无当前回波 |

### JPN

| 站码 | 名称 | 目录状态 | 实测帧数 | 首帧→末帧 |
| --- | --- | --- | --- | --- |
| AKIT | Akita | Active | 25 | 12:07:36→14:07:42 |
| FUNC | Naze | Active | 12 | 12:09:11→13:59:11 |
| HAIG | Hiroshima | Active | 12 | 12:13:25→14:03:24 |
| HAKO | Hakodate | Active | 12 | 12:09:11→13:59:11 |
| ISHI | Ishigakijima | Active | 12 | 12:07:45→13:57:48 |
| ITOK | Okinawa | Active | 12 | 12:09:10→13:59:10 |
| KASH | Tokyo | Active | 13 | 12:09:21→14:09:19 |
| KURU | Nagano | Active | 13 | 12:08:04→14:08:09 |
| KUSH | Kushiro | Active | 12 | 12:09:16→13:59:16 |
| MAKI | Makinohara | Active | 24 | 12:07:50→13:58:17 |
| MISA | Matsue | Active | 12 | 12:09:12→13:59:12 |
| MURO | Murotomisaki | Active | 12 | 12:09:11→13:59:10 |
| NAGO | Nagoya | Active | 12 | 12:09:15→13:59:15 |
| SAPP | Sapporo | Active | 25 | 12:07:45→14:08:00 |
| SEFU | Fukuoka | Active | 6 | 12:19:19→13:59:18 |
| SEND | Sndai | Active | 12 | 12:09:23→13:59:22 |
| TAKA | Takamatsu | Active | 12 | 12:09:15→13:59:16 |
| TANE | Tanegashima | Active | 12 | 12:09:11→13:59:10 |
| TOJI | Fukui | Active | 12 | 12:09:15→13:59:16 |
| YAHI | Niigata | Active | 0 | HTTP 400，无当前回波 |

### PHL

| 站码 | 名称 | 目录状态 | 实测帧数 | 首帧→末帧 |
| --- | --- | --- | --- | --- |
| BASC | Basco | Inactive | 0 | HTTP 400，无当前回波 |
| APAR | Aparri | Active | 0 | HTTP 400，无当前回波 |
| BAGU | Baguio | Active | 12 | 12:08:25→14:00:16 |
| BALE | Baler | Inactive | 0 | HTTP 400，无当前回波 |
| SUBI | Subic | Active | 10 | 12:10:10→13:40:10 |
| TAGA | Tagaytay | Active | 0 | HTTP 400，无当前回波 |
| DAET | Daet | Active | 1 | 12:07:29→12:07:29 |
| VIRA | Virac | Active | 0 | HTTP 400，无当前回波 |
| GUIU | Guiuan | Active | 0 | HTTP 400，无当前回波 |
| ILOI | Iloilo | Active | 0 | HTTP 400，无当前回波 |
| MACT | Mactan | Active | 0 | HTTP 400，无当前回波 |
| BOHO | Bohol | Active | 0 | HTTP 400，无当前回波 |
| QUEZ | Quezon | Active | 0 | HTTP 400，无当前回波 |
| HINA | Hinatuan | Active | 0 | HTTP 400，无当前回波 |
| TAMP | Tampakan | Active | 0 | HTTP 400，无当前回波 |
