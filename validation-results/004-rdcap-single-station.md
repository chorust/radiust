# RDCAP 单站支持验证台账

记录日期：2026-10-01（UTC）
范围：TWN、JPN、PHL 单站；离线研究样本、原生在线获取与科学成果独立读回分开记录。

## 构建与证据边界

| 项目 | 记录 |
|---|---|
| 仓库基线 | `19831d1c5585e4b87a1fd423074ff3be5fe6c311`；工作区包含未提交修改 |
| 工具链 | Rust/Cargo 1.92.0；`aarch64-apple-darwin`（macOS arm64） |
| crate | `radiust-core 0.1.0` |
| 当前功能构建身份 | Rust/Cargo 1.92.0；macOS arm64 `aarch64-apple-darwin`；Python 3.12.7；`radiust 0.1.0` release CLI 与 CPython 3.12 wheel 均已构建 |
| Rust 合同与回归 | `cargo test --workspace` 通过；`rdcap_contract` 31 passed，`source_matrix` 10 passed，CLI RDCAP 回放合同通过；`cargo fmt --all -- --check` 与 `git diff --check` 通过。普通 workspace Clippy 退出码 0；仓库仍有既存 lint warnings，严格 `-D warnings` 检查不能通过 |
| Python 合同与回归 | 全量 `pytest -q`：324 passed、25 skipped；跳过项为显式 opt-in 的 provider/live 用例及独立读回入口（后者由离线验证器实际执行并通过）。Ruff 全量通过 |
| 安装后验证 | release wheel `radiust-0.1.0-cp312-cp312-macosx_11_0_arm64.whl`；含 Zarr extras 的 packaging smoke 2 passed，覆盖安装后 SDK/catalog/资源及 wheel/native CLI `replay --help` 一致性 |
| CLI 回放 | 默认离线 list 显示48站，全来源 discover latest 返回74目标；新增原生 `radiust replay` 对 raw manifest 离线写出 PNG/NetCDF/GeoTIFF/Zarr，重复执行四项均为 `skipped` |
| 离线科学验证 | `scripts/validation/validate_rdcap.py --mode offline` 于 2026-10-01 14:24:38 UTC 通过：Rust RDCAP 合同和独立 Python PNG/NetCDF/GeoTIFF/Zarr 读回通过；安全摘要见 `rdcap-offline-20261001.json` |
| 研究材料 | `validation-results/rdcap-analysis/`；2026-10-01 浏览器观察/导出的三站 CSV、几何和研究对比 |
| 在线验收 | 2026-10-01 12:53:27 UTC 标准 Rust-backed SDK、系统 TLS 实测：TWN/RCHL、JPN/ISHI、PHL/SUBI 均在 discovery 阶段返回 retryable `catalog_unavailable`；未发起 file GET、未获取 raw，也没有 science/readback；安全记录见 `rdcap-live-attempt-20261001.json` |
| 敏感信息 | manifest/index fixture 不保存 ticket、临时凭据或完整 file URL |

## 原始获取与持久化合同（离线 loopback）

| 场景 | 结果 | 证据边界 |
|---|---|---|
| 首读 | 通过：单次 GET 保留原始 JSON envelope 字节；无 HEAD/预读；binding 确定性且不含票据 | loopback HTTP 协议合同，不证明公网 TLS |
| 空内容/轮换 | 通过：空 JSON 后仅刷新同一 epoch key；刷新返回旧票据时不二次 GET；换新票据成功 | 不使用重建 fixture 作为响应 |
| 限额与终态 | 通过：三次文件 GET/两次索引刷新封顶；时刻消失、403、HTML、超限、取消均无 raw 成功；shared discovery deadline、request/host/frame/CPU worker、批量 stop/外部取消和迟到 frame 禁止提交均有合同覆盖 | 标准入口 live 仍受 provider catalog 不可用阻塞 |
| 保存与离线 load | 通过：raw-only v1 manifest 含 `file-response.json` 与 `binding.json`；校验 ticket/identity 分离、内容摘要 revision、绑定篡改及 symlink 拒绝 | 本地合成合同输入；非真实上游采集证据 |
| 并发与幂等 | 通过：同 logical frame 不同 ticket 并发只取一次；cache 复制后离线 manifest 可 load；重复 raw-only 为 skipped | Engine adapter contract；公网凭据轮换待 T028 |

`tests/fixtures/sources/rdcap/manifest.json`记录样本来源、文件大小、SHA-256 和 frame key。`frame.csv`是浏览器派生的内容样本，不是逐字节原生 HTTP 响应。`file-response.reconstructed.json`是为回放构造的 JSON 字符串 envelope；`index.redacted.reconstructed.json`保留观察到的 key、移除票据。这些材料可支持离线研究与合同测试，不能证明 live 获取或独立格式读回。

## 三国样本与逐国状态

| 国家/站点 | 实际 key（epoch ms） | UTC 有效时刻 | frame SHA-256 | Offline | Live raw | 科学成果独立读回 |
|---|---:|---|---|---|---|---|
| TWN/RCHL | 1790834708000 | 2026-10-01T06:05:08Z | `9cabec9adc19b92bceb597ae45348ba25632802ce46b1aa47b6fb6cf77244bbd` | 研究样本已登记；标准 decoder 验收未验证 | 未验证；标准原生获取尚未完成 | 未运行 |
| JPN/ISHI | 1790834268000 | 2026-10-01T05:57:48Z | `0d53b6cad20e2a2ecca94f985dfa21e0a049ff3ddb5e5326f31b5c9b790dbf2f` | 研究样本已登记；标准 decoder 验收未验证 | 未验证；标准原生获取尚未完成 | 未运行 |
| PHL/SUBI | 1790833210000 | 2026-10-01T05:40:10Z | `458917d6007fed272568bc10f2c4d85c52a53c5d037350943aa6616f619bfafe` | 研究样本已登记；标准 decoder 验收未验证 | 未验证；标准原生获取尚未完成 | 未运行 |

