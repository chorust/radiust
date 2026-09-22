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
