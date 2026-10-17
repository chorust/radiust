//! In-process FFI surface for the Radiust macOS Desktop app.
//!
//! # Process model (locked)
//! - Session + Engine live in the same process as the SwiftUI app.
//! - There is **no** daemon binary, Unix socket, launchd agent, or menubar resident.
//! - Closing the app drops the session handle and stops all engine work.
//!
//! # Binding path
//! Hand-written C ABI today; UniFFI can generate Swift bindings over the same
//! session contract later without introducing a second process.

mod session;

pub use session::{
    DesktopDefaults, DesktopSession, LocationMode, PreviewFrame, RegionPolicy, SessionError,
    SessionSnapshot, JP_DEFAULT_SOURCE_ID, JP_DEFAULT_STATION_ID, MVP_REGION,
};

use session::DesktopSession as Session;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::ptr;

/// Opaque session handle for C / Swift callers.
pub struct RadiustSessionHandle {
    inner: Session,
}

fn catch_unwind_result<F, T>(f: F) -> Result<T, SessionError>
where
    F: FnOnce() -> Result<T, SessionError>,
{
    // Engine / reqwest are not UnwindSafe; FFI boundary still fences panics.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(_) => Err(SessionError::Internal("panic in radiust-ffi".into())),
    }
}

unsafe fn write_cstring(out: *mut *mut c_char, value: &str) -> c_int {
    if out.is_null() {
        return -1;
    }
    match CString::new(value) {
        Ok(c) => {
            unsafe {
                *out = c.into_raw();
            }
            0
        }
        Err(_) => -2,
    }
}

/// Free a string returned by this crate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_string_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    drop(unsafe { CString::from_raw(ptr) });
}

/// Library / bridge version string (not a Desktop product acceptance claim).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_ffi_version(out: *mut *mut c_char) -> c_int {
    catch_unwind_result(|| Ok(unsafe { write_cstring(out, env!("CARGO_PKG_VERSION")) }))
        .unwrap_or(-99)
}

/// Open an in-process desktop session.
///
/// Desktop defaults (locked):
/// - `allow_network = true` (UI must disclose; user may disable later)
/// - shared cache root with CLI (`~/.cache/radiust-rust` unless overridden)
/// - MVP region policy `jp` → `rdcap` Japan stations when catalog allows
///
/// Pass `cache_dir_override` as null to keep the Engine/CLI default.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_session_open(
    cache_dir_override: *const c_char,
    out: *mut *mut RadiustSessionHandle,
) -> c_int {
    let result = catch_unwind_result(|| {
        if out.is_null() {
            return Err(SessionError::InvalidArgument("null out handle".into()));
        }
        let override_path = if cache_dir_override.is_null() {
            None
        } else {
            let raw = unsafe { CStr::from_ptr(cache_dir_override) };
            Some(
                raw.to_str()
                    .map_err(|_| SessionError::InvalidArgument("cache_dir not utf-8".into()))?
                    .to_owned(),
            )
        };
        let session = Session::open_desktop_defaults(override_path)?;
        let handle = Box::new(RadiustSessionHandle { inner: session });
        unsafe {
            *out = Box::into_raw(handle);
        }
        Ok(0)
    });
    match result {
        Ok(code) => code,
        Err(_) => {
            if !out.is_null() {
                unsafe {
                    *out = ptr::null_mut();
                }
            }
            -1
        }
    }
}

/// Close a session. Safe to call with null. After this, the Engine is gone.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_session_close(handle: *mut RadiustSessionHandle) {
    if handle.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(unsafe { Box::from_raw(handle) });
    }));
}

/// JSON snapshot of session preferences / policy (no secrets, no locator URLs).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_session_snapshot_json(
    handle: *mut RadiustSessionHandle,
    out: *mut *mut c_char,
) -> c_int {
    catch_unwind_result(|| {
        if handle.is_null() {
            return Err(SessionError::InvalidArgument("null handle".into()));
        }
        let session = unsafe { &(*handle).inner };
        let json = session.snapshot_json()?;
        unsafe { write_cstring(out, &json) };
        Ok(0)
    })
    .unwrap_or(-1)
}

/// Set whether outbound network is allowed (maps to `runtime.allow_network`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_session_set_allow_network(
    handle: *mut RadiustSessionHandle,
    allow: c_int,
) -> c_int {
    catch_unwind_result(|| {
        if handle.is_null() {
            return Err(SessionError::InvalidArgument("null handle".into()));
        }
        let session = unsafe { &mut (*handle).inner };
        session.set_allow_network(allow != 0)?;
        Ok(0)
    })
    .unwrap_or(-1)
}

/// Get whether outbound network is allowed (1/0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_session_get_allow_network(
    handle: *const RadiustSessionHandle,
) -> c_int {
    if handle.is_null() {
        return -1;
    }
    let session = unsafe { &(*handle).inner };
    if session.allow_network() {
        1
    } else {
        0
    }
}

/// Copy the active cache root path into `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_session_cache_dir(
    handle: *const RadiustSessionHandle,
    out: *mut *mut c_char,
) -> c_int {
    catch_unwind_result(|| {
        if handle.is_null() {
            return Err(SessionError::InvalidArgument("null handle".into()));
        }
        let session = unsafe { &(*handle).inner };
        let path = session.cache_dir().to_string_lossy();
        unsafe { write_cstring(out, &path) };
        Ok(0)
    })
    .unwrap_or(-1)
}

