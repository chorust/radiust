# CWA 雷达产品及证据边界

中央气象署整合雷达回波资料说明的「产品取得」章节列有以下公开资料集。不同产品的像素定位、色标或物理值需要分别验证。

| CWA 资料集编号 | 官方说明中的产品 | radiust 当前入口 |
| --- | --- | --- |
| `O-A0059-001` | 整合回波资料（数值） | `tw --product grid`，原生 dBZ 数值解码 |
| `O-A0058-001` | 较大范围／无地形回波图 | `tw --product observation-wide`；会员授权渠道，支持原图与 raw-only，科学解码及像元几何未准入 |
| `O-A0058-002` | 较大范围／有地形回波图 | 尚未接入 |
| `O-A0058-003` | 邻近区域／无地形回波图 | 尚未接入 |
| `O-A0058-004` | 邻近区域／有地形回波图 | 尚未接入 |
| `O-A0058-005` | 较大范围透明回波图 | `tw --product observation`；`--raw-only` 保存 PNG 和配套 JSON，`cat --gray` / `cat --dbz` 使用通过的历史灰度规则 |
| `O-A0058-006` | 邻近区域透明回波图 | 尚未接入 |

CWA 说明将 PNG 描述为「依據 xml 資料所製之圖檔」。已留存的 `O-A0058-005.json` 明确给出 `LongitudeRange=115.00-126.50`、`LatitudeRange=17.75-29.25`、`ImageDimension=3600x3600`、`ProductURL` 和 `DateTime`。适配器把这些值作为**提供者声明的图像范围及尺寸**保存于 `FrameRef.metadata`，并在下载时核对 JSON 产品身份、URL、有效时间、范围和实际图像尺寸。它们不足以独立验证像素中心/边界注册、地理基准面及 RGB→dBZ 映射；PNG 的独立物理色标与地理定位验证仍未完成。Spec 005 支持匹配通过规则后的 gray→dBZ 编码反算：`radiust cat tw --product observation --latest --dbz` 和 `download tw --product observation --latest --dbz` 使用该路径，结果为无可信地理定位的像素 dBZ。共享 Core 选择唯一图像 artifact，校验配套 JSON 的 receipt 并保留完整 acquisition receipt；PNG+JSON 路径已有离线回归，见[验证记录](../validation-results/gray-dbz-companions.json)。这不升级为独立物理标定或在线 dBZ 验收。`--decoded` 继续要求原生数值产品，TW 应选择 `--product grid`。如果上游在 PNG 与 JSON 的两次请求之间更新图像且最终 JSON 时间不变，仅凭这些公开资料仍无法证明二者为同一版本。

`O-A0059-001` 是独立的 921×881 数值网格：已留存原始 JSON 声明 TWD67（EPSG:3821）、0.0125 度、由西南角先向东后向北排列，`-99` 与 `-999` 具有不同的缺测含义。它的网格不能套用到 3600×3600 PNG，PNG 的声明范围也不能替代数值网格的坐标系。

`tw --product grid` 的实际下载地址为 <https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation/O-A0059-001.json>。2026-09-30 实测该地址仍返回 HTTP 200，但约 8.9 MB 的完整响应在慢速连接下会超过默认请求超时。发现阶段现在只读取最多 64 KiB 的文件头；获取阶段使用最多四路、每块 256 KiB 的 HTTP Range 请求，逐块核对 Content-Range、长度和 ETag，并用 If-Match 绑定同一版本。最终仍保存完整原始 JSON、验证时间和几何，再解码原生 dBZ 网格。

公开资料目录：<https://opendata.cwa.gov.tw/dataset/observation/O-A0059-001> 以及 `O-A0058-001` 至 `O-A0058-006` 对应的 observation 资料集页面。图像产品列表来自提供的 CWA 文件截图；本仓库留存的可重放原始资料与时次验证仅覆盖 `O-A0058-005` 和 `O-A0059-001`。

