# Implementation Plan: 灰度编码 dBZ 解码与 gray/dbz 口径统一

**Branch**: `main`（实际 Git 分支；setup-plan 返回逻辑特性 `005-gray-dbz-decoding`） | **Date**: 2026-10-02 | **Spec**: [spec.md](spec.md)

**Input**: `specs/005-gray-dbz-decoding/spec.md`。活动特性指针为 `.specify/feature.json`；没有 before_plan/after_plan hook，setup 没有切换 Git 分支。

**Status**: Phase 0研究与Phase 1设计完成，tasks已生成；2026-10-03修订分析发现的五项问题。尚未实施或运行产品测试。

## Summary

将用户已确认的整数 gray 0～224 按 `gray * 5/16` 转为编码对应 dBZ，接入共用原生工作流，统一相关 CLI/docs/代码活动命名。已有直接数值反射率解码保持原精度，gray 的像素一致性与 dBZ 的质量限制分别验收。来源路径由现有通过规则和输入约束决定，本地图像由显式编码声明决定；无地理定位仍允许像素空间值读取和适用数值成果保存。

NZ已验证gray基准有6个可见超224像素、最大229；用户已确认仅dBZ反算截断到224，原gray不改。来源使用source-upper-clip-v1、本地使用strict-v1，截断位置/数量及质量可追溯。Clarifications、FR-004/007和SC-003已同步；不会将来源例外自动套到裸本地文件。

## Technical Context

**Language/Version**: Rust 1.92.0 / edition 2024；Python >=3.10 薄 SDK，既有支持声明为 CPython 3.10–3.13。

**Primary Dependencies**: 复用 workspace 已锁定的 image 0.25（PNG/GIF/WebP）、serde/serde_json、sha2、Tokio、chrono、netcdf 0.12.1 静态构建、zarrs 0.23.14/Zarr v2、tiff 0.11.3、PyO3 0.29 和 maturin >=1.15,<2。无新增 provider、浏览器、Python 图像业务或地理投影依赖；NumPy/xarray/Pillow 只作显式互操作与独立读回。

**Storage**: 原始文件/缓存及既有v1 manifest-last本地提交；像素数值成果使用NetCDF4/Zarr v2，保存values/quality/必要origin_quality/encoding_adjustment及完整处理记录。新本地identity/receipt入口复用原事务。图像预览/PNG清楚标识无定位。既有geographic数据和适用输出继续原合同，真实S3/OSS验收不在005内。历史规则内容与身份通过兼容读取保留，新dBZ处理具有独立decoder/quality/range策略版本。

**Testing**: 规划225级编码、透明/黑色/越界、灰度15路径像素一致性、质量伴随信息、无时刻/无CRS的数值读回、CLI/同步异步等价、三类直接数值解码、旧命名/旧身份兼容、超限/取消/提交故障的离线验收。本阶段只检查设计和资料，不运行产品测试或将规划命令标为通过。

**Target Platform**: 沿用003的首轮 macOS arm64 CLI/wheel；其他平台保持可构建性，不新增实际 macOS 11 主机、GitHub托管CI或全平台通过声明。

**Project Type**: Rust library + native CLI + PyO3 SDK；Python维持薄入口和显式科学互操作。

**Performance Goals**: O(n)扫描反算，避免同一请求重取资料/重复生成gray；遵守共享decode_workers=2、frame/request/host=2/16/4。值/质量至少6字节/像素，RGBA、origin_quality/alpha/adjustment、crop/inpaint/resize与writer缓冲另计；大图按同时存活缓冲检查，不从max_pixels推定可用内存。Native使用Arc/dataset owner+index包装、writer借用RasterView，避免整场Vec复制；显式NumPy/xarray转换及编码缓冲仍计内存。无新增毫秒SLO或性能加速承诺。

**Constraints**: 225级有效整数码、本地可见超界拒绝；来源已匹配通过规则的超上限值只在反算截断并记录。不能用亮度/值域自动推定来源；alpha=0 missing，已知无效mask不能因修补/截断变有效。不能造time/CRS、恢复裁剪前数值或重标全部来源科学验证。默认禁公网、既有期限/资源/取消和manifest-last继续生效。

**Scale/Scope**: 23条现有显示路径，15 passed/8 blocked；三类已有直接数值路径；来源/本地、CLI/同步/异步、适用四格式。仅相关业务命名与兼容承接；不添加数据源、桌面UI、新CRS或云端验收。

**2026-10-03 Design Revision**: US1提前完成本地三模式及additive报告；数值读回新增NumericFile身份/receipt与upstream provenance，支持无原FrameRef文件再保存；SDK replay mode贯通Core/PyO3/bridge/同步异步；alpha按原8/16位判断并无损保存，8位显示单独派生；T001/T086保护清单逐文件约束历史材料，README/docs活动文档允许按任务更新。

