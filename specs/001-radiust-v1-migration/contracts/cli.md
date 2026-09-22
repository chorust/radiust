# CLI Contract v1

命令是目标契约，当前骨架尚未实现。入口 `radiust`，所有业务调用 [SDK](python-sdk.md)。一次命令为有界任务，不内置轮询服务。

## 命令表

| 命令 | 用途 | 联网条件 |
| --- | --- | --- |
| `list sources`、`list SOURCE` | 来源目录、详情 | 不联网 |
| `list products SOURCE`、`list stations SOURCE` | 产品与站点 | 不联网 |
| `discover SOURCE QUERY` | 帧引用 | 按来源发现 |
| `download SOURCE QUERY` | 获取、解码、可选转换、保存 | 按来源能力 |
| `cat SOURCE QUERY`、`cat --file PATH` | 单帧展示 | file 不联网 |
| `config show [--redact]` | 已脱敏有效配置与来源 | 不联网，默认也脱敏 |
| `doctor [--source SOURCE] [--network]` | 依赖、配置、缓存可写性 | 仅 --network 允许探测 |
| `cache status`、`cache gc [--dry-run]`、`cache clear [--yes]` | 缓存维护 | 不联网 |

`--conf PATH`、`--json`、`--quiet`、`--verbose` 在支持的叶命令解析；conf 可放根命令或叶命令，重复指定时报错。quiet/verbose 互斥。cat 不接受 json。所有参数与依赖预检先于原始资料获取。

## 查询

- `--product ID`：仅来源有唯一默认产品时可省略。
- `--station ID` 可重复；`--base-time ISO8601` 预报起报筛选。
- `--latest` / `--at ISO8601` / `--start ISO8601 --end ISO8601` 恰一，无隐式 latest；时间必须带 Z 或 offset。
- `--max-age SECONDS` 为正秒数，仅 latest。latest 按来源/产品/站点分组；单帧预览遇到多匹配失败。
- download 支持区间；cat 仅 latest/at，不支持 start/end。

## 下载

| 参数 | 默认/规则 |
| --- | --- |
| `--output ROOT` | ./data；local、oss://、s3:// |
| `--format FORMAT` | netcdf；也支持 geotiff/png/zarr |
| `--raw` | decoded + 全部原始资料正式保存 |
| `--raw-only` | 仅原始资料及 raw-manifest，不解码、不拼接 |
| `--variable NAME` | PNG/GeoTIFF 多变量必须选择 |
| `--grid native/geographic` | native；geographic 需 bbox 与 resolution |
| `--bbox west,south,east,north` | 经纬度，跨日期变更线拒绝 |
| `--resolution NUMBER` | CLI geographic 单位度；SDK 其他目标按其 CRS 单位 |
| `--resampling nearest/bilinear` | nearest；类别拒绝 bilinear |
| `--output-template TEXT` | 白名单替换，不执行代码，不能逃出 ROOT |
| `--overwrite` | 重新校验/获取后替代，不能直接复用未校验缓存 |
| `--no-cache`、`--cache-dir PATH` | 禁用长期缓存/选择目录；允许有界临时文件 |
| `--dry-run` | 可发现与检查目标，只输出计划；不下载 artifact、不写 output |
| `--on-error continue/stop` | continue；stop 对应 SDK raise，保留部分结果 |
| `--access-key`、`--secret-key`、`--endpoint` | 显式优先；文档推荐环境凭据，参数值不得打印 |

raw/raw-only 互斥；raw-only 与用户显式 format/grid/variable/resampling 互斥（默认 format 的内部值不算用户显式输入）。地理参数只有显式 grid=geographic 才接受。不能把 PNG 当完整科学存档。

预演在 revision 尚未知时将 output_id/最终路径标为 unresolved 并给出路径模式，不能编造 hash 或提前声称 skip。可读已有 manifest 做目标检查，但没有可信 revision 不可断言复用。

## 终端 cat