`tw-http --product observation --raw-only` 使用 CWA 雷达回波页面 <https://www.cwa.gov.tw/V8/C/W/OBS_Radar.html>，并将该页面作为索引和图片请求的 Referer。页面通过 `/Data/js/obs_img/Observe_radar.js` 提供最近时次，图片仍位于 `/Data/radar/CV1_3600_YYYYMMDDHHmm.png`；适配器选择较大范围、无地形的高解析度图像，并将台湾当地时间转换为 UTC。此网页入口支持索引内的历史时次，仍仅输出原始图像。

## 会员平台授权渠道的用户回放

2026-10-10，用户在自己设备上运行独立 Python 诊断，报告 `O-A0058-001` 元数据获取成功，产品有效时间为 `2026-10-10T13:50:00+08:00`（`05:50Z`）；元数据中的 `ProductURL` 通过 HTTPS 及预期 CWA S3 主机检查，图片入口可读取，前 8 字节符合 PNG 文件头。结果见[用户回放记录](../validation-results/cwa-o-a0058-001-user-replay-20261010.json)。此前使用 `v2` 的尝试返回 404，未记录失败阶段；按[当前官方使用说明](https://opendata.cwa.gov.tw/devManual/instruction)改用 `https://opendata.cwa.gov.tw/fileapi/v1/opendataapi/O-A0058-001`，通过 `Authorization` 查询参数提供用户自己的会员授权码，选择 `format=JSON` 后，用户报告成功。公开记录只保留无凭据的入口，不保存授权码或带码 URL。

授权码由通用配置层加载为 `sources.tw.api_key`，对应环境变量为 `RADIUST_SOURCES__TW__API_KEY`；环境变量优先于配置文件，CLI／SDK 的配置展示会脱敏。`tw --product observation-wide` 读取该配置，通过会员平台获取 `O-A0058-001` 元数据。`observation` 与 `grid` 保留匿名 S3 渠道及原有含义；未配置授权码时，只将 `observation-wide` 报告为 `missing_credentials`，不影响这两个匿名产品。产品默认值仍为 `observation`，不会自动切换来源或渠道。

这份用户回放记录只证明当时的元数据及 PNG 文件头检查，没有留存原始响应、完整图像或哈希，也没有取得准确获取时间。后续适配器及科学验收证据分别见下节；会员 API 访问成功不替代商用、再分发及署名条件核实。

## 授权适配器与科学验收（2026-10-10）

适配器复用共享 Engine、HTTP 请求预算、网络 opt-in、超时、取消和有界重试。会员元数据请求只允许重定向到同一请求地址或该产品的官方 S3 JSON 精确地址；图片使用原有同源、字节有界的获取机制。它校验 `dataid`、`ProductURL`、MIME、产品有效时间、声明范围和实际 PNG 尺寸，并在获取图片前后读取会员元数据，拒绝时次／范围／尺寸变化。公开 locator、receipt、缓存和报告只记录无凭据地址；授权码不进入帧身份。

本机配置示例（文件可放在 `~/.config/radiust/config.yaml`，不要提交真实授权码）：

```yaml
sources:
  tw:
    api_key: "你的 CWA 会员授权码"
```

源码中新接入的产品需要更新实际运行的 CLI。先运行 `radiust list products tw --json`，确认列表包含 `observation-wide`；旧 CLI 即使显示相同版本号，也可能没有这个产品。在仓库根更新 Cargo 安装可执行 `cargo install --offline --locked --path crates/radiust-cli --force`；也可直接使用当前构建的 `target/debug/radiust` 或已更新的 `.venv/bin/radiust`。CLI 与 Python SDK 的安装分别维护，更新 Cargo 二进制不更新 Python 扩展。

已有 Python 可编辑安装时，源码改动不会自动重编 Rust 扩展。需在实际使用的仓库及虚拟环境中原位重建，再用 `.venv/bin/radiust list products tw --json` 检查。下列命令显式选择 `.venv`，并避免同时激活的 Conda 环境干扰；`--skip-install` 保留已有可编辑安装与依赖，仅更新原生扩展：

```bash
env -u CONDA_PREFIX VIRTUAL_ENV="$PWD/.venv" \
  .venv/bin/maturin develop --release --offline --skip-install
```

2026-10-10，PATH 中旧 Cargo CLI 的该产品目录缺失，`cat` 返回 `source returned no matching frame`。更新同路径二进制后，原命令成功取得 `06:30Z` 的 3600×3600 PNG 原图；配置无需修改，见[安装更新回放](../validation-results/cwa-cli-installed-refresh-20261010.json)。

主仓库 Python 3.12 的可编辑安装也已单独修复：保留现有位置视图源码、合入 CWA 接入补丁并原位重建扩展后，同目录的 `.venv/bin/radiust cat tw --product observation-wide --latest --raw` 取得 `06:50Z` 原图；36 项 Python 合同、17 项位置视图 Core flow 及相关 Rust 回归通过。见[主仓库可编辑安装回放](../validation-results/cwa-primary-editable-refresh-20261010.json)。

显式开启网络后查看原图或保存原始 PNG 与 JSON：

```bash
RADIUST_RUNTIME__ALLOW_NETWORK=true radiust cat tw --product observation-wide --latest --raw
RADIUST_RUNTIME__ALLOW_NETWORK=true radiust download tw --product observation-wide --latest --raw-only
```

CLI、同步 SDK、异步 SDK 在 8.7 秒内取得同一条 `2026-10-10T06:10Z` 帧，PNG 与 JSON receipt 哈希均相同；每份 3600×3600 PNG 通过 Pillow 完整解码。具体结果见[适配器验收记录](../validation-results/cwa-membership-acceptance-20261010.json)。数据保留在本机临时输出目录，未作为仓库 fixture 或产品数据分发。使用[三入口回放脚本](../scripts/validation/validate_cwa_membership.py)可复验；它要求 `--live`、当前用户授权码及已重建的 CLI／Python 扩展：

```bash
.venv/bin/python scripts/validation/validate_cwa_membership.py --live \
  --output /tmp/radiust-cwa-replay --report /tmp/radiust-cwa-replay.json
```

回放脚本通过显式异常检查产品身份、原始完成状态、artifact 清单、字节数、SHA256、资料集、有效时间、图像尺寸和三入口下载报告；这些检查在 `python -O` 下仍然执行。离线优化模式回归覆盖损坏清单和失败报告，但不构成新的真实上游验收。

`O-A0058-001` 仍为原图产品：本次图像包含标题、网格线、署徽、站点指示及 dBZ 图例。产品声明范围和 3600×3600 尺寸不足以确认像元中心／边界注册、datum、RGB 对应的精确值及注记掩膜。仅观察图例不建立精确 dBZ 解码；背景颜色也不能自动区分无回波、缺测和无覆盖。因此 `--dbz`／原生科学解码仍失败关闭，没有继承 `O-A0058-005` 的历史灰度规则，也没有应用 `O-A0059-001` 的几何。下载前后元数据一致仍不足以证明可变 PNG 与 JSON 为同一上游版本，实际覆盖及默认来源准入保持未验证。

独立数值产品 `tw --product grid` 的一次在线原生科学读回通过：`2026-10-10T06:20Z`，881×921、811401 个网格值、质量标志与 x/y 坐标逐项对照原始 `O-A0059-001` JSON，单位 dBZ、原生 TWD67（EPSG:3821）。结果及哈希见[数值读回记录](../validation-results/cwa-grid-native-readback-20261010.json)，可用[离线独立核对脚本](../scripts/validation/readback_cwa_grid.py)重放：

```bash
.venv/bin/python scripts/validation/readback_cwa_grid.py \
  /path/to/raw/O-A0059-001.json /path/to/decoded.nc --report /tmp/cwa-grid-readback.json
```

这只验收该次数值产品读回，不升级图片产品或其他时次，也不替代仪器物理标定、实际覆盖或三入口数值回放；TWD67→WGS84 转换仍不支持。

相关 Rust、CLI、Python 回归及检查结果见[离线验证记录](../validation-results/cwa-membership-offline-20261010.json)。默认测试禁止公网；三入口 live 回放与独立数值读回分别记录，不把合成 fixture 或 loopback 检查计为上游验收。
