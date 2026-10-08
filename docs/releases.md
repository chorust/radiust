# Tag 自动发布

`.github/workflows/release.yml` 在推送 `vMAJOR.MINOR.PATCH` tag 时执行。tag 必须包含这套发布代码；仅接受三段稳定版本号，不接受前导零、预发布或任意分支名。版本 `0.x` 仍可作为明确标注能力范围的预览版，不代表所有来源或迁移规格已经验收。

## v0.1.7 下载与输出路径收敛

相较 `v0.1.6`，本版本清理 Rust 迁移后不再使用的 Python 模块、recovery/scraping 可选依赖、旧扩展回退和未使用的 Rust helper。Python/PyO3 科学下载统一经过格式感知的 Rust 方法，并复用输入解析、科学输出编码、raw manifest 序列化和 Zarr 输出枚举逻辑。来源时间选择和取消策略保持原有行为；本次没有新增来源在线能力。

PR #1 的 48 项 GitHub 检查全部通过。PR 验证记录报告：离线 Rust workspace 712 项通过、4 项忽略；Python suite 384 项通过、26 项跳过；最终 review 修复另有 27 项 Rust 检查和 43 项 Python 测试通过。live 上游与凭据型 provider 验证未运行。tag 发布工作流还会独立构建并验证两个 crate 和 CPython 3.10–3.13 wheel，再发布到 crates.io 和 PyPI。

## v0.1.6 RDCAP 站点 ID

本版本将 RDCAP 对外站点 ID 统一为两字母国家前缀加上游站码，例如 `TWN/RCHL` → `TWRCHL`、`JPN/MAKI` → `JPMAKI`、`PHL/SUBI` → `PHSUBI`。CLI、SDK、目录、帧身份和输出路径使用新格式；上游请求及元数据中的三字母国家代码保持不变。旧的含斜杠 ID 不再接受，短站码仍只在目录中唯一时解析。该变化不代表新增在线来源能力。

针对性检查通过：32 个 Rust RDCAP 单元测试、42 个 RDCAP contract/source-matrix 测试、12 个 CLI 测试和 43 个 Python 测试。站点目录和输出格式验证仅覆盖离线样本及 loopback 合同，不代表公开上游验收。一次较早验证的原始结果保留在 [`rdcap-station-ids-20261007.json`](../validation-results/rdcap-station-ids-20261007.json)，包括当时两项 `tw/observation` 回归失败；后续测试已按当前行为更新并通过。

## v0.1.5 RDCAP 修复

本补丁新增 RDCAP 的显式 TLS 校验例外 `sources.rdcap.insecure_tls: true`（默认仍校验证书），并保留匿名会话 cookie，使申请索引与读取文件票据使用同一内存会话。配置例外仅适用于 RDCAP HTTPS 主机，其他来源与主机保持正常证书校验；不输出该配置的 warning，网络 opt-in、请求限额、超时、取消与票据重试规则保持不变。

[MAKI 在线验证](../validation-results/rdcap-session-live-20261007.json)在禁用缓存后成功获取原始数据并完成 `--decoded` / `--dbz` 数值预览；[去掉 warning 后的对照](../validation-results/rdcap-no-warning-live-20261007.json)确认预览成功且 stderr 为空。RDCAP 返回数值网格，`--gray` 来源图像模式不适用。当前单站证据不代表三国科学能力或独立输出读回全部验收。