研究样本的尺寸为 RCHL 901×901、ISHI/SUBI 900×900；比较样本 `TWN/RCHL/comparison.json` 含 8 个网页参考点。既有研究记录报告三国内容解码和 RCHL 参考点核对；这些是研究证据，不是本功能标准入口、质量标记或格式读回的通过结论。

## SC-001–SC-008 验收状态

本表不预填 `passed`。结果只在相应标准入口与证据齐备后更新。

| 标准 | Offline 结果 | Live 结果 | 状态/限制 |
|---|---|---|---|
| SC-001 | 通过：13 TWN + 20 JPN + 15 PHL 去重为48站；BALE Active/Inactive冲突、动态增站/短码歧义/刷新失败回退、保留快照和无坐标均有合同测试；CLI离线列出48站并显示逐国能力 | 未验证：loopback仅验证协议与目录解析，不代替公网实时目录 | offline_passed；live_not_verified |
| SC-002 | 通过：真实epoch-ms样本回放覆盖latest、精确at、半开range、stale、空索引与no_matching_time；逐目标状态无合成时间，range按frame items计数，raw file GET为0 | 未验证：三国当前公网索引尚未由标准CLI/Engine验收 | offline_passed；live_not_verified |
| SC-003 | offline fixture 不作为证据 | 三国标准 SDK 均在 discovery 收到 retryable `catalog_unavailable`，未进入 raw→decode→readback | not_verified；T028/T053 未关闭 |
| SC-004 | 离线合同通过：三站 geometry、八参考点、方向/中心注册和误差门槛由重建内容样本验证 | 未验证 | offline_passed；live_not_verified |
| SC-005 | 离线合同通过：9999→NaN+quality 65、缺测/弱值、15 档 palette 与四格式质量传播均有测试 | 未验证 | offline_passed；live_not_verified |
| SC-006 | 离线验证器通过四格式 writer 与独立 Python 读回；相同输出重复写为 `skipped` | 未验证 | offline_passed；live_not_verified |
| SC-007 | 离线异常、mixed batch、完整目标计数、超时/deadline、stop/外部取消、未开始终态、临时资源清理、迟到提交隔离及共享预算合同通过 | live provider 结果未验证 | offline_passed；live_not_verified |
| SC-008 | 同一 raw manifest/ref 经原生 CLI、同步 SDK、异步 SDK 读取并写出 PNG/NetCDF/GeoTIFF/Zarr；logical ID、UTC、值、质量、CRS 等价；旧来源全量回归通过 | live provider query 未验证 | offline_passed；live_not_verified |

## 更新记录

- 2026-10-01：建立证据基线；增加去票据、带摘要的研究 fixture 和 `rdcap_contract` manifest 检查。无 SC 被判为通过。
- 2026-10-01：完成目录 hook、Engine 快照合并/总deadline/取消、近期索引解析和 CLI 离线目录显示；离线SC-001/002合同分别通过。loopback只作为本地协议测试；公网发现、原始获取、科学解码与独立读回仍未验收。
- 2026-10-01：完成 US2 离线实现：RDCAP 单票据 GET、同 key 有界索引刷新、内容摘要 revision、确定性 `binding.json`、raw manifest/cache绑定核验及 ticket-safe 离线读取；raw-only Engine 测试验证保存后离线 load 与重复 `skipped`。新增 loopback 状态机覆盖旧票据去重、403、HTML、空内容、超限、取消。T028三国正常TLS在线获取仍未验证；SC-007整体因批量预算/混合终态矩阵未完成而保持 not_verified。
- 2026-10-01：完成 US3 离线科学链路与 US4 SDK/CLI 接口，包括 CSR decoder、EPSG:4326 几何、quality bit 6、版本化 palette、四 writer、raw-manifest replay、同步/异步 API、部分发现报告和原生 CLI replay。`rdcap_contract` 31 项通过；原生 CLI 回放同一 manifest 首轮写出四种格式、重复写四项均 skipped。独立 Python 读回核对数值、quality、CRS、palette/sidecar；offline validator 全步骤 passed。样本仍明确是重建内容，不是 live HTTP。
- 2026-10-01：RDCAP batch 完成多站部分失败、stop、外部取消/未开始和迟到结果禁止提交检查；同一 ref 的原生 CLI、同步 SDK、异步 SDK 四格式结果及身份/值/质量/CRS 对照通过。Rust workspace 全部测试、Python 全量 324 passed/25 skipped、Ruff、Rust fmt 与 diff 检查通过；release wheel smoke 通过。SC-007/008 离线状态更新为 `offline_passed`。
- 2026-10-01：标准 SDK 在线尝试固定三站均因 provider catalog 不可用在 discovery 失败，未触发文件请求；逐国能力仍为 `unverified`。T028/T053 保持未勾选，T058 和最终 gate 因必需 live 条件未满足继续开放。
