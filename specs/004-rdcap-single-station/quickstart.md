# RDCAP 验证与使用指南

本指南记录当前已实现的离线目录、SDK、解码、输出与验证器，以及仍未通过的在线验收门槛。离线重建样本和合同测试不代表三国实时服务已验收。2026-10-01 的标准 TLS SDK 实测中，TWN/RCHL、JPN/ISHI、PHL/SUBI（当时使用的旧站点 ID） 均在 discovery 阶段返回可重试的 `catalog_unavailable`；没有请求文件 ticket，也没有 raw 产物。T028、T053 保持未勾选，SC-003 为 `not_verified`。安全摘要见 [在线尝试记录](../../validation-results/rdcap-live-attempt-20261001.json)；离线验证摘要见 [离线记录](../../validation-results/rdcap-offline-20261001.json)。

## 当前可运行的离线检查

先构建原生 CLI，并运行已有 RDCAP 合同测试、SDK 测试和绑定模型测试：

```sh
cargo build -p radiust-cli
cargo test -p radiust-core --test rdcap_contract
uv run --locked --group dev pytest -q tests/test_rdcap_sdk.py tests/contract/test_rdcap_models.py
uv run --locked --group ci --group dev --extra geotiff --extra zarr -- python scripts/validation/validate_rdcap.py --mode offline --out /tmp/rdcap-validation/offline
target/debug/radiust --json list stations rdcap
target/debug/radiust --json list products rdcap
```

内置目录列出 48 个站点（TWN 13、JPN 20、PHL 15），但这是离线快照；命令结果中的逐国 discovery/raw/science/readback 能力目前均为 `unverified`。CLI 的 `list`、`discover`、`download`、`cat` 和 `replay` 都是原生通用入口。离线验证器运行 Rust 合同及独立 Python 格式读回，输出安全 `summary.json`；它不把研究回放结果当成 live 证据。

如需复现已有研究样本，使用含 NumPy 与 Pillow 的 Python 环境。以下命令只读仓库 CSV，不访问公网；输出 `echo.png` 是网页显示/标记复现，不是科学 PNG：

```sh
python scripts/validation/probe_rdcap.py --country TWN --station RCHL --decode-file validation-results/rdcap-analysis/RCHL/frame.csv --out /tmp/rdcap-replay/RCHL
python scripts/validation/probe_rdcap.py --country JPN --station ISHI --decode-file validation-results/rdcap-analysis/ISHI/frame.csv --out /tmp/rdcap-replay/ISHI
python scripts/validation/probe_rdcap.py --country PHL --station SUBI --decode-file validation-results/rdcap-analysis/SUBI/frame.csv --out /tmp/rdcap-replay/PHL
```

这些文件和 `*.reconstructed.json` 是研究回放材料，不是 adapter 获取的原始 HTTP 响应。完整离线验证以 `scripts/validation/validate_rdcap.py --mode offline` 为入口；Rust 与 Python 测试也可分别运行。

## RDCAP Python SDK

单站使用国家限定 ID 和 `reflectivity` 产品。实时发现需要显式启用网络；`discover_report()` 的 JSON 是安全摘要，不序列化 locator/ticket，成功项用同一进程中的 `frame(index)` 取得可继续 acquire/fetch 的句柄：

```python
import radiust

query = radiust.Query(
    "rdcap", product="reflectivity", stations=("TWRCHL",), latest=True
)
with radiust.Client(config={"runtime": {"allow_network": True}}) as client:
    report = client.discover_report(query)
    print(report.to_json())
    index = next(
        (i for i, item in enumerate(report.items) if item.status == "success"),
        None,
    )
    if index is not None:
        ref = report.frame(index)
        with client.acquire(ref) as raw:
            field = client.decode(raw)
```