`v0.1.3` 与 `v0.1.4` 的构建发布均在 registry 上传之前取消，tag 保留不改写，包未上传。[发布工作流](https://github.com/chorust/radiust/actions/runs/37593168357)已全部通过，两个 crate 和八个 wheel 已由工作流发布；PyPI Trusted Publishing 成功。Linux x86_64 与 macOS arm64 均从公开 crates.io 安装 CLI，并从公开 PyPI 安装 CPython 3.12 wheel，SDK 与 CLI 检查通过。八个公开 wheel 的 SHA-256 均与 CI 验证产物一致；另从公开 PyPI 安装的包完成 MAKI 无缓存数值预览，stderr 为空。详见 [`tag-release-v0.1.5.json`](../validation-results/tag-release-v0.1.5.json)。

## v0.1.2 发布结果（2026-10-06）

[GitHub Actions run 37466179486](https://github.com/chorust/radiust/actions/runs/37466179486) 已通过源码准备、发布检查、Linux/macOS crate 打包与独立安装、八个 wheel 构建与隔离安装；`radiust-core` 和 `radiust-cli` 已由工作流发布到 crates.io。Python 发布 job 的 OIDC 交换返回 `invalid-publisher`，所以八个经 CI 验证的同一 wheel 改用本地 PyPI 凭据上传。公开 PyPI 摘要与 CI 产物一致，CPython 3.10–3.13 的 macOS arm64 和 Linux x86_64 环境均从公开索引安装并通过 SDK/CLI 检查。详细摘要见 [`tag-release-v0.1.2.json`](../validation-results/tag-release-v0.1.2.json)。

失败 token 的 claims 是 owner `chorust`、repository `radiust`、workflow `.github/workflows/release.yml`、environment `pypi`。首次失败后需在 PyPI 的 `radiust` 项目 Publishing 设置核对这四项；已发布的 wheel 不能用同一版本重传来验证 OIDC。此记录保留首次发布的失败与回退经过；`v0.1.5` 已由新版本 tag 成功验证 Trusted Publishing。

## 首次配置

仓库管理员需完成以下一次性配置：

1. 在 GitHub 仓库的 Actions secrets 中设置 `CARGO_REGISTRY_TOKEN`（或确认同名组织 secret 已对本仓库开放），允许发布 `radiust-core` 和 `radiust-cli`；持有人须拥有对应 crate 的发布权限。首次发布 `radiust-cli` 时，token 还须允许创建该 crate。
2. 在 [PyPI 的 radiust 项目 Publishing 设置](https://pypi.org/manage/project/radiust/settings/publishing/)添加 GitHub Trusted Publisher：owner `chorust`，repository `radiust`，workflow `release.yml`，environment `pypi`。
3. 本仓库已建立名为 `pypi` 的 GitHub environment。工作流只在 Python 发布 job 申请 `id-token: write`，不需要保存 PyPI API token。若管理员为该 environment 设置了审批规则，发布将按该规则等待。

配置方法分别见 [Cargo publishing](https://doc.rust-lang.org/cargo/reference/publishing.html) 和 [PyPI Trusted Publishing](https://docs.pypi.org/trusted-publishers/using-a-publisher/)。凭据只用于发布 job，不进入源码快照、打包报告或构建 job。

## 发布步骤

将代码和工作流合入需要发布的提交后，创建新的未使用版本 tag，例如：

```bash
git tag -a v0.1.7 -m "Radiust 0.1.7"
git push origin v0.1.7
```

发布新版本时将 `git push` 命令中的 tag 与上方示例保持一致。`0.1.1` 首次发布使用本地凭据；`0.1.2` 的 crate 由 GitHub 发布、wheel 因 Trusted Publisher 不匹配而使用本地凭据发布。后续自动发布应使用新版本，不能用 CI 重建产物覆盖同版本。已有 GitHub `v0.1.0` 预览资产不应移动旧 tag 来替换其内容。

自动流程依次执行：

1. 校验 tag 对应的提交，生成仓库外的源码快照。根据 tag 同步三个 Rust crate、内部精确版本依赖、Python metadata／`__version__` 和两个锁文件。主分支和 tag 原始提交不被改写。
2. 从 Python 的规范资源复制 core 编译所需的 JSON，并复制 Apache-2.0 LICENSE；Rust `include_str!` 只引用 crate 内部资源。记录源码提交、版本和资源 SHA-256。日常修改资源后运行 `python scripts/release/prepare.py --sync-resources`；离线 CI 会检查这些副本是否一致。
3. 在 Linux x86_64 与 macOS arm64 上打包 core 和 CLI。把 `.crate` 解压到仓库外，构造隔离的目录 registry，验证新项目通过 registry 引用 core，以及 `cargo install --locked` 安装 CLI。验证时不使用工作区 path override，也不请求气象上游。
4. 构建 CPython 3.10–3.13 的 Linux x86_64 manylinux_2_28（glibc ≥ 2.28）和 macOS arm64 wheel。macOS 执行 delocate 和 Mach-O 审计；每个 wheel 在仓库外的全新虚拟环境中安装，验证版本、SDK、CLI 和内置目录，并拒绝带入 `tests/fixtures/` 下仅用于验证的样本。尚未确认再分发授权的历史气象图片不进入源码发布快照或 wheel。基础 wheel 无需 Rust、NumPy 或 xarray。
5. 发布脚本的版本／冲突处理测试、wheel metadata 检查和全部独立安装验证通过后，比较将要上传的 `.crate` 与已验证产物的 SHA-256，发布 core，等待 crates.io 索引可见，再发布 CLI。随后通过 Trusted Publishing 上传已验证的 wheel。
6. 在两个平台再从公开 registry 安装指定版本：`cargo install --locked --version VERSION radiust-cli` 和 `pip install --only-binary=:all: radiust==VERSION`。

每个 job 保存源码快照、`.crate`／wheel 或安装报告作为 Actions artifact。`release-info.json` 记录 tag、版本、源码提交和资源摘要。发布可移植 wheel 的范围不等于该平台已经完成所有来源、科学能力或真实存储 provider 验收；macOS 11 deployment target／二进制审计也不代表已经在真实 macOS 11 主机运行。

本流程暂不上传 Python sdist，也不发布 `radiust-python` Rust crate（它是 wheel 的内部 PyO3 包）。Linux arm64、macOS Intel 和 Windows 不在当前 tag 发布矩阵中。已有 `wheels.yml` 的构建测试矩阵保持独立。

## 预演和失败重试

GitHub Actions 的 `release` 工作流支持手动运行：填写含有发布工作流的已有 tag，保持 `dry_run=true`，只构建和验证；取消 `dry_run` 可以重试发布。

本地可先生成快照并在离线依赖已缓存的环境中检查：

```bash
python3 scripts/release/prepare.py --tag v0.1.7 --output /tmp/radiust-release-source
python3 scripts/release/verify_crates.py --source /tmp/radiust-release-source \
  --output /tmp/radiust-release-artifacts --offline
```

`verify_crates.py` 要求 Python 3.12 或更新版本；Rust/CMake/C/C++ 前置工具与[源码安装](installation.md#从源码构建或使用-cargo-安装)一致。`--offline` 需要 Cargo 的依赖和索引已缓存，不把本地 registry 验证当作真实公网发布验收。

registry 发布无法跨两个服务原子提交。若 core／CLI 已上传而后续失败，重跑同一 tag：已存在且摘要相同的 crate／wheel 会跳过；已存在但内容不同、被 yank 的 crate 或 registry 访问错误会使流程失败，不能覆盖同版本。macOS wheel 构建和 delocate 修复使用 tag 所指提交的时间作为 `SOURCE_DATE_EPOCH`，固定 ZIP 条目时间戳，避免重试时间改变产物摘要。此设置不能修复此前已上传的、使用非固定时间戳生成的 wheel；仍有摘要冲突时须使用新版本 tag。内容变化须使用新版本 tag。
