# Source development

项目方向见[理念](../PHILOSOPHY.md)与[发展路线](../ROADMAP.md)。新增来源既可以扩展国家／地区，也可以改善已有地区的稳定性与产品选择；官方优先，同时允许可靠且可追溯的第三方产品。

每次接入或改变产品／获取渠道时，同步更新[来源政策与权限台账](../DATA_SOURCES.md)：区分观测机构、产品提供方与分发渠道，记录访问条件、条款依据、署名、商用及再分发条件，未知项标为未确认。原始获取证据、图像展示、几何和科学解码分别验收；fixture 验证许可不自动成为产品再利用授权。目录登记本身不意味着可用覆盖。

adapter 保持用户设备可直接调用的获取边界，不要求 Radiust 账号或必经的 Radiust 数据代理；凭据由用户按来源配置，遵守现有网络许可、访问限制与资源预算。未来 Desktop 默认选择与回退将复用独立来源能力，回退后的实际来源必须可辨认。当前不因这份方向文档改变 CLI 默认选择、公共接口或网络 opt-in 行为。

来源只负责来源知识，不负责公共 pipeline。最小契约是：

```python
Source.info -> SourceInfo
await Source.discover(query, context) -> list[FrameRef]
await Source.download(ref, context) -> RawFrame
Source.decode(raw, context) -> RadarField | RadarDataset
```

`FrameRef` 必须包含带时区的 `valid_time`，影响内容的定位字段放在有版本的 `locator` 中。动态 URL、签名参数、cookie、凭据和临时路径不能进入长期身份。下载阶段必须返回有名称、角色、MIME、大小和 SHA-256 的 artifact；获取时间不能替代产品有效时间。

decoder 必须只读 RawFrame 和版本化资源。精确 palette 优先；近似匹配必须有距离阈值；未知颜色在 strict 模式失败，在显式 permissive 模式写入 NaN 和 `unknown_color`。透明、站外、低于检测阈值和已知无雨要分别表达。

## Gray-display and source dBZ rules

Source image gray rules live in `python/radiust/resources/gray/` and are consumed by the shared Rust gray pipeline. A rule is eligible only for its exact source/product/path/station and declared artifact constraints. The retained evidence record must identify the historical operation order, rule/config hashes, source input and same-frame gray baseline. `tests/fixtures/gray-dbz/paths.json` records whether that evidence is passed or blocked; do not broaden a one-station or one-product sample to a whole source.

The `gray` result preserves the historical display pixels. A source `dbz` result requires a passed gray decision and the verified `gray-dbz-v1` convention; it stores numeric reflectivity, quality/origin masks, adjustment flags and processing identity in the Rust Core. The local declaration is strict 0–224. Source paths may use the evidence-bound upper clip at 224 only during numeric conversion; the source gray image remains unchanged, and invalid pixels remain invalid. This encoding agreement is not independent physical calibration. FR/PT brightness-alpha operations, repair, resize and quantization must be described as limitations because the mapping cannot be inverted to recover original reflectivity.

When writing a rule, test gray pixels and source dBZ values separately. Keep four result axes separate: historical gray parity, numeric dBZ formula/quality, geographic mapping, and live provider availability. Unknown time or geometry remains unknown. A raw fixture without a real `FrameRef` and acquisition receipt may support a gray-image comparison, but it cannot be promoted to a source-bound production dBZ result or a live source claim.

新来源需要：

1. 静态资源描述加入 `python/radiust/resources/catalog.json`，并声明 `availability` 和 required extras。
2. 固定一个可追溯、可合法使用的原始样本和发现响应，记录 hash、时间绑定、几何控制点、参考像素和容差。
3. 先写 source contract，再实现 discover/download/decode；公网测试与 fixture 测试分离。
4. 更新 `migration/sources/<id>.json`，说明旧实现去向、行为差异和历史错误修正。

没有真实 raw 或可靠科学语义时，来源可以出现在目录中，但必须保持 `needs_configuration` 或迁移阻塞状态，不能用合成图、旧 display PNG 或简单的 merge 结果宣称迁移完成。

## Rust core 迁移

当前 Python `Source` 接口和 `radiust.sources` entry point 仍可用于现有 Python pipeline；它们不会作为运行时插件自动注入原生 Rust CLI。迁移后的内置 adapter 实现 `radiust_core::source::SourceAdapter` 并注册到编译期 `SourceRegistry`，通过共享 `SourceContext` 使用网络 opt-in、请求/主机预算、取消、deadline 和临时目录。适配器只负责该来源的发现与获取边界；科学解码、输出提交和报告由 Engine 管理。

第三方扩展需把来源逻辑移植为 Rust adapter，声明并验证产品、站点、凭据和 capability，然后重新构建 wheel/应用。浏览器或 OCR 依赖应作为明确的可选系统能力探测；不可用时返回安全的受限状态，不绕过网络许可或回退执行任意 Python 插件。未通过原始样本、身份、科学像素/几何和独立读回的来源只能提供其已验证的发现/原始能力，不能宣称解码通过。
