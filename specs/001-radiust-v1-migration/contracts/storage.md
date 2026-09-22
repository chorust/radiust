# Storage, Manifest and Cache Contract v1

对应 FR-005～007、018～027。实体见 [data-model.md](../data-model.md)，取舍见 [research.md](../research.md)。

## Output layout 与身份

默认逻辑组路径：

```text
<root>/source=<source>/product=<product>/date=YYYY-MM-DD/hour=HH/
  <station-or-composite>_<valid_time>[_base-<base_time>]_<variant_id>.<ext>
  <same-stem>.manifest.json
  raw/<same-stem>/raw-manifest.json
  raw/<same-stem>/<original-artifact-name>
```

日期和小时均来自 UTC valid_time，文件时间格式 `YYYYMMDDTHHmmssZ`，若来源存在非零微秒则加六位微秒，避免截断身份。NetCDF `.nc`、GeoTIFF `.tif`、PNG `.png`、Zarr `.zarr/`。raw-only 使用独立 output_kind 与 variant，无科学主文件。

对象存储保留上述“逻辑组 manifest”路径，但真实数据置于同一分区的 `_generations/<output_id>/<unique-generation>/` 下；清单中的 URI 指向实际不可变文件，不能要求所有远端科学文件都直接占据可变逻辑路径。generation 采用随机唯一 id，不能只用 output_id，否则 overwrite 同身份会原地修改旧文件。

模板字段白名单：source/product/station/valid_time/base_time/date/hour/variant_id/ext。解析后的路径必须处于 root 内，禁止绝对路径、`..`、编码后路径逃逸和本地 symlink 逃逸；以规范路径/对象 key 做最终检查。一个自定义模板路径已有不同完整身份时报 OutputConflict；显式 overwrite 才进入替换流程。

## 完成条件与 skip

一次输出请求完成需同时满足：

1. manifest schema 受支持，logical_id/revision/processing_hash/output_id 计算一致；短 variant 仅用于命名。
2. 全部 artifact 存在、大小和 SHA-256 可验证。目录输出逐文件列入清单；不能只记录 Zarr 目录大小。
3. 请求 raw 时 `raw_complete=true`，原始清单完整且 revision 与科学结果相同；decoded-only 不能据此 skip。
4. 请求 raw-only 时 output_kind 必须匹配，不能当成 decoded 成功。
5. immutable 且 revision 可信时可下载前验证并 skip；mutable/未知 revision 先重新发现/校验/获取。动态 latest 不按固定 URL 永久跳过。

远端 provider 没有可信内容 checksum 时流式读回 hash；ETag（尤其 multipart）或自写 metadata 不自动等于 SHA-256。现有 manifest 损坏或文件缺失属于 incomplete，可修复；完整有效但身份不同属于 conflict。

## 提交状态机

```text
validate → discover → acquire/validate revision
                       ↓
                 decode/regrid（raw-only 跳过）
                       ↓
                   stage files
                       ↓
             verify all file receipts
                       ↓
                  commit fence
                       ↓
              publish final manifest
                       ↓
                   committed
```

- 每一步失败保留阶段、帧、可重试性与安全错误。commit fence 前已接受取消的工作不能进入 publish。
- final manifest 写入已经开始时，取消不能撤回远端已接受的请求。必须保护有界收尾、重新查询结果，报告实际 committed 或 `commit_outcome_unknown`；unknown 归失败，提示安全重查，不能宣称成功也不能直接覆盖重试。
- Ctrl-C 整体退出 130；先前 committed 帧仍记录 written，未开始/取消帧明确标记。任务启动时接受取消后迟到 CPU 结果丢弃。
- 清理失败追加诊断，不掩盖原始错误；未发布 generation 保持可识别，后续显式维护回收。

### Local

- 创建 root 的独占 advisory lock；同 root 第二个 writer 立即报 OutputLockedError。锁是进程生存锁，崩溃后释放，不凭空锁文件判断活跃。
- 所有 staging 与目标同文件系统。临时文件写完 flush/fsync 后验证，再 rename；完成清单最后原子 rename，并同步必要父目录。
- 替换/修复过程中先移除或隔离旧完成清单，确保它不指向半更新数据。不是整组跨文件原子事务：读者必须以当前有效清单为边界，验证失败则重试或报告未完成。
- 根目录既有不同身份且未 overwrite 时不变更；cache gc 不删除 output staging。

