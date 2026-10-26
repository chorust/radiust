import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var session: DesktopSessionStore

    var body: some View {
        Form {
            Section("联网") {
                Toggle(
                    "允许出站获取观测",
                    isOn: Binding(
                        get: { session.allowNetwork },
                        set: { session.setAllowNetwork($0) }
                    )
                )
                Text("默认开启；须向用户披露。关闭后不向公网拉取，可浏览已缓存帧（后续接线）。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section("架构") {
                LabeledContent("壳层", value: "SwiftUI")
                LabeledContent("引擎", value: "同进程 radiust-core")
                LabeledContent("daemon", value: "无（已锁定）")
                LabeledContent("硬件", value: "Apple Silicon only")
                LabeledContent("分发", value: "直接下载 + 公证")
            }
        }
        .formStyle(.grouped)
        .frame(width: 420, height: 280)
        .padding()
    }
}
