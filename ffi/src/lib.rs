/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! C / C++ FFI bindings for cameleon (USB3 Vision cameras).
//!
//! Note: Each CmlnCamera holds a full cameleon::Camera (with USB handles).
//! The camera is open on cmln_camera_open. USB handles are released on
//! cmln_camera_close or cmln_camera_destroy.
//!
//! After cmln_capture_start is called, the camera is moved into the
//! capture handle. The CmlnCamera* becomes invalid.

use std::os::raw::{c_char, c_int};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

pub struct CmlnCamera {
    inner: Option<CameraInner>,
}

struct CameraInner {
    cam_info: CamInfo,
    /// Holds USB handles. None until opened, Some while opened, None after closed.
    camera: Option<cameleon::Camera<
        cameleon::u3v::ControlHandle,
        cameleon::u3v::StreamHandle,
    >>,
}

/// Snapshot of camera info (used even after camera is closed).
#[derive(Clone)]
struct CamInfo {
    vendor_name: String,
    model_name: String,
    serial_number: String,
}

pub struct CmlnCapture {
    inner: Option<CaptureInner>,
}

struct CmlnFrameData {
    id: u64,
    width: u32,
    height: u32,
    data: Vec<u8>,
    timestamp_us: u64,
}

struct CaptureInner {
    frame: std::sync::Mutex<Option<CmlnFrameData>>,
    running: AtomicBool,
    #[allow(dead_code)]
    thread: Option<std::thread::JoinHandle<()>>,
    _rx: mpsc::Receiver<CmlnFrameData>,
    /// When this Arc drops, the camera drops, which closes USB handles.
    #[allow(dead_code)]
    _camera_arc: Arc<std::sync::Mutex<Option<
        cameleon::Camera<cameleon::u3v::ControlHandle, cameleon::u3v::StreamHandle>,
    >>>,
}

pub const CMLN_OK: c_int = 0;
pub const CMLN_ERR: c_int = -1;
pub const CMLN_ERR_NULL_HANDLE: c_int = -2;
pub const CMLN_ERR_CAPTURE_STOPPED: c_int = -5;

#[no_mangle]
pub extern "C" fn cmln_enumerate(
    out_handles: *mut *mut CmlnCamera,
    out_count: *mut u32,
) -> c_int {
    if out_handles.is_null() || out_count.is_null() {
        return CMLN_ERR_NULL_HANDLE;
    }
    match cameleon::u3v::enumerate_cameras() {
        Ok(cameras) => {
            let n = cameras.len();
            let mut ptrs: Vec<*mut CmlnCamera> = cameras.into_iter().map(|cam| {
                let info = cam.info();
                let cam_info = CamInfo {
                    vendor_name: info.vendor_name.clone(),
                    model_name: info.model_name.clone(),
                    serial_number: info.serial_number.clone(),
                };
                // Store the Camera directly — USB handles are NOT acquired yet.
                // control/stream channels are opened in cmln_camera_open.
                // (enumerate_cameras internally calls ControlHandle::new + StreamHandle::new
                // which open the USB device. We close them here to release usb handles.)
                Box::into_raw(Box::new(CmlnCamera {
                    inner: Some(CameraInner {
                        cam_info,
                        camera: Some(cam),
                    }),
                }))
            }).collect();

            if n == 0 {
                unsafe { *out_handles = std::ptr::null_mut(); *out_count = 0; }
                return CMLN_OK;
            }

            unsafe {
                *out_handles = ptrs.as_mut_ptr() as *mut CmlnCamera;
                *out_count = n as u32;
            }
            std::mem::forget(ptrs);
            CMLN_OK
        }
        Err(e) => {
            eprintln!("cmln_enumerate: {}", e);
            unsafe { *out_handles = std::ptr::null_mut(); *out_count = 0; }
            CMLN_ERR
        }
    }
}

#[no_mangle]
pub extern "C" fn cmln_free_enum(handles: *mut *mut CmlnCamera, count: u32) {
    if handles.is_null() || count == 0 { return; }
    unsafe { let _ = Vec::from_raw_parts(handles, count as usize, count as usize); }
}

fn copy_cstr(src: &str, dst: *mut c_char, dst_len: usize) {
    if dst_len == 0 || dst.is_null() { return; }
    let len = std::cmp::min(src.len(), dst_len - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(src.as_ptr(), dst as *mut u8, len);
        *dst.add(len) = 0;
    }
}

