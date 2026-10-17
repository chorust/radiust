# radiust-ffi

In-process C ABI for the Radiust macOS Desktop (SwiftUI).

## Locked constraints

- **Same process only** — Session + `radiust-core` Engine live inside the `.app`.
- **No daemon** — no Unix socket service, no launchd agent, no menubar resident binary.
- **Quit stops everything** — closing the app / calling `radiust_session_close` ends engine work.
- **Shared cache** — default `~/.cache/radiust-rust` (same as CLI).
- **Desktop network default** — `allow_network=true` at open; UI must disclose and allow disable.
- **MVP region** — `jp` policy defaults to catalog source `rdcap` (Japan stations).

## Build

```bash
cargo build -p radiust-ffi --release
```

Header: [`include/radiust_ffi.h`](include/radiust_ffi.h).

## UniFFI path

This crate is the session contract. UniFFI (or `cbindgen`) can generate Swift later over the same functions; do not introduce a second process when migrating generators.