/// Set location selection mode: `system`, `country`, or `station`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_session_set_location_mode(
    handle: *mut RadiustSessionHandle,
    mode: *const c_char,
) -> c_int {
    catch_unwind_result(|| {
        if handle.is_null() || mode.is_null() {
            return Err(SessionError::InvalidArgument("null argument".into()));
        }
        let raw = unsafe { CStr::from_ptr(mode) };
        let text = raw
            .to_str()
            .map_err(|_| SessionError::InvalidArgument("mode not utf-8".into()))?;
        let parsed = LocationMode::parse(text)?;
        let session = unsafe { &mut (*handle).inner };
        session.set_location_mode(parsed);
        Ok(0)
    })
    .unwrap_or(-1)
}

/// Set country / station picker values (UTF-8). Empty string clears.
/// Country codes use catalog conventions (`jp`, `JP`, `JPN` accepted for Japan MVP).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_session_set_manual_selection(
    handle: *mut RadiustSessionHandle,
    country: *const c_char,
    station: *const c_char,
) -> c_int {
    catch_unwind_result(|| {
        if handle.is_null() {
            return Err(SessionError::InvalidArgument("null handle".into()));
        }
        let country = read_optional_cstr(country)?;
        let station = read_optional_cstr(station)?;
        let session = unsafe { &mut (*handle).inner };
        session.set_manual_selection(country, station);
        Ok(0)
    })
    .unwrap_or(-1)
}

fn read_optional_cstr(ptr: *const c_char) -> Result<Option<String>, SessionError> {
    if ptr.is_null() {
        return Ok(None);
    }
    let raw = unsafe { CStr::from_ptr(ptr) };
    let text = raw
        .to_str()
        .map_err(|_| SessionError::InvalidArgument("string not utf-8".into()))?;
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text.to_owned()))
    }
}

/// Default source ID suggested for the active region policy (e.g. `rdcap` for jp).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_session_default_source_id(
    handle: *const RadiustSessionHandle,
    out: *mut *mut c_char,
) -> c_int {
    catch_unwind_result(|| {
        if handle.is_null() {
            return Err(SessionError::InvalidArgument("null handle".into()));
        }
        let session = unsafe { &(*handle).inner };
        unsafe { write_cstring(out, session.default_source_id()) };
        Ok(0)
    })
    .unwrap_or(-1)
}

/// Discover + download latest PNG for the session station (in-process Engine only).
/// Writes PreviewFrame JSON (includes `png_path`) into `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radiust_session_fetch_latest_preview(
    handle: *mut RadiustSessionHandle,
    out: *mut *mut c_char,
) -> c_int {
    catch_unwind_result(|| {
        if handle.is_null() {
            return Err(SessionError::InvalidArgument("null handle".into()));
        }
        let session = unsafe { &mut (*handle).inner };
        let preview = session.fetch_latest_preview()?;
        let json = serde_json::to_string(&preview)
            .map_err(|err| SessionError::Internal(err.to_string()))?;
        unsafe { write_cstring(out, &json) };
        Ok(0)
    })
    .unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn open_close_session_uses_shared_cache_and_network_default() {
        let dir = tempfile::tempdir().unwrap();
        let cache = CString::new(dir.path().to_string_lossy().as_ref()).unwrap();
        let mut handle: *mut RadiustSessionHandle = ptr::null_mut();
        let rc = unsafe { radiust_session_open(cache.as_ptr(), &mut handle) };
        assert_eq!(rc, 0);
        assert!(!handle.is_null());
        assert_eq!(unsafe { radiust_session_get_allow_network(handle) }, 1);

        let mut cache_out: *mut c_char = ptr::null_mut();
        assert_eq!(unsafe { radiust_session_cache_dir(handle, &mut cache_out) }, 0);
        let returned = unsafe { CStr::from_ptr(cache_out) }.to_string_lossy();
        assert_eq!(returned, dir.path().to_string_lossy());
        unsafe { radiust_string_free(cache_out) };

        let mut source_out: *mut c_char = ptr::null_mut();
        assert_eq!(
            unsafe { radiust_session_default_source_id(handle, &mut source_out) },
            0
        );
        let source = unsafe { CStr::from_ptr(source_out) }.to_str().unwrap();
        assert_eq!(source, "rdcap");
        unsafe { radiust_string_free(source_out) };

        assert_eq!(unsafe { radiust_session_set_allow_network(handle, 0) }, 0);
        assert_eq!(unsafe { radiust_session_get_allow_network(handle) }, 0);

        unsafe { radiust_session_close(handle) };
    }

    #[test]
    fn snapshot_json_mentions_jp_and_no_daemon() {
        let mut handle: *mut RadiustSessionHandle = ptr::null_mut();
        assert_eq!(unsafe { radiust_session_open(ptr::null(), &mut handle) }, 0);
        let mut out: *mut c_char = ptr::null_mut();
        assert_eq!(unsafe { radiust_session_snapshot_json(handle, &mut out) }, 0);
        let json = unsafe { CStr::from_ptr(out) }.to_str().unwrap().to_owned();
        unsafe { radiust_string_free(out) };
        unsafe { radiust_session_close(handle) };
        assert!(json.contains("\"region\":\"jp\""));
        assert!(json.contains("\"process_model\":\"in_process\""));
        assert!(json.contains("\"daemon\":false"));
    }
}
