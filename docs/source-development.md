# Source development

来源只负责来源知识，不负责公共 pipeline。最小契约是：

```python
Source.info -> SourceInfo
await Source.discover(query, context) -> list[FrameRef]
await Source.download(ref, context) -> RawFrame
Source.decode(raw, context) -> RadarField | RadarDataset
```

`FrameRef` 必须包含带时区的 `valid_time`，影响内容的定位字段放在有版本的 `locator` 中。动态 URL、签名参数、cookie、凭据和临时路径不能进入长期身份。下载阶段必须返回有名称、角色、MIME、大小和 SHA-256 的 artifact；获取时间不能替代产品有效时间。

decoder 必须只读 RawFrame 和版本化资源。精确 palette 优先；近似匹配必须有距离阈值；未知颜色在 strict 模式失败，在显式 permissive 模式写入 NaN 和 `unknown_color`。透明、站外、低于检测阈值和已知无雨要分别表达。

新来源需要：

1. 静态资源描述加入 `python/radiust/resources/catalog.json`，并声明 `availability` 和 required extras。
2. 固定一个可追溯、可合法使用的原始样本和发现响应，记录 hash、时间绑定、几何控制点、参考像素和容差。
3. 先写 source contract，再实现 discover/download/decode；公网测试与 fixture 测试分离。
4. 更新 `migration/sources/<id>.json`，说明旧实现去向、行为差异和历史错误修正。

没有真实 raw 或可靠科学语义时，来源可以出现在目录中，但必须保持 `needs_configuration` 或迁移阻塞状态，不能用合成图、旧 display PNG 或简单的 merge 结果宣称迁移完成。
