# 全量发现报告 v1

沿用 `schema_version: 1`；command 为 `discover`，query.source 为 `all`。现有单来源 discover 和 DownloadReport 字段及退出规则保持原义。本文件仅增加聚合报告类型。

```json
{
  "schema_version": 1,
  "command": "discover",
  "run_id": "example-run",
  "query": {"source": "all", "latest": true, "max_age": null},
  "counts": {
    "total": 2, "success": 1, "no_data": 0, "stale": 0,
    "missing_credentials": 0, "retired": 0, "network_restricted": 1,
    "upstream_failed": 0, "ambiguous": 0, "timeout": 0,
    "cancelled": 0, "not_started": 0
  },
  "items": [
    {
      "source": "example_a", "product": "observation", "station": null,
      "status": "success", "valid_time": "2026-09-22T00:00:00Z",
      "error": null,
      "frame": {"source": "example_a", "product": "observation", "station": null, "valid_time": "2026-09-22T00:00:00Z", "base_time": null},
      "capabilities": {"scientific_decode": false, "reason": "Scientific decoding is not validated"}
    },
    {
      "source": "example_b", "product": "observation", "station": null,
      "status": "network_restricted", "valid_time": null, "frame": null,
      "error": {"code": "network_restricted", "message": "Network access is disabled", "stage": "discover", "retryable": false},
      "capabilities": null
    }
  ],
  "error": null,
  "interrupted": false
}
```

示例来源是虚构契约样例，不代表当前来源在线状态。frame 是安全身份投影，不发布定位 URL、认证字段或整个 metadata。capabilities 可为空；不以缺科学解码器改变 status。非成功的 valid_time 通常为空；stale 可保留已知旧帧时间，ambiguous 不任意给一个时间/帧。

所有状态计数字段始终存在并包含零；counts.total 不参与状态求和。按 data-model.md 的空值优先规则排序。来源级占位项 product/station=null，错误必须解释无法展开的原因。异常记录只表示已知事实；未开始不能伪称超时的上游请求。

参数错误使用既有 error envelope，不启动目标；部分失败用 items[].error，不能以一个顶层异常丢弃已完成项。中断仍输出单一完整报告，interrupted=true、退出130。stdout 写入失败不重试输出第二份 JSON。
