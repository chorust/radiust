import SwiftUI

/// Temporary MVP shell — brand polish is explicitly non-blocking.
struct RootView: View {
    @EnvironmentObject private var session: DesktopSessionStore

    var body: some View {
        NavigationSplitView {
            sidebar
        } detail: {
            detail
        }
        .frame(minWidth: 880, minHeight: 560)
    }

    private var sidebar: some View {
        List {
            Section("MVP") {
                LabeledContent("地区", value: "日本 (jp)")
                LabeledContent("默认来源", value: session.defaultSourceId)
                LabeledContent("进程", value: "同进程 / 无 daemon")
            }
            Section("位置") {
                Picker("模式", selection: $session.locationMode) {
                    ForEach(LocationMode.allCases) { mode in
                        Text(mode.title).tag(mode)
                    }
                }
                TextField("国家代码", text: $session.selectedCountry)
                    .textFieldStyle(.roundedBorder)
                TextField("站名 / 站 ID", text: $session.selectedStation)
                    .textFieldStyle(.roundedBorder)
                Button("应用位置选择") {
                    session.applyLocationSelection()
                }
                Button("请求系统定位权限") {
                    session.requestSystemLocationPermission()
                }
            }
            Section("网络") {
                Toggle("允许出站获取观测", isOn: Binding(
                    get: { session.allowNetwork },
                    set: { session.setAllowNetwork($0) }
                ))
                Text(
                    "安装后默认允许联网。可随时关闭；关闭后仅使用本地缓存语义。"
                )
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .navigationTitle("Radiust")
        .listStyle(.sidebar)
    }

    private var detail: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("打开即看（脚手架）")
                .font(.largeTitle.weight(.semibold))
            Text(session.statusMessage)
                .foregroundStyle(.secondary)
            GroupBox("会话快照") {
                ScrollView {
                    Text(session.snapshotJSON)
                        .font(.system(.body, design: .monospaced))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .textSelection(.enabled)
                }
                .frame(minHeight: 180)
            }
            GroupBox("缓存") {
                Text(session.cacheDir.isEmpty ? "（未打开会话）" : session.cacheDir)
                    .font(.system(.body, design: .monospaced))
                    .textSelection(.enabled)
                Text("与 CLI 共用 radiust-rust 缓存根；Desktop 不另起拉取权威。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Text("退出应用后引擎随进程结束，无常驻后台。")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
        .padding(24)
    }
}
