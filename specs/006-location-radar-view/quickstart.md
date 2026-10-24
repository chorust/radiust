# Quickstart Validation: 位置视图

**Status**: 53/53 tasks complete. T027/T036 fixed-input CLI, sync SDK and async SDK replay passed on 2026-10-17. On 2026-10-24 the installed-wheel bare `cat` selected and rendered the current RainViewer frame for personal-use `local_view`; the terminal summary shows the required credit and link. SC-001–008 are reconciled. Observation coverage remains unknown, and the admission does not cover commercial/high-volume use or redistribution. Evidence: [`006-local-view-offline.json`](../../validation-results/006-local-view-offline.json), [`006-local-view-packaging.json`](../../validation-results/006-local-view-packaging.json), and [`006-local-view-live.json`](../../validation-results/006-local-view-live.json).

## 1. 前提与构建

从仓库根目录执行。需要当前项目要求的 Rust 1.92、uv/Python 开发环境及 006 CLI/PyO3 接口。安装依赖可能需要包仓库网络；产品测试默认禁公网。构建和参考库版本、运行日期及 fixture hash 写入验收记录。

```bash
cargo build -p radiust-cli
uv sync --group dev --extra science
uv run maturin develop
RADIUST_BIN=./target/debug/radiust
```

science 的 pyproj 仅用于独立参考与显式科学导出，不进入原生投影流水线。新增 PT resource 后用现有打包检查确认镜像：

```bash
python3 scripts/release/prepare.py --check-resources
```

## 2. 离线坐标与目录验收

先验证 [projection.md](projection.md) 的中心、半径和方向。固定独立参考工具版本、WGS84 参数与结果摘要，形成离线控制点，不只用本实现往返作为正确性证明。

覆盖半径 10/50/200/500 km、四方位与斜向、日期线、高纬度和极点方向。中心返回 `(0,0)`，地表距离对应 `hypot(x,y)`，误差目标 ≤1 m。50 km/1 km 得到 101×101；圆外 mask 与 source_outside 不混淆。

逐条对照 [station-inventory.md](station-inventory.md)：48 RDCAP、47 SIDARMA 历史站址、10 选择器、26 产品；PHBALE 冲突、MY 区域类型、id/SIDARMA 查询键与坐标出处保留。运行索引不擅自增加 all 目标或填站点范围。

按 [legacy-reference](legacy-reference.md)复现 47 项 ID/lon/lat/bbox 迁移一致性；704 个旧目录键、38 个裁剪 bbox 差异和三个 skip 不改变当前站点数或准入。Tropical Globe 的 88 个近距候选仍不是 same_site 认证。

实施时把新投影/局部流程测试采用 `projection`/`local_view` 名称，以下为运行约定。先列出测试确认非零、包含所需场景，再运行；过滤为零、pytest 未收集或缺 fixture 均不得记录为通过。

```bash
cargo test -p radiust-core projection -- --list
cargo test -p radiust-core local_view -- --list
cargo test -p radiust-cli local_view -- --list
uv run pytest --collect-only -q tests/contract/test_local_view.py
cargo test -p radiust-core --test local_view_projection -- --nocapture
cargo test -p radiust-core --test local_view_sampling --test local_view_catalog --test local_view_evidence
cargo test -p radiust-core --lib local_view
cargo test -p radiust-core --lib local_view_flow::
cargo test -p radiust-cli --test local_view_cli
uv run pytest -q tests/contract/test_local_view.py tests/contract/test_cli_cat.py
env -u CONDA_PREFIX uv run maturin develop --manifest-path crates/radiust-python/Cargo.toml --features extension-module,test-support
env -u CONDA_PREFIX uv run pytest -q tests/integration/test_local_view.py
```

当前 projection fixture 含 59 个独立参考点；最大误差为 0 m。Core `local_view_` 过滤 52 项（43 个 library 测试和 9 个集成测试），投影/采样/目录/证据集成 9 项、CLI local_view 集成 3 项、Python 局部视图合同 14 项及新的固定输入三入口回放 5 项通过。端到端回放对 PT raw RGBA 与 RainViewer decoded/dbz 的 values、quality 计算摘要并逐入口精确比对，同时检查 logical ID、valid_time、receipt、processing ID、范围和网格；视图变化后每个入口只获取一次原帧，processing ID 随请求变化。PT raw 与 SG categorical 在 auto+bilinear 下均经三个入口排除，SG 未触发发现/获取；native 失败也不回退 raw。59 点投影误差、nearest 精确值和 bilinear ≤1e-4 dBZ 结果保留在离线记录；PT 网页 overlay 没有独立像素 tie points，故源像元定位误差仍单独列为限制。另有 2026-10-10 RainViewer live 同帧对照，见 [`006-local-view-live.json`](../../validation-results/006-local-view-live.json)。SC-001 的裸 `cat` 与无位置、禁网、无适用来源等边界均有在线/受控证据；SC-006 的固定帧值/quality 摘要在 CLI、同步 SDK 和异步 SDK 间一致。

