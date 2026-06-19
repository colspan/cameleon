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