- source 与 --file 二选一；file 仅接受本项目契约 NetCDF 或 PNG。多时间 NetCDF 用 --at、多变量用 --variable。file 拒绝 source/station/product/latest 等网络参数。
- 不接受 download 的 raw/raw-only/output/format/overwrite，也不接受 json。PNG 原样展示，palette/vmin/vmax/grid 等科学参数对 PNG 报错。
- `--renderer auto|kitty|iterm2|ansi|text` 默认 auto；`--width/--height` 为正列数/行数最大范围，给标题和色标留空间，保持栅格比例。
- `--palette ID`、`--vmin NUMBER`、`--vmax NUMBER` 显式覆盖连续配色，必须成有效区间；类别只用离散配色，拒绝连续上下限。
- 标题包含来源、产品、站点、UTC 有效/起报时间、变量、单位；未知信息标未知。图像按坐标方向显示；极坐标或不支持网格需显式转地理网格。
- auto 总探测 200ms：确认 Kitty → 确认 iTerm2 → ANSI/text；NO_COLOR、无颜色、TERM=dumb 时 text。tmux/screen 未确认 passthrough 时不发图片协议。
- stdout 非 TTY，除显式 text 外一律在获取前退出 2。不静默写 /dev/tty。探测、显示、取消均恢复终端模式。
- text 输出时间、变量、shape、单位、有效值范围和缺测比例，无图片/颜色控制序列。纯 PNG 只报告可证实图像属性。

## 报告与退出码

除 cat 外 stdout 只含一个最终人类报告或一个 JSON 对象；stderr 是日志/进度/警告。quiet 不抑制显式 json。JSON schema_version=1，顶层含 command、run_id、query（无查询时 null）、counts、items、error、interrupted；不同只读命令的 items 使用其实体投影。不得输出含认证信息的 ref.uri。

下载 counts 包含 written/skipped/failed/cancelled/not_started/total，并满足总和；items 含帧标识、status、输出 URI 与 error。预演 counts 为 planned，不能填 written。致命预检错误亦输出合法单一 JSON 对象（当 --json 已成功解析）。无法解析 --json 本身的语法错误只保证 stderr 与退出 2。

| 退出码 | 规则 |
| --- | --- |
| 0 | 全部成功或有效 skip；有效只读查询/预演成功 |
| 2 | 参数、配置、依赖或 renderer 预检失败 |
| 3 | 无数据，或所有匹配帧都超过时效阈值且无其他处理错误 |
| 4 | 至少一个 written/skipped 且至少一个失败；失败含逐帧无数据/过期 |
| 5 | 没有成功项，且存在获取/解码/输出等处理失败 |
| 130 | 用户中断；报告仍保留先前已提交帧 |

优先级：预检失败直接 2；开始处理后用户中断优先 130；正常结束根据全部结果判断 0/3/4/5。cat 无批量，使用 0/2/3/5/130。

## 配置与资源

有效优先级：显式参数 > 指定 YAML > `RADIUST_` 环境变量 > 包默认；provider 原生 credential chain 为认证兜底。环境映射采用 `RADIUST_<SECTION>__<FIELD>`，例如 `RADIUST_CACHE__DIR`；source map 再增加 source id 层，不接受无法映射的键。未知普通 RADIUST 配置报错；明确保留的 `RADIUST_TEST_` 命名空间由测试配置加载器处理，公共配置忽略它，不将其作为产品功能。

runtime/cache/sources/storage/output 各自 schema 校验；map 深合并、列表整体替换。配对 access/secret 必须来自同一层，半对凭据报错；session token 可随同一凭据组。有效快照不含凭据值，只显示字段来源与是否配置。

缓存默认 20GB（20,000,000,000 bytes）/30天/24小时检查间隔；GC 由命令生命周期触发且有界，无后台常驻器。clear 仅 radiust 管理且未借用的条目，TTY 确认、非 TTY 需 --yes。cache dir 与 output root 不能重合或互为父子；拒绝把输出目录注册成缓存。
