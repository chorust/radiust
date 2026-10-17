import Foundation
import RadiustFFI

enum RadiustFFIError: Error, LocalizedError {
    case openFailed(Int32)
    case callFailed(String, Int32)
    case unavailable

    var errorDescription: String? {
        switch self {
        case .openFailed(let code):
            return "radiust_session_open failed (\(code))"
        case .callFailed(let name, let code):
            return "\(name) failed (\(code))"
        case .unavailable:
            return "radiust-ffi library unavailable in this build"
        }
    }
}

/// Thin Swift wrapper over the in-process C ABI. Not a network client.
final class RadiustFFIBridge {
    private var handle: OpaquePointer?

    private init(handle: OpaquePointer) {
        self.handle = handle
    }

    static func open(cacheDirOverride: String?) throws -> RadiustFFIBridge {
        var out: OpaquePointer?
        let code: Int32
        if let cacheDirOverride {
            code = cacheDirOverride.withCString { ptr in
                radiust_session_open(ptr, &out)
            }
        } else {
            code = radiust_session_open(nil, &out)
        }
        guard code == 0, let out else {
            throw RadiustFFIError.openFailed(code)
        }
        return RadiustFFIBridge(handle: out)
    }

    func close() {
        if let handle {
            radiust_session_close(handle)
            self.handle = nil
        }
    }

    deinit {
        close()
    }

    func snapshotJSON() throws -> String {
        try readString { out in
            radiust_session_snapshot_json(requireHandle(), out)
        }
    }

    func allowNetwork() throws -> Bool {
        let value = radiust_session_get_allow_network(requireHandle())
        guard value >= 0 else {
            throw RadiustFFIError.callFailed("radiust_session_get_allow_network", value)
        }
        return value != 0
    }

    func setAllowNetwork(_ enabled: Bool) throws {
        let code = radiust_session_set_allow_network(requireHandle(), enabled ? 1 : 0)
        guard code == 0 else {
            throw RadiustFFIError.callFailed("radiust_session_set_allow_network", code)
        }
    }

    func cacheDir() throws -> String {
        try readString { out in
            radiust_session_cache_dir(requireHandle(), out)
        }
    }

    func defaultSourceId() throws -> String {
        try readString { out in
            radiust_session_default_source_id(requireHandle(), out)
        }
    }

    func setLocationMode(_ mode: String) throws {
        let code = mode.withCString { ptr in
            radiust_session_set_location_mode(requireHandle(), ptr)
        }
        guard code == 0 else {
            throw RadiustFFIError.callFailed("radiust_session_set_location_mode", code)
        }
    }

    func fetchLatestPreviewJSON() throws -> String {
        try readString { out in
            radiust_session_fetch_latest_preview(requireHandle(), out)
        }
    }

    func setManualSelection(country: String?, station: String?) throws {
        let code: Int32
        if let country, let station {
            code = country.withCString { countryPtr in
                station.withCString { stationPtr in
                    radiust_session_set_manual_selection(requireHandle(), countryPtr, stationPtr)
                }
            }
        } else if let country {
            code = country.withCString { countryPtr in
                radiust_session_set_manual_selection(requireHandle(), countryPtr, nil)
            }
        } else if let station {
            code = station.withCString { stationPtr in
                radiust_session_set_manual_selection(requireHandle(), nil, stationPtr)
            }
        } else {
            code = radiust_session_set_manual_selection(requireHandle(), nil, nil)
        }
        guard code == 0 else {
            throw RadiustFFIError.callFailed("radiust_session_set_manual_selection", code)
        }
    }

    private func requireHandle() -> OpaquePointer {
        guard let handle else {
            fatalError("RadiustFFIBridge used after close")
        }
        return handle
    }

    private func readString(_ body: (UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> Int32)
        throws -> String
    {
        var out: UnsafeMutablePointer<CChar>?
        let code = body(&out)
        guard code == 0, let out else {
            throw RadiustFFIError.callFailed("readString", code)
        }
        defer { radiust_string_free(out) }
        return String(cString: out)
    }
}