### OSS / S3-compatible

- 使用不可变 generation；上传全部文件、close writer、校验 receipts、写 generation manifest，再更新逻辑组 manifest。根清单是读者公开完成边界。
- 同 root 单 writer 是使用条件，v1 不提供跨机器锁/恰好一次。支持条件写的 provider 可额外保护，但不能把不支持条件写视为支持并发。
- overwrite 上传新 generation；旧 generation 和旧 manifest 证据留存至用户显式输出维护。逻辑组切换失败保留旧版；响应丢失先读取核对。
- multipart 中断调用 abort，失败记录安全 upload/generation id，配套 bucket 生命周期回收。普通 cache clear/gc 没有删除远端正式文件的权限路径。
- 标准 wheel 编入 OSS/S3 功能。storage extra 是兼容安装名，不启用 Rust feature。provider capability 声明按实测矩阵发布。

## Manifest 字段约束

| 字段 | 必需性与规则 |
| --- | --- |
| schema_version | 必需，整数 1 |
| logical_id/revision/output_id | 必需，完整身份；revision 包括上游命名空间或内容清单 hash |
| processing_spec/processing_hash | 必需，相互一致；raw-only 无 decoder/regrid |
| generation | 远端必需，本地可省略 |
| artifacts | 非空，name/relative_uri/role/media_type/size_bytes/sha256；名称唯一且路径受限 |
| raw_complete | 布尔；只有原始集合完整且 raw-manifest 校验通过才为 true |
| created_at | UTC aware 固定格式 |
| supersedes | overwrite 时记录前一 manifest/output/generation 标识，不写秘密 |

`raw-manifest.json` 保存安全 FrameRef、receipt、全部 original_artifacts 与相对路径。离线重放只读取经大小/hash 校验的本地资料，不执行文件中的 URL、代码或任意类加载。

## 内部 storage 接口

`stage(request) -> StagedGroup`；`write_artifact(staged, name, stream) -> Receipt`；`verify(staged) -> VerifiedGroup`；`commit(verified, cancellation) -> CommitReceipt`；`abort(staged) -> CleanupReport`；`inspect(group_path) -> ManifestStatus`。

Python 负责 processing/output identity 与整组状态机，Rust 负责字节流、临时文件、hash、网络 writer 与提交原语。Receipt 不包含凭据；commit 不自行重试整帧或重新 decode。权限、认证、配额与路径错误不可当瞬时网络错误无限重试。

## Cache

默认 `~/.cache/radiust/{objects,mosaics,tmp}` + `index.sqlite`。目录受 ownership marker 保护；cache path 与 output root 重合/包含关系拒绝。cache key 基于稳定帧、版本/validator、acquisition/mosaic 版本，decode 参数不污染原始缓存。

- 写入 tmp → 校验 → atomic rename → 短 SQLite transaction 更新索引。启动时扫描可修复索引与孤儿文件，不保持跨网络事务。
- lease 覆盖实际读者/worker 生命周期；GC、clear 遇到 lease 跳过并报告。lease 使用跨进程锁，不仅进程内引用计数。
- 相同 key 同进程合并下载；跨进程每 key 锁避免重复提交。可变版本 revalidate，损坏条目驱逐再获取。
- 单 tile 不长期缓存；拼接缓存保留原始 receipt 摘要，但不承诺能从 mosaic 重建原始 tiles。raw 请求缺 tile 必须重取并确认 revision。
- GC 顺序 stale tmp → expired → LRU 至低于 20GB；max_age=30d，gc_interval=24h。被 lease 占满时报告不能回收的数量，不能破坏在用数据或无限循环。
- no-cache 不读写长期条目，仍用有界 tmp；正式 raw 绝不由 RawFrame 或缓存所有权删除。

## 必测矩阵

local、AWS S3、至少一个 S3-compatible（优先 MinIO 本地服务）、Aliyun OSS，分别覆盖 decoded/raw/raw-only、首次/skip/补raw/覆盖/冲突、文件与清单损坏、短 hash 碰撞、上传中断、最后提交响应丢失、取消 fence、锁冲突或远端单写者前提。provider 未实测明确标未验证，不能用 mock 全绿代替真实服务支持声明。