`DiscoveryReport` 同时保留成功、`no_data` 和失败终态；从 JSON 重新构造的报告没有原进程私有句柄，不能调用 `frame(index)`。`Client.fetch_many()` 和 `AsyncClient.fetch_many()` 对 query 输入在 `on_error="collect"` 或 `"continue"` 时，把成功 refs 继续交给 batch 获取，同时在 `BatchResult.discovery_report` 保留所有发现终态，并通过 `discovery_counts` 给出发现状态计数。它与 frame 级 `BatchResult.counts` 分开统计；`no_data` 会记录，但不生成 frame。`on_error="raise"` 或 `"stop"` 遇到发现失败时通过 `BatchError.partial_result` 保留发现报告及成功 frame 句柄，并在发现失败时停止后续获取。纯 `FrameRef` 批次的这两个发现属性为 `None`。异步客户端也提供 `await discover_report()`、`await replay_raw_manifest()`，其余操作按异步生命周期使用。

原始文件保存后，可在禁网配置下根据该下载输出目录中的实际 manifest 路径离线重放。raw-only RDCAP 的 manifest 文件名为 `raw-manifest.json`，同一结果目录还包含绑定及原始 payload；完整路径按实际输出根目录和 logical frame ID 确定：

```python
from pathlib import Path
import radiust

manifest = Path("./data/frames/<logical-frame-id>/raw-manifest.json")
with radiust.Client(config={"runtime": {"allow_network": False}}) as client:
    field = client.replay_raw_manifest(manifest)
```

把占位 logical ID 换成真实值。重放会校验 manifest、binding、路径和摘要，不会恢复 ticket 或发起网络请求；被篡改或身份不匹配的内容会失败。返回的 `RadarField` 独立于 Client/raw 生命周期。`write(field, ref=ref, ...)` 可写 PNG、NetCDF、GeoTIFF、Zarr v2；它要求原始 `FrameRef`、只接受本地 native-grid 输出，不接受 raw 附加或重网格。要获取并保存 raw，使用 `download(..., raw_only=True)`；要在同一提交中保存 raw 和解码成果，使用 `download(..., raw=True)`。

原生 CLI 也能直接离线重放已提交的 manifest。默认写入四种科学格式；`--format` 可用逗号分隔选择子集，重复运行相同输出会报告 `skipped`：

```sh
target/debug/radiust replay ./data/frames/<logical-frame-id>/raw-manifest.json \
  --output ./replayed --format png,netcdf,geotiff,zarr --json
```

该命令强制禁网，仅经 Rust Engine 校验 manifest、binding 和 payload 后解码及写出；研究重建 envelope 仍只代表离线 fixture。

### 质量与渲染

既有质量位 bit 0–5 保持原含义；新增 bit 6 `source_annotation`（mask 64）。当前经三国样本观察，原始值 9999 被分类为图面范围/站点标记候选：科学值变为 NaN，quality 为 65（bit 0 `missing` 与 bit 6 `source_annotation`）。这是有样本依据的推断规则，不是提供者的正式 sentinel 声明。`to_xarray()` 保留 `uint16` quality 与七个 `flag_masks`/`flag_meanings`；NetCDF、GeoTIFF provenance、Zarr writer 保存新增位，reader 接受旧六位 metadata。

RDCAP 默认 palette 为 `rdcap-reflectivity-v1`，含 15 个 5–75 dBZ 下界包含档，最后一档也包含所有 ≥75 dBZ 值；低于 5 dBZ 的有限 field 值仍有效，但显示透明。缺测与 annotation 也透明。科学 preview 和 PNG writer 共用规则，PNG sidecar 带 palette、规则、decoder 和 geometry 身份。PNG 只供显示，连续科学值应使用 NetCDF、GeoTIFF 或 Zarr。

`output_template` 只作用于解码格式，支持 `{source}`、`{product}`、`{station}`、`{valid_time}`、`{base_time}`、`{date}`、`{hour}`、`{variant_id}`、`{ext}`。本地模板必须保持在输出根目录内并生成相对路径；RDCAP 的 `{station}` 直接使用无斜杠站点 ID（例如 `TWRCHL`）。绝对路径、越界路径会拒绝；raw-only 不支持 output template。非空 `encoder_options` 尚不支持。

