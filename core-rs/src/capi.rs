//! The C ABI of the application engine (`app.rs`), for the Qt front end. Four functions and a callback; everything else is JSON.
//! Header: `core-rs/vector_app.h`. Strings are UTF-8, NUL-terminated. The event callback runs on engine threads.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::Arc;

use crate::app::{App, Sink};

pub type VcrEventFn = extern "C" fn(user: *mut c_void, name: *const c_char, json: *const c_char);

struct User(*mut c_void);
unsafe impl Send for User {}
unsafe impl Sync for User {}

fn text(p: *const c_char) -> String { if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() } }

/// Make an engine. `data_dir` holds the saved session; `cb` gets every event as (name, json) with `user` passed back.
#[no_mangle]
pub extern "C" fn vcr_app_new(data_dir: *const c_char, cb: VcrEventFn, user: *mut c_void) -> *mut App {
    let user = User(user);
    let sink: Sink = Arc::new(move |name, payload| {
        let (n, p) = (CString::new(name).unwrap_or_default(), CString::new(payload.replace('\0', "")).unwrap_or_default());
        let u = &user;
        cb(u.0, n.as_ptr(), p.as_ptr());
    });
    Box::into_raw(Box::new(App::new(PathBuf::from(text(data_dir)), sink)))
}

/// Run a method; returns its JSON answer (`null` for most: the work happens in the background and comes back as events).
/// The result must be released with `vcr_string_free`.
#[no_mangle]
pub extern "C" fn vcr_app_call(app: *mut App, method: *const c_char, args_json: *const c_char) -> *mut c_char {
    if app.is_null() { return std::ptr::null_mut(); }
    let args: serde_json::Value = serde_json::from_str(&text(args_json)).unwrap_or(serde_json::Value::Null);
    let out = unsafe { &*app }.call(&text(method), &args).to_string();
    CString::new(out).unwrap_or_default().into_raw()
}

#[no_mangle]
pub extern "C" fn vcr_string_free(s: *mut c_char) { if !s.is_null() { drop(unsafe { CString::from_raw(s) }); } }

/// Stop the engine (it leaves the SDK's stores cleanly) and free it.
#[no_mangle]
pub extern "C" fn vcr_app_free(app: *mut App) { if !app.is_null() { drop(unsafe { Box::from_raw(app) }); } }