**Resolved Design Questions**: NZ范围政策由用户确认；质量伴随信息、Pixel模型、无time/CRS格式、local身份/事务、CLI/SDK与旧规则codec由三名只读subagents及主代理核对收敛，记录于[research.md](research.md)。无待定设计门槛；实际来源科学/质量/live验收仍待实施，不预填通过。

## Constitution Check

[constitution](../../.specify/memory/constitution.md)仍是未批准占位模板，不能从示例推定正式原则。本次依据005明示要求、现有原生架构及成果兼容基线核对。

| Gate | Phase 0 前 | Phase 1 后 |
| --- | --- | --- |
| 原生共用解码与Python薄层 | 通过：既有架构可复用 | 通过：Core统一转换/分派，SDK委托原生与共享scheduler |
| 用户确认的编码与已知质量限制 | 公式已定，NZ政策当时冲突 | 通过：用户确认来源upper clip；local strict；质量/origin/adjustment契约明确 |
| 不伪造时间或地理定位 | 通过：来源与本地输入身份需分开 | 通过：Pixel optional time/geometry，地理操作仅可信映射 |
| gray像素、raw身份与历史证据保留 | 通过：仅活动命名迁移 | 通过：质量旁路不改像素，原序列化hash与wire读取保留 |
| 直接数值与非反射率结果语义保持 | 通过：不能量化或改单位 | 通过：三类direct优先，generic旧入口不强制dbz |
| 离线/编码/上游物理/live验收分开 | 通过：不升级来源状态 | 通过：15/8逐路径、005 planned、旧spec待验收不关闭 |
| 必需需求无矛盾、无未定设计 | NZ矛盾已发起澄清 | 通过：用户答复已回写spec，所有研究问题已有决策 |

上述“通过”表示设计满足约束，不表示代码、产品测试或来源live已验收。正式constitution尚未批准的事实继续保留，不以本次plan代替治理文档。

## Project Structure

### Documentation (this feature)

```text
specs/005-gray-dbz-decoding/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── cli-sdk.md
│   ├── gray-decoding.md
│   └── persistence.md
└── checklists/requirements.md
```

研究、数据模型、三份合同、验证指南及[tasks.md](tasks.md)已生成；任务保留86项/29项并行标记，全部未实施。

### Source Code (repository root)

```text
crates/radiust-core/src/
├── gray.rs, legacy_display.rs            # 新活动模块；旧模块薄兼容，不维护两套算法
├── dbz.rs, raster.rs                     # 新编码反算、Pixel/统一view模型
├── engine.rs,science.rs,grid.rs           # 来源/直接数值分派、quality常量名称修正
├── identity.rs,raw_manifest.rs           # 稳定来源identity、独立local身份与offline加载
├── output/{png,netcdf,zarr,geotiff}.rs    # 新Pixel读写与原地理输出
├── download.rs,storage/                  # 校验identity入口汇入原事务
└── python_types.rs                      # 共用Core的PyO3类型/方法
crates/radiust-cli/src/{lib.rs,commands/,report.rs}
python/radiust/{api.py,rust_client.py,_bridge.py,science_adapters.py,__init__.py}
python/radiust/decoders/{gray_dbz.py,__init__.py}        # 规范/旧类薄Core接口；wheel范围显式调整
python/radiust/resources/{legacy_display/,gray/}       # 历史路径及新规范映射
crates/radiust-core/tests/                # 解码、质量、路径、格式、身份合同
crates/radiust-cli/tests/                 # 模式/JSON/本地/来源命令合同
tests/                                   # SDK/wheel/独立数值读回
scripts/validation/, README.md, docs/     # 活动gray/dbz入口和示例
```

**Structure Decision**: 使用现有workspace，不增加crate或第二套Python业务流程。活动源码/resource名称使用gray/dbz，历史规则/operation/version codec保留可验证原序列化语义；GrayPreview/GrayFrame归一底层实现。新增PixelDbzField与RasterResult/View承接缺time/geometry，保持旧RadarField严格契约。以上新文件为拟实施位置，当前没有创建源码。

## Phase 0 — Research

已完成[research.md](research.md) R1–R8；R9记录分析后的五项修订。三名subagents分别收集灰度质量/路径、像素模型/格式、命名与公开接口证据；主代理核对storage identity及15张基准范围。关键决策包括来源反算截断且原gray不改、质量旁路、直接native优先、独立local identity和Manifest v1复用、报告additive模式schema、旧资源hash codec。必要用户澄清已完成；没有未决设计项。

## Phase 1 — Design

### 处理主线

