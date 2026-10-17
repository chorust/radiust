import AppKit
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
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("打开即看")
                    .font(.largeTitle.weight(.semibold))
                Spacer()
                Button("拉取最新雷达") {
                    session.fetchLatestRadar()
                }
                .disabled(!session.allowNetwork)
            }
            Text(session.statusMessage)
                .foregroundStyle(.secondary)
            if let preview = session.lastPreview {
                GroupBox("观测") {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("来源 \(preview.sourceId) · 产品 \(preview.product)")
                        Text("站点 \(preview.station ?? "—") · 观测时间 \(preview.validTime)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        if let image = NSImage(contentsOfFile: preview.pngPath) {
                            Image(nsImage: image)
                                .resizable()
                                .scaledToFit()
                                .frame(maxHeight: 420)
                                .background(Color.black)
                                .clipShape(RoundedRectangle(cornerRadius: 8))
                        } else {
                            Text("已拉取但无法打开 PNG：\(preview.pngPath)")
                                .font(.caption)
                                .foregroundStyle(.orange)
                        }
                        Text("回波稀疏时画面偏暗属正常；无底图时只有回波层。")
                            .font(.caption2)
                            .foregroundStyle(.tertiary)
                    }
                }
            } else {
                GroupBox("雷达图") {
                    Text("尚未拉取。点「拉取最新雷达」（默认 jp → rdcap／JPMAKI）。")
                        .foregroundStyle(.secondary)
                }
            }
            GroupBox("会话快照") {
                ScrollView {
                    Text(session.snapshotJSON)
                        .font(.system(.body, design: .monospaced))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .textSelection(.enabled)
                }
                .frame(minHeight: 120)
            }
            Text("退出应用后引擎随进程结束，无常驻后台。缓存与 CLI 共用。")
                .font(.caption)
                .foregroundStyle(.tertiary)
            Spacer(minLength: 0)
        }
        .padding(24)
    }
}