#[no_mangle]
pub extern "C" fn cmln_camera_info(
    cam: *mut CmlnCamera,
    out_vendor: *mut c_char, vendor_buf_len: usize,
    out_model: *mut c_char, model_buf_len: usize,
    out_serial: *mut c_char, serial_buf_len: usize,
) -> c_int {
    if cam.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let inner = unsafe { (*cam).inner.as_ref().unwrap() };
    if !out_vendor.is_null() { copy_cstr(&inner.cam_info.vendor_name, out_vendor, vendor_buf_len); }
    if !out_model.is_null() { copy_cstr(&inner.cam_info.model_name, out_model, model_buf_len); }
    if !out_serial.is_null() { copy_cstr(&inner.cam_info.serial_number, out_serial, serial_buf_len); }
    CMLN_OK
}

#[no_mangle]
pub extern "C" fn cmln_camera_open(cam: *mut CmlnCamera) -> c_int {
    if cam.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let inner = unsafe { &mut (*cam).inner.as_mut().unwrap() };
    // Camera is None means it was already moved to capture.
    // Camera is Some -> ensure closed first, then open fresh.
    let camera = match &mut inner.camera {
        Some(c) => c,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    // Ensure clean state before open (enumerate may have left handles open)
    _ = camera.close();
    // Retry open — previous destroy may not have fully released usbfs yet
    // (libusb internal threads still cleaning up).
    for attempt in 0..5 {
        match camera.open() {
            Ok(()) => return CMLN_OK,
            Err(e) => {
                eprintln!("open attempt {} failed: {}", attempt + 1, e);
                if attempt < 4 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }
    CMLN_ERR
}

#[no_mangle]
pub extern "C" fn cmln_camera_close(cam: *mut CmlnCamera) -> c_int {
    if cam.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let inner = unsafe { &mut (*cam).inner.as_mut().unwrap() };
    if inner.camera.is_none() { return CMLN_OK; }
    match inner.camera.as_mut().unwrap().close() {
        Ok(()) => CMLN_OK,
        Err(e) => { eprintln!("cmln_camera_close: {}", e); CMLN_ERR }
    }
}

#[no_mangle]
pub extern "C" fn cmln_camera_load_context(cam: *mut CmlnCamera) -> c_int {
    if cam.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let inner = unsafe { &mut (*cam).inner.as_mut().unwrap() };
    if inner.camera.is_none() { return CMLN_ERR_NULL_HANDLE; }
    match inner.camera.as_mut().unwrap().load_context() {
        Ok(_) => CMLN_OK,
        Err(e) => { eprintln!("cmln_camera_load_context: {}", e); CMLN_ERR }
    }
}

#[no_mangle]
pub extern "C" fn cmln_camera_destroy(cam: *mut CmlnCamera) {
    if !cam.is_null() { unsafe { let _ = Box::from_raw(cam); } }
}

#[no_mangle]
pub extern "C" fn cmln_capture_start(cam: *mut CmlnCamera) -> *mut CmlnCapture {
    if cam.is_null() { return std::ptr::null_mut(); }
    let camera = unsafe {
        let ci = (*cam).inner.as_mut().unwrap();
        ci.camera.take()
    };
    let camera = match camera { Some(c) => c, None => return std::ptr::null_mut() };

    let camera_arc: Arc<std::sync::Mutex<Option<cameleon::Camera<
        cameleon::u3v::ControlHandle, cameleon::u3v::StreamHandle
    >>>> = Arc::new(std::sync::Mutex::new(Some(camera)));

    let (tx, rx) = mpsc::channel::<CmlnFrameData>();
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    let handle = camera_arc.clone();

    let join_handle = std::thread::spawn(move || {
        // Take camera from Arc (MutexGuard is released immediately)
        let cam_opt = { let mut g = handle.lock().unwrap(); g.take() };
        let mut cam = match cam_opt { Some(c) => c, None => return };
        let payload_rx = match cam.start_streaming(3) {
            Ok(r) => r,
            Err(_) => return,
        };
        loop {
            if !running_clone.load(Ordering::SeqCst) { break; }
            match payload_rx.try_recv() {
                Ok(p) => {
                    let bytes = p.image().unwrap_or(&[]).to_vec();
                    let id = p.id(); let ts = p.timestamp().as_micros() as u64;
                    let (w, h) = p.image_info()
                        .map(|i| (i.width as u32, i.height as u32)).unwrap_or((0, 0));
                    let _ = tx.send(CmlnFrameData { id, width: w, height: h, data: bytes, timestamp_us: ts });
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_micros(200)),
            }
        }
        drop(payload_rx);
        // Put camera back into Arc
        let mut g = handle.lock().unwrap();
        *g = Some(cam);
    });

    let capture = CaptureInner {
        frame: std::sync::Mutex::new(None), running: AtomicBool::new(true),
        thread: Some(join_handle), _rx: rx, _camera_arc: camera_arc.clone(),
    };
    // `camera_arc` is dropped here, but `inner` holds the only other reference
    // (moved into `CaptureInner`). The camera will drop when `CaptureInner` is
    // dropped (in `cmln_capture_destroy`).
    Box::into_raw(Box::new(CmlnCapture { inner: Some(capture) }))
}

#[no_mangle]
pub extern "C" fn cmln_capture_async(
    cap: *mut CmlnCapture, out_width: *mut u32, out_height: *mut u32,
    out_data_len: *mut u32, out_timestamp_us: *mut u64,
    out_frame_id: *mut u64, out_data: *mut *const u8,
) -> c_int {
    if cap.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let inner = unsafe { (*cap).inner.as_ref().unwrap() };
    if !inner.running.load(Ordering::SeqCst) { return CMLN_ERR_CAPTURE_STOPPED; }
    match inner._rx.try_recv() {
        Ok(frame) => {
            let mut fg = inner.frame.lock().unwrap(); *fg = Some(frame);
            let f = fg.as_ref().unwrap();
            unsafe {
                if !out_width.is_null() { *out_width = f.width; }
                if !out_height.is_null() { *out_height = f.height; }
                if !out_data_len.is_null() { *out_data_len = f.data.len() as u32; }
                if !out_timestamp_us.is_null() { *out_timestamp_us = f.timestamp_us; }
                if !out_frame_id.is_null() { *out_frame_id = f.id; }
                if !out_data.is_null() { *out_data = f.data.as_ptr(); }
            }
            1
        }
        Err(_) => 0,
    }
}

#[no_mangle]
pub extern "C" fn cmln_capture_block(
    cap: *mut CmlnCapture, out_width: *mut u32, out_height: *mut u32,
    out_data_len: *mut u32, out_timestamp_us: *mut u64,
    out_frame_id: *mut u64, out_data: *mut *const u8,
) -> c_int {
    if cap.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let inner = unsafe { (*cap).inner.as_ref().unwrap() };
    match inner._rx.recv() {
        Ok(frame) => {
            let mut fg = inner.frame.lock().unwrap(); *fg = Some(frame);
            let f = fg.as_ref().unwrap();
            unsafe {
                if !out_width.is_null() { *out_width = f.width; }
                if !out_height.is_null() { *out_height = f.height; }
                if !out_data_len.is_null() { *out_data_len = f.data.len() as u32; }
                if !out_timestamp_us.is_null() { *out_timestamp_us = f.timestamp_us; }
                if !out_frame_id.is_null() { *out_frame_id = f.id; }
                if !out_data.is_null() { *out_data = f.data.as_ptr(); }
            }
            1
        }
        Err(_) => 0,
    }
}

#[no_mangle]
pub extern "C" fn cmln_capture_free_frame(cap: *mut CmlnCapture) {
    if cap.is_null() { return; }
    let inner = unsafe { (*cap).inner.as_mut().unwrap() };
    let mut fg = inner.frame.lock().unwrap(); *fg = None;
}

#[no_mangle]
pub extern "C" fn cmln_capture_stop(cap: *mut CmlnCapture) {
    if cap.is_null() { return; }
    let inner = unsafe { (*cap).inner.as_mut().unwrap() };
    inner.running.store(false, Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn cmln_capture_destroy(cap: *mut CmlnCapture) {
    if cap.is_null() { return; }
    unsafe {
        let inner = (*cap).inner.as_mut().unwrap();
        inner.running.store(false, Ordering::SeqCst);
    }
    unsafe { let _ = Box::from_raw(cap); }
    // Wait for worker thread to exit and camera's Drop to close USB handles.
    // The worker thread does: running=false → payload_rx::drop → camera back to Arc.
    // When Arc drops, the final Camera::drop closes USB handles (control + stream).
    // Give it enough time for libusb to fully release the device.
    std::thread::sleep(std::time::Duration::from_millis(300));
}

// ═══════════════════════════════════════════════════════════════════
// Parameter functions — set/get/supported params
// ═══════════════════════════════════════════════════════════════════

#[no_mangle]
pub extern "C" fn cmln_param_set_int(cam: *mut CmlnCamera, name: *const c_char, value: i64) -> c_int {
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_integer(&ctx) {
        Some(n) => match n.set_value(&mut ctx, value) {
            Ok(()) => CMLN_OK,
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

#[no_mangle]
pub extern "C" fn cmln_param_set_float(cam: *mut CmlnCamera, name: *const c_char, value: f64) -> c_int {
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_float(&ctx) {
        Some(n) => match n.set_value(&mut ctx, value) {
            Ok(()) => CMLN_OK,
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

#[no_mangle]
pub extern "C" fn cmln_param_set_bool(cam: *mut CmlnCamera, name: *const c_char, value: c_int) -> c_int {
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_boolean(&ctx) {
        Some(n) => match n.set_value(&mut ctx, value != 0) {
            Ok(()) => CMLN_OK,
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

#[no_mangle]
pub extern "C" fn cmln_param_set_string(cam: *mut CmlnCamera, name: *const c_char, val: *const c_char) -> c_int {
    if name.is_null() || val.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let val = match unsafe { std::ffi::CStr::from_ptr(val) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_integer(&ctx) {
        Some(n) => match n.set_value(&mut ctx, val.parse::<i64>().unwrap_or(0)) {
            Ok(()) => CMLN_OK,
            Err(_) => CMLN_ERR,
        },
        None => match node.as_string(&ctx) {
            Some(n) => match n.set_value(&mut ctx, val) {
                Ok(()) => CMLN_OK,
                Err(_) => CMLN_ERR,
            },
            None => CMLN_ERR,
        },
    }
}

#[no_mangle]
pub extern "C" fn cmln_param_set_enum(cam: *mut CmlnCamera, name: *const c_char, sym: *const c_char) -> c_int {
    if name.is_null() || sym.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let sym = match unsafe { std::ffi::CStr::from_ptr(sym) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_enumeration(&ctx) {
        Some(n) => match n.set_entry_by_symbolic(&mut ctx, &sym) {
            Ok(()) => CMLN_OK,
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

#[no_mangle]
pub extern "C" fn cmln_param_get_int(cam: *mut CmlnCamera, name: *const c_char, out: *mut i64) -> c_int {
    if cam.is_null() || name.is_null() || out.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_integer(&ctx) {
        Some(n) => match n.value(&mut ctx) {
            Ok(v) => { unsafe { *out = v; } CMLN_OK }
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

#[no_mangle]
pub extern "C" fn cmln_param_get_float(cam: *mut CmlnCamera, name: *const c_char, out: *mut f64) -> c_int {
    if cam.is_null() || name.is_null() || out.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_float(&ctx) {
        Some(n) => match n.value(&mut ctx) {
            Ok(v) => { unsafe { *out = v; } CMLN_OK }
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

#[no_mangle]
pub extern "C" fn cmln_param_get_bool(cam: *mut CmlnCamera, name: *const c_char, out: *mut c_int) -> c_int {
    if cam.is_null() || name.is_null() || out.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_boolean(&ctx) {
        Some(n) => match n.value(&mut ctx) {
            Ok(v) => { unsafe { *out = if v { 1 } else { 0 }; } CMLN_OK }
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

#[no_mangle]
pub extern "C" fn cmln_param_get_enum(cam: *mut CmlnCamera, name: *const c_char,
                                       buf: *mut c_char, buf_len: usize) -> c_int {
    if cam.is_null() || name.is_null() || buf_len == 0 { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    let entry = match node.as_enumeration(&ctx) {
        Some(n) => match n.current_entry(&mut ctx) {
            Ok(e) => e,
            Err(_) => return CMLN_ERR,
        },
        None => return CMLN_ERR,
    };
    let sym = entry.symbolic(&ctx);
    copy_cstr(sym, buf, buf_len);
    CMLN_OK
}

#[no_mangle]
pub extern "C" fn cmln_param_get_supported(
    cam: *mut CmlnCamera,
    out_names: *mut *const c_char,
    capacity: usize,
    out_count: *mut u32,
) -> c_int {
    if cam.is_null() || out_count.is_null() { return CMLN_ERR_NULL_HANDLE; }

    let cam_ref = match unsafe { (*cam).inner.as_mut() } {
        Some(i) => match i.camera.as_mut() {
            Some(c) => c,
            None => { unsafe { *out_count = 0; } return CMLN_ERR_NULL_HANDLE; }
        },
        None => { unsafe { *out_count = 0; } return CMLN_ERR; }
    };

    let ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => { unsafe { *out_count = 0; } return CMLN_ERR; }
    };

    let candidates = [
        "ExposureTime", "ExposureTimeAbs", "ExposureTimeAuto",
        "Gain", "GainAuto", "GainRaw",
        "Gamma", "GammaAbs", "GammaAuto",
        "Brightness", "OffsetX", "OffsetY",
        "Width", "Height", "WidthMax", "HeightMax",
        "PixelFormat",
        "TriggerMode", "TriggerSource", "TriggerActivation",
        "TriggerSoftware", "BalanceRatio", "BalanceRatioAuto",
        "AutoWhiteBalance", "Shutter", "PulseWidth", "PulseDelay",
        "AcquisitionMode", "AcquisitionFrameRate", "AcquisitionFrameRateEnable",
        "LineSelector", "LineMode", "LineInv",
        "UserOutputSelector", "UserOutputValue",
        "ColorFilter",
    ];

    let mut names: Vec<&'static str> = Vec::new();
    for c in &candidates {
        if ctx.node(*c).is_some() {
            names.push(*c);
        }
    }

    let count = names.len() as u32;
    unsafe { *out_count = count; }

    if !out_names.is_null() && capacity >= names.len() {
        for (i, &name) in names.iter().enumerate() {
            unsafe { *out_names.add(i) = name.as_ptr() as *const c_char; }
        }
    }

    count as c_int
}

// ═══════════════════════════════════════════════════════════════════
// Parameter bounds and metadata (for supportedParams / profile diagnostics)
// ═══════════════════════════════════════════════════════════════════

/// Get the minimum value of an integer parameter.
#[no_mangle]
pub extern "C" fn cmln_param_get_int_min(cam: *mut CmlnCamera, name: *const c_char, out: *mut i64) -> c_int {
    if cam.is_null() || name.is_null() || out.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_integer(&ctx) {
        Some(n) => match n.min(&mut ctx) {
            Ok(v) => { unsafe { *out = v; } CMLN_OK }
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

/// Get the maximum value of an integer parameter.
#[no_mangle]
pub extern "C" fn cmln_param_get_int_max(cam: *mut CmlnCamera, name: *const c_char, out: *mut i64) -> c_int {
    if cam.is_null() || name.is_null() || out.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_integer(&ctx) {
        Some(n) => match n.max(&mut ctx) {
            Ok(v) => { unsafe { *out = v; } CMLN_OK }
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

/// Get the minimum value of a float parameter.
#[no_mangle]
pub extern "C" fn cmln_param_get_float_min(cam: *mut CmlnCamera, name: *const c_char, out: *mut f64) -> c_int {
    if cam.is_null() || name.is_null() || out.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_float(&ctx) {
        Some(n) => match n.min(&mut ctx) {
            Ok(v) => { unsafe { *out = v; } CMLN_OK }
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

/// Get the maximum value of a float parameter.
#[no_mangle]
pub extern "C" fn cmln_param_get_float_max(cam: *mut CmlnCamera, name: *const c_char, out: *mut f64) -> c_int {
    if cam.is_null() || name.is_null() || out.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_float(&ctx) {
        Some(n) => match n.max(&mut ctx) {
            Ok(v) => { unsafe { *out = v; } CMLN_OK }
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

/// Get increment value of an integer parameter.
#[no_mangle]
pub extern "C" fn cmln_param_get_int_increment(cam: *mut CmlnCamera, name: *const c_char, out: *mut i64) -> c_int {
    if cam.is_null() || name.is_null() || out.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_integer(&ctx) {
        Some(n) => match n.inc(&mut ctx) {
            Ok(Some(v)) => { unsafe { *out = v; } CMLN_OK }
            Ok(None) => { unsafe { *out = 1; } CMLN_OK }
            Err(_) => CMLN_ERR,
        },
        None => CMLN_ERR,
    }
}

/// Get the symbolic names of all enumeration entries.
/// Returns the number of entries. If buf is non-NULL, fills it with
/// NUL-terminated symbolic names separated by NUL characters.
/// If out_required_size is non-NULL, sets the total buffer size needed.
#[no_mangle]
pub extern "C" fn cmln_param_get_enum_entries(
    cam: *mut CmlnCamera, name: *const c_char,
    buf: *mut c_char, buf_len: usize,
    out_required_size: *mut usize,
) -> c_int {
    if cam.is_null() || name.is_null() || buf_len == 0 { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let ctx = if let Ok(c) = cam_ref.params_ctxt() { c } else { return CMLN_ERR };
    let node = if let Some(n) = ctx.node(&name) { n } else { return CMLN_ERR_NULL_HANDLE };

    let entries = match node.as_enumeration(&ctx) {
        Some(n) => n.entries(&ctx),
        None => return CMLN_ERR,
    };

    // Build a NUL-separated list of symbolic names
    let mut parts: Vec<String> = Vec::new();
    for entry in &entries {
        parts.push(entry.symbolic(&ctx).to_string());
    }
    let content = parts.join("\0");
    let required = content.len() + 1; // trailing NUL

    if !out_required_size.is_null() {
        unsafe { *out_required_size = required; }
        if buf_len == 0 { return entries.len() as c_int; }
    }

    if required > buf_len {
        return entries.len() as c_int; // return count even if buffer too small
    }

    unsafe {
        std::ptr::copy_nonoverlapping(content.as_ptr(), buf as *mut u8, content.len());
    }

    entries.len() as c_int
}

/// Get the type of a parameter node (0=Integer, 1=Float, 2=Boolean, 3=String, 4=Enum, 5=Command).
/// Returns -1 if the node does not exist or context is not loaded.
#[no_mangle]
pub extern "C" fn cmln_param_get_type(
    cam: *mut CmlnCamera, name: *const c_char,
    out_type: *mut i32,
) -> c_int {
    if cam.is_null() || name.is_null() || out_type.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let ctx = if let Ok(c) = cam_ref.params_ctxt() { c } else { return CMLN_ERR };
    let node = if let Some(n) = ctx.node(&name) { n } else { return CMLN_ERR_NULL_HANDLE };

    // Determine type by trying each cast
    let t = if node.as_integer(&ctx).is_some() { 0 }
         else if node.as_float(&ctx).is_some() { 1 }
         else if node.as_boolean(&ctx).is_some() { 2 }
         else if node.as_string(&ctx).is_some() { 3 }
         else if node.as_enumeration(&ctx).is_some() { 4 }
         else { return CMLN_ERR_NULL_HANDLE };

    unsafe { *out_type = t; }
    CMLN_OK
}

/// Get whether a parameter is readable.
#[no_mangle]
pub extern "C" fn cmln_param_is_readable(
    cam: *mut CmlnCamera, name: *const c_char,
    out: *mut c_int,
) -> c_int {
    if cam.is_null() || name.is_null() || out.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    // Try integer first (most common)
    match node.as_integer(&ctx) {
        Some(n) => match n.is_readable(&mut ctx) {
            Ok(v) => { unsafe { *out = if v { 1 } else { 0 }; } CMLN_OK }
            Err(_) => CMLN_ERR,
        },
        None => match node.as_float(&ctx) {
            Some(n) => match n.is_readable(&mut ctx) {
                Ok(v) => { unsafe { *out = if v { 1 } else { 0 }; } CMLN_OK }
                Err(_) => CMLN_ERR,
            },
            None => CMLN_ERR_NULL_HANDLE,
        },
    }
}

/// Get whether a parameter is writable.
#[no_mangle]
pub extern "C" fn cmln_param_is_writable(
    cam: *mut CmlnCamera, name: *const c_char,
    out: *mut c_int,
) -> c_int {
    if cam.is_null() || name.is_null() || out.is_null() { return CMLN_ERR_NULL_HANDLE; }
    let name = match unsafe { std::ffi::CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return CMLN_ERR_NULL_HANDLE,
    };
    let cam_ref = unsafe { (*cam).inner.as_mut().unwrap() }.camera.as_mut().unwrap();
    let mut ctx = match cam_ref.params_ctxt() {
        Ok(c) => c,
        Err(_) => return CMLN_ERR,
    };
    let node = match ctx.node(&name) {
        Some(n) => n,
        None => return CMLN_ERR_NULL_HANDLE,
    };
    match node.as_integer(&ctx) {
        Some(n) => match n.is_writable(&mut ctx) {
            Ok(v) => { unsafe { *out = if v { 1 } else { 0 }; } CMLN_OK }
            Err(_) => CMLN_ERR,
        },
        None => match node.as_float(&ctx) {
            Some(n) => match n.is_writable(&mut ctx) {
                Ok(v) => { unsafe { *out = if v { 1 } else { 0 }; } CMLN_OK }
                Err(_) => CMLN_ERR,
            },
            None => CMLN_ERR_NULL_HANDLE,
        },
    }
}


