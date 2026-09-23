# CLI 契约

## 时间与全量发现

`discover SOURCE`、`download SOURCE`、`cat SOURCE` 无时间选择器时等价显式 `--latest`。显式 latest 与 at/range 冲突仍退出 2；不完整范围、非法时间不被默认值掩盖。单来源默认产品、base-time、max-age 不变；latest 按产品/站点取最新，同时间歧义不得任取。download/discover 可返回多帧，cat 必须唯一；本地 NetCDF 文件多时刻仍必须显式选择。

`discover all [--latest] [--max-age SECONDS] [--json]`：查询静态目录的所有有效产品/站点组合；禁止 at/start/end/base-time/product/station，在调用任何适配器或网络前退出 2。all 是 discover 的保留聚合目标，不扩展到 download/cat。

整批预算新增 `runtime.discovery_deadline`，正有限秒数，默认 300；环境配置 `RADIUST_RUNTIME__DISCOVERY_DEADLINE`。沿用现有 `--conf` 配置入口，不复用并改变 frame_deadline。预算从开始目录展开计算，所有目标共享同一 monotonic deadline。并行目标不超过 frame_concurrency，网络还须服从 request_concurrency、host_concurrency 和来源更严格限制。预算结束后 5 秒内完成最终报告和退出；不允许每个来源重新得到 300 秒。

退出码：中断优先 130；所有目标 success 为 0；success 与其他状态混合为 4；无 success 且仅 no_data/stale 为 3；其他无 success 为 5；参数错误为 2。空目录返回 3，并显式报告没有目标。

## raw 预览

来源模式 `cat SOURCE` 默认等同 `--raw`，原样预览 PNG/GIF；`--raw` 明确选择原图模式，`--decoded` 显式调用科学解码器，`--legacy-display` 显式启用匹配且验证通过的旧显示转换，三种模式互斥。raw 原图模式不套用任何灰度转换。

`cat --file PATH --renderer text` 离线读取 PNG/GIF/既有科学文件；来源模式选项 `--raw`、`--decoded`、`--legacy-display` 不能与 `--file` 同用，退出 2。raw 来源模式与图片文件模式禁止科学变量、物理范围和科学配色选项，即便值等于默认值也按显式传入判断。宽高只用于展示。

非 TTY stdout 仅接受显式 text；其他 renderer 在获取之前失败。auto 沿用现有终端能力判定且探测有界；显式不支持的图形协议失败。GIF 使用首画面并注明。text 输出尺寸、身份、raw、已知时间、显示模式及规则版本；未知值明确 unknown。图片仅按真实格式识别并受字节/像素/临时盘上限约束。

只有显式 `--legacy-display` 时才尝试应用匹配规则；无匹配或未验证规则可预览原图并给出原因。已匹配规则执行失败必须报错。多 artifact 仅进入已验证的完整组合路径，否则给出下载完整资料建议。普通本地图片没有来源证据则原图预览；不新增凭空指定 source 即强制转换的选项。

## 文本、进度及安全

list/discover/download 显示标题、身份、状态、UTC时间/输出路径与计数；doctor/config/cache 使用分组键值。40 列降为逐条布局，80/120 列优先表格；长文本换行，关键字段不截断。中文及组合字符使用显示宽度而非字节长度处理，禁用颜色后仍能读懂状态。

stderr TTY 才可动态更新阶段和 completed/total；未知 total 显示完成数。最终报告/图片之前结束进度，所有退出路径恢复光标。JSON stdout 只输出一个完整对象；不含进度或图片序列。非 TTY 和 NO_COLOR 路径不产生终端控制序列（NO_COLOR 时进度可使用普通追加文本）。quiet 隐藏人类报告和进度，保留显式 JSON 和错误；quiet/verbose 互斥。

报告、异常和诊断共用递归脱敏与控制字符安全转换，不只处理 config。保留 JSON 字段名称和类型；危险字符串转换为安全可见文本，不能吞掉身份、原因。禁止把未知异常的任意响应正文直接发布。