## 3. 受控端到端回放

Engine 测试通过 adapter 提供目录/帧；新增的 `test-support` feature 仅用于固定 PT/RainViewer fixture 和 SG 的分类排除桩，不发公网请求。输入配置保持 `allow_network: false`；测试专用 Engine 为显式 fixture source 打开共享候选/获取 gate，注册表用本地 adapter 替换测试来源，adapter 不调用 HTTP/FTP transport。CLI 以独立 target 目录构建，避免普通构建覆盖 fixture binary。CLI JSON 在此测试构建和显式 fixture 环境下附加数组 SHA-256 摘要，普通构建报告不变。测试对 PT 已验证网页 overlay 几何的 raw/nearest，以及 RainViewer decoded/dbz 的值和 quality 摘要做精确比较；同时覆盖半径变化、缓存命中、native 失败不得返回 raw，以及 PT raw/SG categorical 在三个入口的 auto+bilinear 排除。

合同/集成测试需执行同样的 CLI 参数并校验成功报告。人工运行先建立临时演示配置，默认禁网；这个配置不会注入假帧：

```bash
RADIUST_LOCAL_CONF="$(mktemp /tmp/radiust-local-view.XXXXXX)"
cat > "$RADIUST_LOCAL_CONF" <<'YAML'
location:
  longitude: 121.60
  latitude: 24.00
local_view:
  radius_km: 50
  resolution_m: 1000
runtime:
  allow_network: false
YAML
"$RADIUST_BIN" --conf "$RADIUST_LOCAL_CONF" cat --renderer text --json
"$RADIUST_BIN" --conf "$RADIUST_LOCAL_CONF" cat --radius-km 50 --resolution-m 1000
"$RADIUST_BIN" --conf "$RADIUST_LOCAL_CONF" cat rdcap --station TWRCHL --local --dbz
```

禁网且无受控输入时预期限制/无可用候选报告，不期待当前图或 exit 0。受控回放的成功 JSON 是单个完整 envelope，保留 source/product/station、valid_time、AEQD 中心与参数、实际 grid、选择/回退理由和覆盖未知项；不含数组或私有 locator。终端在原有 renderer 中呈现中心与距离轴，不用 width/height 改变科学 grid。

SDK 合同已检查 `Client(config=...).local_view()` 与 `await AsyncClient(config=...).local_view()` 的配置/环境 origins、终态、请求转换、网格/mask 持有、处理记录转接及取消；Python 不执行重采样。固定回放中同步/异步 SDK 的数值和 quality bytes 相同；CLI 通过仅限 test-support 的摘要与两者精确对照。此验证只证明两份固定 fixture 上的代码路径，不扩大来源许可、真实覆盖或默认来源可用性声明。

## 4. 失败、资源与兼容

| 场景 | 预期 |
| --- | --- |
| 位置缺失、单坐标、无效范围 | 联网/分配前拒绝 |
| 默认禁网、无访问准入、缺凭据 | 分别保留实际限制，不自动开网络/TLS 例外 |
| 源定位 unknown、TW datum 未支持、SG 源球面模型未支持 | 明确空间失败，不改标坐标 |
| 用户在产品域外或已知扫描覆盖外 | 尝试适用备用来源并保留原因 |
| 整帧缺测或低于显示阈值 | 保留真实视图状态，不伪造成已知无雨 |
| 首选失败/全部失败/8 候选截断 | 一次有界流程，切换/失败/未穷尽可见 |
| 请求可确定的 shape/像元/最低输出内存超限 | 联网与分配前拒绝，不自动放粗分辨率 |
| 源解码后同时存活内存超限 | 终止整批；释放真实缓冲后释放 lease，不继续备用源 |
| 取消/期限耗尽 | 受控取消 5 秒内收尾；循环内检查；迟到 worker 不发布，permit/lease 不提前释放 |
| `cat SOURCE`、`cat --file PATH` 不带 local 参数 | 通过原有代表回归，配置位置不改变其默认模式 |

预检与旧文件入口可人工运行，不需观测网络：

```bash
"$RADIUST_BIN" --conf "$RADIUST_LOCAL_CONF" cat --radius-km 0 --json
"$RADIUST_BIN" --conf "$RADIUST_LOCAL_CONF" cat --resolution-m 0 --json
"$RADIUST_BIN" --conf "$RADIUST_LOCAL_CONF" cat --station TWRCHL --json
"$RADIUST_BIN" --conf "$RADIUST_LOCAL_CONF" cat --file tests/fixtures/sources/pt/raw/PTST2.png --local --json
"$RADIUST_BIN" --conf "$RADIUST_LOCAL_CONF" cat --file tests/fixtures/sources/pt/raw/PTST2.png --raw --renderer text --json
```

