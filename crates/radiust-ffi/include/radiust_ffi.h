/* Copyright 2026 Radiust contributors. Apache-2.0.
 *
 * In-process C ABI for Radiust Desktop. No daemon / no Unix socket.
 * Link the same-process static or dynamic library into the SwiftUI .app.
 */

#ifndef RADIUST_FFI_H
#define RADIUST_FFI_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct RadiustSessionHandle RadiustSessionHandle;

/** Free a heap string returned by this library. */
void radiust_string_free(char *ptr);

/** Write library version into *out (caller frees with radiust_string_free). */
int radiust_ffi_version(char **out);

/**
 * Open an in-process session with Desktop defaults:
 * allow_network=true, shared CLI cache root, jp→rdcap policy.
 * cache_dir_override may be NULL to use ~/.cache/radiust-rust.
 */
int radiust_session_open(const char *cache_dir_override, RadiustSessionHandle **out);

/** Drop the session; Engine work ends with the process / this call. */
void radiust_session_close(RadiustSessionHandle *handle);

int radiust_session_snapshot_json(RadiustSessionHandle *handle, char **out);
int radiust_session_set_allow_network(RadiustSessionHandle *handle, int allow);
int radiust_session_get_allow_network(const RadiustSessionHandle *handle);
int radiust_session_cache_dir(const RadiustSessionHandle *handle, char **out);
int radiust_session_set_location_mode(RadiustSessionHandle *handle, const char *mode);
int radiust_session_set_manual_selection(RadiustSessionHandle *handle,
                                         const char *country,
                                         const char *station);
int radiust_session_default_source_id(const RadiustSessionHandle *handle, char **out);

/** Discover+download latest PNG via Engine; out = PreviewFrame JSON (png_path, times). */
int radiust_session_fetch_latest_preview(RadiustSessionHandle *handle, char **out);

#ifdef __cplusplus
}
#endif

#endif /* RADIUST_FFI_H */
