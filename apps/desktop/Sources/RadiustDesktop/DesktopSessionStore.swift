import Foundation
import Combine

/// Owns the in-process FFI session for the lifetime of the SwiftUI app.
///
/// Dropping / closing this store calls `radiust_session_close` — Engine stops
/// with the UI. No background daemon is started.
@MainActor
final class DesktopSessionStore: ObservableObject {
    @Published private(set) var snapshotJSON: String = "{}"
    @Published private(set) var allowNetwork: Bool = true
    @Published private(set) var cacheDir: String = ""
    @Published private(set) var defaultSourceId: String = "rdcap"
    @Published private(set) var statusMessage: String =
        "脚手架：同进程会话尚未连接真实观测帧。"
    @Published var locationMode: LocationMode = .country
    @Published var selectedCountry: String = "jp"
    @Published var selectedStation: String = ""
    @Published var networkDisclosureAcknowledged: Bool = false
    @Published private(set) var lastPreview: RadarPreview?

    private var bridge: RadiustFFIBridge?

    struct RadarPreview: Equatable {
        let sourceId: String
        let product: String
        let station: String?
        let validTime: String
        let pngPath: String
    }

    func openIfNeeded() {
        guard bridge == nil else { return }
        do {
            let opened = try RadiustFFIBridge.open(cacheDirOverride: nil)
            bridge = opened
            refreshFromBridge()
            statusMessage =
                "已打开同进程会话（无 daemon）。默认地区 jp → \(defaultSourceId)；缓存与 CLI 共用。"
        } catch {
            statusMessage = "无法打开 radiust-ffi 会话：\(error.localizedDescription)"
        }
    }

    func close() {
        bridge?.close()
        bridge = nil
        statusMessage = "会话已关闭；引擎随进程结束（无常驻后台）。"
    }

    func setAllowNetwork(_ enabled: Bool) {
        guard let bridge else { return }
        do {
            try bridge.setAllowNetwork(enabled)
            allowNetwork = enabled
            refreshFromBridge()
        } catch {
            statusMessage = "更新联网开关失败：\(error.localizedDescription)"
        }
    }

    func applyLocationSelection() {
        guard let bridge else { return }
        do {
            try bridge.setLocationMode(locationMode.rawValue)
            let station = selectedStation.trimmingCharacters(in: .whitespacesAndNewlines)
            try bridge.setManualSelection(
                country: selectedCountry,
                station: station.isEmpty ? nil : station
            )
            refreshFromBridge()
            statusMessage = "位置选择已写入会话（系统定位／国家／站名钩子；观测拉取后续接线）。"
        } catch {
            statusMessage = "更新位置失败：\(error.localizedDescription)"
        }
    }

    func requestSystemLocationPermission() {
        // Hook for CoreLocation; full CLLocationManager wiring lands with real map UX.
        locationMode = .system
        applyLocationSelection()
        statusMessage =
            "已切换到系统定位模式。请在系统设置中允许定位；亦可改用国家／站名选择。"
    }

    func fetchLatestRadar() {
        guard let bridge else {
            statusMessage = "会话未打开，无法拉取。"
            return
        }
        statusMessage = "正在通过同进程 Engine 拉取最新观测…"
        do {
            let json = try bridge.fetchLatestPreviewJSON()
            refreshFromBridge()
            if let parsed = Self.parsePreview(json) {
                lastPreview = parsed
                statusMessage =
                    "已显示 \(parsed.sourceId)／\(parsed.station ?? "?") @ \(parsed.validTime)"
            } else {
                statusMessage = "已拉取，但预览 JSON 解析失败：\(json)"
            }
        } catch {
            statusMessage = "拉取失败：\(error.localizedDescription)"
        }
    }

    private static func parsePreview(_ json: String) -> RadarPreview? {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        guard let sourceId = obj["source_id"] as? String,
              let product = obj["product"] as? String,
              let validTime = obj["valid_time"] as? String,
              let pngPath = obj["png_path"] as? String
        else { return nil }
        return RadarPreview(
            sourceId: sourceId,
            product: product,
            station: obj["station"] as? String,
            validTime: validTime,
            pngPath: pngPath
        )
    }

    private func refreshFromBridge() {
        guard let bridge else { return }
        do {
            snapshotJSON = try bridge.snapshotJSON()
            allowNetwork = try bridge.allowNetwork()
            cacheDir = try bridge.cacheDir()
            defaultSourceId = try bridge.defaultSourceId()
        } catch {
            statusMessage = "读取会话快照失败：\(error.localizedDescription)"
        }
    }
}

enum LocationMode: String, CaseIterable, Identifiable {
    case system
    case country
    case station

    var id: String { rawValue }

    var title: String {
        switch self {
        case .system: return "系统定位"
        case .country: return "按国家"
        case .station: return "按站名"
        }
    }
}