前四条应预检失败，码 2 且无网络请求；最后一条走原文件预览，不自动补几何。合同测试还覆盖缺位置、单坐标、未知配置键、native 失败不能改 raw、auto+bilinear 排除 raw/类别、旧 source 默认 raw、历史时间 local 拒绝。

用非对称栅格核对中心/边缘注册、半像元、升降轴和非方图；用 data_window/WMS 冲突 fixture 核对裁后注册与 receipt。nearest 的值/质量精确匹配；bilinear dBZ 在 Z 域独立比较；圆外、补边、透明、annotation、recovered 和已知零值不混淆。独立 source 几何控制点与纯投影误差分别记录。

受控时钟/慢 adapter 验证目录刷新、八个候选及处理共用一个 discovery_deadline；deadline 不能随候选重置。重复取消压力回放检查后台任务和内存不累积，源 raw cache hash/有效时间保持原值。

## 5. 真实来源准入

在明确网络授权与实际配置下，通过安装后的标准入口记录当前帧、获取摘要、几何/空间关系、投影检查、用途准入及工具 build。RDCAP 正常 TLS 与例外路径分开；维护既有逐国 gate。不得读取旧项目凭据或把回放结果当上游成功。

产品 live 验收需用户明确 opt-in、隔离配置、正常 TLS，并记录用途、构建和当前帧。RainViewer 个人用途 `local_view` 与裸 `cat` 链路已通过；RDCAP 仍是独立的准入候选，需满足自身访问和授权条件。不能用不支持的凭据/TLS 路径兜底。

```bash
RADIUST_LOCATION__LONGITUDE=-16.9 RADIUST_LOCATION__LATITUDE=32.7 RADIUST_RUNTIME__ALLOW_NETWORK=true \
  "$RADIUST_BIN" --conf "$RADIUST_LOCAL_CONF" cat rainviewer --local --dbz --renderer text --json
RADIUST_RUNTIME__ALLOW_NETWORK=true "$RADIUST_BIN" --conf "$RADIUST_LOCAL_CONF" cat rdcap --station TWRCHL --local --dbz --renderer text --json
RADIUST_RUNTIME__ALLOW_NETWORK=true "$RADIUST_BIN" --conf "$RADIUST_LOCAL_CONF" cat --renderer text --json
```

2026-10-24 复核的 RainViewer 官方 API 条款允许个人/教育用途，要求应用显示带 RainViewer 链接的署名；商业或高流量集成需个案协商，服务无 SLA，且上游数据所有者可变更或撤回数据。[官方条款](https://www.rainviewer.com/api.html)。本项目仅将个人用途的 `local_view` 纳入自动选择，不声明商业集成或底层观测数据可再分发。

2026-10-10 的安装后显式来源链路与 2026-10-24 的默认链路分别记录在 [`006-local-view-live.json`](../../validation-results/006-local-view-live.json)。后者通过空 source 的 `cat --renderer text`，在隔离 HOME/config、配置默认禁网且仅对该命令开启网络、无凭据、正常 TLS 条件下，选中 `rainviewer/composite` 的 `2026-10-24T03:00Z` 帧，生成 50 km、1 km、101×101 dBZ 视图；最终 wheel SHA-256 为 `3278ea9afc182f7d1f53c545e19486eb6c6ac2ec56880d947d1f897756d0f68b`。报告显示 `Weather data by RainViewer: https://www.rainviewer.com/`，候选关系为 `needs_probe_coverage_unknown`。图框和像元有效域不证明真实雷达观测覆盖；当前测试点及其他地区的 observation coverage 保持 unknown。IPMA 2026-10-10 标准 CLI 发现返回 `no_data`。

这里的 RDCAP/演示点只是验证用例，不保证适用或准入；未通过 gate 时，选择已过 gate 的产品/公开测试点并记录原因。PT 页面使用 Leaflet EPSG:3857 `ImageOverlay`，已记录官方 bounds、尺寸与 receipt 绑定显示 affine；IPMA 未发布独立像素 tie points，因此只支持网页显示几何，不支持定量解释或覆盖声明。PT 当前在线可用性和默认准入仍待 gate。

SC-008 已由 RainViewer 个人用途 `local_view` 的当前安装后裸 `cat` 链路满足；开放使用范围按用户个人用途和已展示署名限定。未验证地区、站点和产品仍保留限制。本规格不关闭 001–005、Desktop、预测或拼图验收。