来源路径：discover/select/acquire receipt → direct native或匹配passed gray → 保持原gray算法并伴随quality → 来源upper clip反算 → RasterResult → 适用writer → 共用事务。用户本地：显式--dbz/decode_gray_file声明 → 有界读取/校验/digest → local strict反算 → Pixel结果 → NetCDF/Zarr/PNG → identity提交入口。数值文件只读取真实reflectivity/dBZ，不重复反算。

数值文件路径：有界读取实际文件/依赖组件 → NumericFileIdentity与NumericReadReceipt绑定content digest/变量/选择 → Pixel/native值及真实time/geometry → 保存历史记录为upstream provenance → 以file_dbz和当前文件身份再保存。未含原FrameRef的旧科学文件合法，不伪造来源或gray编码。NetCDF单文件SHA、Zarr全树清单摘要、既有GeoTIFF数据/质量/出处组件摘要详见[persistence](contracts/persistence.md)。alpha以原U8/U16保存，数值有效性在原位深判断；派生8位预览与原数组分开计预算。

质量传播在原alpha/palette/background/crop/repair/resize处理点同步记录，不通过最终PNG猜测；成功修补与原原因分别保存。encoding_adjustment独立记录上限截断，不修改旧quality bit含义或将NaN变有效。来源模型只有可信几何/映射时提供地理view。

### 已生成artifacts

| Artifact | 内容 |
| --- | --- |
| [data-model.md](data-model.md) | 来源/local_gray/local_numeric identity与receipt、GrayDecision、Pixel/native backing、原alpha位深、质量/调整、processing及状态 |
| [gray-decoding.md](contracts/gray-decoding.md) | 编码声明、strict/source-upper-clip、Core接口、精度/质量/结构化错误 |
| [cli-sdk.md](contracts/cli-sdk.md) | cat/download/replay、同步/异步/batch、旧参数、报告schema1与资源名称迁移 |
| [persistence.md](contracts/persistence.md) | Pixel NetCDF/Zarr、可选time/geometry、独立读回、manifest/事务/hash兼容 |
| [quickstart.md](quickstart.md) | 实施后离线225码、15路径/NZ、SDK、数值读回、直接数值/历史/故障验收 |

### Requirement coverage

| Requirements | 设计覆盖 |
| --- | --- |
| FR-001–007 / SC-001–002 | 编码/范围策略、质量旁路、已丢失信息与调整记录 |
| FR-008–011 / SC-005 | 规范gray/dbz、旧generic语义、报告v1 additive模式信息 |
| FR-012–016 / SC-003–004 | 精确路径资格、15/8、native direct优先、local显式声明与接口等价 |
| FR-017–020 / SC-006–007 | Pixel optional time/geometry、数值profile、稳定raw/新增处理身份与历史codec |
| FR-021–024 / SC-004/007–008 | 共用scheduler/错误/限制/取消、独立读回与逐维验证、活动文档 |

### 依赖与实施顺序约束

复用001质量/成果合同、002gray证据、003共用原生Core、004direct reflectivity边界；不以这些spec全部验收为前置。按[tasks.md](tasks.md)执行：Setup/Foundation → US1本地严格反算/SDK/CLI三模式与报告（T001–T021）→ US2活动命名/旧入口/wheel兼容 → US3来源质量/native优先、SDK replay和逐路径验收 → US4数值读写/NumericFile身份/共用事务、CLI replay成果 → 完整指南与验收索引。US1可完整执行quickstart第2节，不依赖US2；数值保存/新文件读回不属于US1检查点。并行组须遵循实际文件与前置依赖。

历史保护范围由T001的protected-artifacts.json固定：001–004既有spec/plan/tasks等文件、旧legacy-display fixtures/resources/原验证记录及证据引用的原raw/gray/规则文件；T086逐文件比对SHA。README.md/docs/为本次活动迁移对象，不纳入不变摘要检查；旧spec/历史报告/规则资源不得因活动文档迁移改写。005完成不自动关闭旧任务或修改roadmap状态。

## Complexity Tracking

没有已批准constitution的违规需要豁免。Pixel类型/统一view与identity提交入口是FR-003/017/018无time/CRS保存所需的最小扩展；替代方案造time/CRS或全局放宽RadarField会破坏合同。origin_quality/encoding_adjustment是FR-006及已确认截断位置可读回所需，不能用只有计数的provenance替代。

## Plan Completion

Phase 0/1完成，post-design gates通过；tasks已生成并在2026-10-03同步修订设计问题。before_plan/after_plan均无注册hook；实际Git分支仍main，逻辑特性为005-gray-dbz-decoding。本次只修订005文档，没有改源码/001–004的spec/plan/tasks或历史证据，没有执行指南中的产品测试。设计通过不等于功能验收通过。
