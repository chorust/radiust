# RDCAP 提供者请求与解析合同

范围：TWN/JPN/PHL单站reflectivity近期索引。提供者页面：[雷达地图](https://rdcap.cwa.gov.tw/data_access/radar_map)、[花莲单站](https://rdcap.cwa.gov.tw/data_access/radar_display/TWN/RCHL)。以下来自2026-10-01实测，不是提供者公开API保证；生产设计需单独验收。

## 目录和索引请求

| 方法/地址 | form | 用途 |
|---|---|---|
| POST /data_access/get_country_list | 空 | 将Country名称映射到提供者country code |
| POST /data_access/get_radar_list | 空，或radar_name[]=RCHL | 获取目录；全目录后仅取三国 |
| POST /data_access/get_radar_data | country=TWN&radar_name=RCHL&datetime= | 获取当前滚动时间线 |
| GET /file?ft=opaque_ticket | 无 | 选中帧原始文件；不自行构造ft |

基础origin为https://rdcap.cwa.gov.tw。POST form使用application/x-www-form-urlencoded编码；所有AJAX请求携带X-Requested-With: XMLHttpRequest，对应站点Referer，Accept允许JSON。无XHR header实测被拒绝。User-Agent可使用工具正常标识，不把伪装浏览器作为成功条件。网络只能经共用HttpTransport，无专用无限制client。

索引响应形状：

```json
{
  "header": "https://rdcap.cwa.gov.tw/file?ft=<opaque>",
  "list": [
    {"key": "1790834708000", "url": ["https://rdcap.cwa.gov.tw/file?ft=<opaque>"]}
  ]
}
```

key为整数字符串毫秒UTC；验证溢出和chrono范围，不用float。选中帧当前验证为一个URL；零或多URL的未知产品结构不给科学成功。header是单独元信息ticket，完整frame已含头，不GET该header。

目录以country/code合并，过滤之外国家不声明支持；原始名称/Status/Owner/坐标只作元信息，缺失明确unknown。状态冲突完整保留。任何目录/时间请求的HTML拒绝页、错误文本、意外JSON对象都不能因HTTP200而接受。

## 查询选择

每次发现先在总期限内最多做一次目录刷新（country/radar请求共享），归一化用户站码后交给已有目标调度。无选择展开全部，未知明确错误；刷新失败不丢已知目标，不伪造目录状态。默认list为内置离线快照，注明快照日期。

索引按真实时间排序，latest按站点取最新、at精确、range半开；相同key仅ticket变化去重。有稳定描述冲突时ambiguous_index；相同key文件内容差异形成revision冲突，不能随机发布。空list为no_data，非空但at/range无匹配为no_matching_time。超出窗口不给归档回退，不发送未验证datetime历史参数。

## 获取状态机与single-attempt

1. 选定country/code/key，锁定FrameRef身份。先查完整raw cache。
2. 验证提供者返回URL是HTTPS、host=rdcap.cwa.gov.tw、path=/file、包含非空ft，无userinfo或fragment。其他查询字段/结构不明则拒绝。允许的redirect只在已审查同origin路径内，不能转交票据至他处。
3. 不HEAD、不预读，single-attempt GET直接流式保存完整响应和摘要；不对同ticket发第二次GET。
4. 校验HTTP状态/字节上限，并确认UTF-8 JSON string非空；HTTP200的空string、空bytes或HTML不是成功。body可合法保存但science格式未知时留raw、decode_unverified。
5. 仅可重试失败/消耗ticket空内容时刷新索引，匹配相同key重新取得ticket，最多两次刷新/三次file GET。原key消失为selected_frame_disappeared；新索引最新帧不能替代旧帧。明确访问拒绝和损坏非重试错误直接终止。
6. 刷新、网络重试、解析、等待全部共享请求/host预算与同一帧deadline；取消后迟到结果不能commit。索引POST可使用既有三次暂时性错误重试，但不能产生无界嵌套ticket重试。

single-attempt为新增HTTP策略；默认策略和其他source保持不变。预算统计包含每次真正HTTP请求；成功内容在本次生命周期内多消费者共享已保存字节，不能靠再请求ticket共享。

## 文件格式

HTTP body是JSON string，解析后是七行文本：

```text
901,901,T,117.12,19.49,126.12,28.49,int16,-999,-999,EPSG:4326
linearTransform(0.1000,0.0000)
<r,g,b,raw_lower_threshold 四元组序列>
ndv:<NNZ>
colIdx:<NNZ个0-based整数>
rowPtr:<H+1个整数>
vals:<NNZ个int16>
```

允许正常LF/CRLF和末尾换行；未知额外字段/重复行不静默忽略。字节、行token数、维度/NNZ checked arithmetic在分配前检查。ndv不是缺测值，稀疏值与网格行为见[data-model](../data-model.md)。scale=.1、offset=0、T、int16、EPSG:4326、default/invalid=-999与验证legend是本版science适用门槛；不同CRS、注册、值变换或颜色模式为decode_unverified，结构不合法为invalid_grid。

文件无country/time属性，绑定依赖本次选中的索引country/code/key→返回文件；HTTP content-type只能参考，不能代替实际解析验证。raw-only只做获取合法性/非空envelope和绑定校验，不执行dense解码、mosaic、regrid或PNG生成。

## 错误分类

| 条件 | 安全code/阶段 | 公共结果 |
|---|---|---|
| 目录已知不存在 / 刷新不可用未知站 | unknown_station / catalog_unavailable，discover | 显式失败，不空成功 |
| 空时间线 / 时间无匹配 | no_data / no_matching_time，discover | no_data，可区分code |
| 最新过旧 / 稳定冲突 | stale / ambiguous_index，discover | stale / ambiguous |
| 同key消失 / 票据耗尽 | selected_frame_disappeared / ticket_exhausted，acquire | failed |
| 禁网/访问拒绝 / deadline | network_disabled或access_restricted / timeout | 对应限制/timeout终态 |
| HTML/意外JSON / 未验证科学 / 损坏CSR | unexpected_body / decode_unverified / invalid_grid | failed，分别acquire/decode |
| 配置超限 / 取消 | resource_limit / cancelled | failed / cancelled |

沿用现有report envelope、终态/退出码及SDK错误类型，通过新增typed source error→SafeError/ErrorReport映射传递code、stage、retryable。字段不得含ft、ticket、完整下载URL、请求cookies、HTML反射内容或本地敏感路径。
