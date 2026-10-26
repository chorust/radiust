# Radiust Desktop (macOS / SwiftUI)

Scaffold for the Radiust Mac client. **Not product-complete; not ROADMAP-accepted.**

## Locked product decisions

| Item | Value |
| --- | --- |
| Shell | SwiftUI (not Tauri) |
| Engine | In-process `radiust-core` via `radiust-ffi` |
| Daemon | **None** — quit app → engine stops |
| Arch | Apple Silicon (`arm64`) only |
| MVP region | Japan (`jp` → `rdcap`) |
| Network | Outbound allowed by default + UI disclosure + disable |
| Cache | Shared with CLI: `~/.cache/radiust-rust` |
| Distribution | Direct download + notarization (not MAS) |

## Layout

```text
apps/desktop/
  Package.swift                 # SwiftPM package (macOS arm64)
  Sources/RadiustDesktop/       # SwiftUI app + FFI wrapper
  Resources/Info.plist          # Location + network usage strings
crates/radiust-ffi/             # Same-process C ABI (no daemon)
```

## Build on a Mac (Apple Silicon)

1. Build the Rust bridge (staticlib preferred for a single .app process):

   ```bash
   cargo build -p radiust-ffi --release
   ```

2. Open or build the Swift package on macOS 14+ / Xcode 15+:

   ```bash
   cd apps/desktop
   swift build -c release --arch arm64
   ```

3. Packaging / notarization scripts are intentionally deferred; temporary UI is OK.

Linux CI cannot compile SwiftUI; Rust `radiust-ffi` tests cover the session contract offline.

### Cloud review (this environment)

1. In-process session demo (no daemon):

   ```bash
   CXX=g++ cargo run -p radiust-ffi --example desktop_session_demo
   ```

2. Temporary UI layout preview (browser mock, **not** the macOS `.app`):

   ```bash
   cd apps/desktop/preview && python3 -m http.server 8765 --bind 127.0.0.1
   # open http://127.0.0.1:8765/
   ```

## Process model

```text
Radiust.app  ──(in-process)──►  radiust_ffi  ──►  radiust-core Engine
     ▲ quit / close session          │
     └──────── everything stops ─────┘
```

There is no `radiust-daemon`, no socket protocol, and no login item for v1.
