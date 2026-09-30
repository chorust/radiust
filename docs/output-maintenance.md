# Output maintenance

正式输出和 cache 是两个生命周期。cache 位于 `~/.cache/radiust`（或显式 `--cache-dir`），可以被损坏驱逐、过期回收和清空；`--raw` 写入正式输出目录，并由 manifest 管理，cache GC 不会触碰它。

本地输出组先写 staging，再校验文件，最后原子替换 manifest。只有 manifest 完整、每个 artifact 存在且大小和 SHA-256 匹配时，下一次运行才会报告 `skipped`。缺文件或损坏清单会触发可修复写入；同路径不同 output identity 需要 `--overwrite`。

排查时可以查找：

```bash
radiust cache status --json
radiust cache gc --dry-run --json
find ./data -name '*.manifest.json' -print
```

staging 目录、无效 manifest 和未完成生成应在确认没有活动 writer 后清理。正式目录必须与 cache 根目录分离；cache clear 不能代替 output 维护，也不能删除正式 raw。

OSS/S3 使用同一套 generation、verify、commit fence、manifest-last 协议；上传失败的 generation 保留可识别前缀，待显式远端维护策略回收。标准 wheel 已包含 OpenDAL 适配器，真实 provider 的 AWS S3、S3-compatible、Aliyun OSS 矩阵仍需单独执行，未验证的 provider 不应写入支持声明。

运行 provider smoke 需要显式设置 `RADIUST_TEST_ALLOW_PROVIDER=1`，并配置 `RADIUST_PROVIDER_AWS_S3_URI`、`RADIUST_PROVIDER_S3_URI` 或 `RADIUST_PROVIDER_OSS_URI`。URI 必须指向包含 `test` 的专用路径前缀；每次执行只会在其下创建唯一的 `radiust-validation-<id>` 子路径。S3-compatible 与 OSS 可分别设置 `RADIUST_PROVIDER_S3_ENDPOINT`/`RADIUST_PROVIDER_OSS_ENDPOINT`，以及对应凭据环境变量；AWS 可使用 credential chain。凭据只从进程环境读取，不写入 fixture 或报告。未 opt-in 时测试会显示 `provider status=unverified` 并跳过；创建的远端验证对象需要由专用测试桶的生命周期规则回收。

## Rust-native migration boundary

原生 Rust cache 默认位于 `~/.cache/radiust-rust`，与 Python 的 `~/.cache/radiust` 隔离。Python cache 使用 `entries` 索引；Rust cache 使用独立的 native objects 索引。Rust 打开 cache 时会拒绝旧 `entries` schema 和没有 Rust 索引的既有对象目录，不会读取、修复或清理 Python cache。跨版本读取、同根锁和 repair 合同完成前，不要把 `runtime.cache_root` 指向 Python cache，也不要手工合并两种目录。

Python 现有正式输出按 v1 manifest 校验、逐 artifact SHA-256 读回，并在最后发布 manifest。Rust 已验证读取 Python 生成的旧 v1 fixture，并提供本地 manifest-last raw、PNG+sidecar、NetCDF4、GeoTIFF 和 Zarr v2 提交；离线测试覆盖 skip、repair、rollback、三件组/嵌套 artifact 完整性和重复提交。RainViewer composite 与 TW grid 的四种解码格式已接入 Engine；独立 Python fixture 读回覆盖 PNG、NetCDF4、GeoTIFF 和 Zarr。实际 provider 下载产物的跨实现读回仍未完成。Rust 对象存储的 `write_stream` 与 `read_to_path` 尚未串接到远端 generation/pointer manifest-last 提交。AWS S3、S3-compatible 与 Aliyun OSS 的真实 provider 验收仍单独记录为未验证；本地或内存测试不能替代 provider 证据。

Rust `ProcessingSpec` 把 `decoder_version`、`resource_version` 和 `encoder_version` 纳入 processing hash，默认值目前均为 `1`。改变会影响解码像素/数值、来源规则资源或编码语义时，必须提升相应版本，使新的 `processing_hash` 和 `output_id` 不会被当作旧成果而跳过。这个身份规则已用于本地 raw/PNG 提交；远端提交和其余格式仍待实现。