## 在线验收入口与状态

仅在获准联网且提供者可访问时运行以下标准 CLI。先启用 `runtime.allow_network`，保持正常 TLS 证书校验；不要使用 `curl -k` 或研究脚本替代 Engine/CLI 证据：

```sh
cat > /tmp/rdcap-live.yaml <<'YAML'
runtime:
  allow_network: true
YAML

target/debug/radiust --conf /tmp/rdcap-live.yaml --json discover rdcap --station TWRCHL --latest
target/debug/radiust --conf /tmp/rdcap-live.yaml --json discover rdcap --station JPISHI --latest
target/debug/radiust --conf /tmp/rdcap-live.yaml --json discover rdcap --station PHSUBI --latest

target/debug/radiust --conf /tmp/rdcap-live.yaml --json download rdcap --station TWRCHL --latest --raw-only --output /tmp/rdcap-validation/live-raw/TWN
target/debug/radiust --conf /tmp/rdcap-live.yaml --json download rdcap --station JPISHI --latest --raw-only --output /tmp/rdcap-validation/live-raw/JPN
target/debug/radiust --conf /tmp/rdcap-live.yaml --json download rdcap --station PHSUBI --latest --raw-only --output /tmp/rdcap-validation/live-raw/PHL
```

这些是现有 `radiust` 子命令和参数；它们是验收操作说明，不代表命令已经成功执行。记录实际日期、CLI build、country/station/valid time、raw 摘要、manifest 和请求尝试数。某个代表站没有数据时只换同一国家的站点；不要用另一个国家或离线 fixture 补齐。

| 门槛 | 当前状态 | 尚需证据 |
|---|---|---|
| T028 / SC-003 raw 阶段 | 未通过，任务未勾选；2026-10-01 三站均在 discovery 收到 retryable `catalog_unavailable`，未发起文件 GET | 三国各至少一站真实 raw response、binding、manifest、摘要及日期/build |
| T053 / SC-003 science/readback | 未通过，任务未勾选；离线验证器已实现，但受 T028 上游目录不可用阻断，尚无 live 帧可读回 | 三国标准入口固定同一 ref，保存/解码并独立读回科学成果，记录四格式同帧矩阵 |
| SC-001 / SC-002 离线 | 当前台账为 `offline_passed`；live 未验收 | 实时目录/索引仍需独立在线证据 |
| SC-004 | 离线通过：三国重建内容合同覆盖 geometry、RCHL 八点及数值边界；live 未验证 | 三国正常 TLS 获取后的 geometry/误差核验 |
| SC-005 | 离线通过：缺测/弱值/annotation 65、15 档 palette 和质量传播合同通过；live 未验证 | 三国真实响应的标记解释与科学读回 |
| SC-006 | 离线通过：PNG/NetCDF/GeoTIFF/Zarr 独立 Python 读回及重复写 `skipped` 通过；live 未验证 | 同一 live frame 的四格式独立读回与幂等记录 |
| SC-007 / SC-008 | 部分离线通过：stop/raise/partial report、CLI replay 和 SDK 生命周期用例通过；总体 `not_verified` | 完整 budget/取消/迟到提交矩阵及 CLI/同步/异步同 ref 等价核对 |

T053 的 live 流程由 `scripts/validation/validate_rdcap.py --mode live --conf <config> --out <directory>` 驱动：标准 SDK 固定每国一帧，保存 raw manifest，再重放同一 raw、写入四种格式并独立读回，记录重复写的 `skipped` 状态。配置必须显式启用 `runtime.allow_network`，并保持正常 TLS 校验。2026-10-01 的运行在 discovery 即失败，因此没有 raw 或科学成果；T028/T053 及 SC-003 继续保持未验收，直到逐国证据进入验证台账。
