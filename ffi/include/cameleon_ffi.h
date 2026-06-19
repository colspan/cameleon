/*
 * cameleon_ffi.h - C / C++ bindings for cameleon (USB3 Vision cameras)
 *
 * This header is auto-generated from ffi/src/lib.rs.
 *
 * Usage flow:
 *   1. cmln_enumerate()         - find cameras
 *   2. cmln_camera_open() / cmln_camera_load_context() - configure
 *   3. cmln_capture_start()     - begin streaming
 *   4. cmln_capture_async()     - try_recv (non-blocking)
 *   5. cmln_capture_block()     - recv (blocking)
 *   6. cmln_capture_free_frame()- free Rust-owned frame data
 *   7. cmln_capture_destroy()   - stop + dealloc
 */

#ifndef CAMELEON_FFI_H
#define CAMELEON_FFI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── opaque handles ─────────────────────────────────────────── */

typedef struct CmlnCamera CmlnCamera;
typedef struct CmlnCapture CmlnCapture;

/* ── return values ──────────────────────────────────────────── */

#define CMLN_OK              0
#define CMLN_ERR            -1
#define CMLN_ERR_NULL_HANDLE  -2
#define CMLN_ERR_ALREADY_CLOSED -3
#define CMLN_ERR_CAPTURE_ACTIVE -4
#define CMLN_ERR_CAPTURE_STOPPED -5

/* ── Enumerate ─────────────────────────────────────────────── */

/*
 * Enumerate all connected USB3 Vision cameras.
 *
 * On success:
 *   - *out_handles is set to an array of `count` Camera pointers
 *     (valid until cmln_free_enum() is called)
 *   - *out_count is set to the number of cameras
 *
 * Ownership: The returned array is owned by this library. Call
 * cmln_free_enum(handles, count) when done to free it. Individual
 * cameras remain alive unless cmln_camera_destroy() is called.
 */
int cmln_enumerate(CmlnCamera ***out_handles, uint32_t *out_count);

/* Free the array returned by cmln_enumerate(). */
void cmln_free_enum(CmlnCamera **handles, uint32_t count);

/* ── Camera info ────────────────────────────────────────────── */

/*
 * Retrieve vendor, model, and serial strings.
 * Each buffer should be at least CMLN_MAX_STRING_LEN bytes.
 * Strings are NUL-terminated. Pass NULL to skip a field.
 */
int cmln_camera_info(CmlnCamera *cam,
                     char *out_vendor, size_t vendor_buf_len,
                     char *out_model,   size_t model_buf_len,
                     char *out_serial,  size_t serial_buf_len);

#define CMLN_MAX_STRING_LEN 256

/* ── Camera lifecycle ───────────────────────────────────────── */

/*
 * Open the camera handle. Must be called before any other camera
 * operations.
 */
int cmln_camera_open(CmlnCamera *cam);

/*
 * Close the camera. After this the handle may not be reused.
 */
int cmln_camera_close(CmlnCamera *cam);

/*
 * Load the GenApi XML context. Required before starting streams.
 */
int cmln_camera_load_context(CmlnCamera *cam);

/*
 * Destroy a single camera handle (frees memory).
 */
void cmln_camera_destroy(CmlnCamera *cam);

/* ── Capture / Streaming ───────────────────────────────────── */

/*
 * Start the capture worker thread.
 *
 * The camera handle `cam` is consumed — it becomes invalid after
 * this call. Use the returned CmlnCapture* for all subsequent
 * operations.
 *
 * Returns NULL on failure.
 */
CmlnCapture *cmln_capture_start(CmlnCamera *cam);

/*
 * Non-blocking frame grab (try_recv).
 *
 * Returns:
 *   1  — frame ready (outputs populated)
 *   0  — timeout (no frame available, try again later)
 *  -1/-5 — error
 *
 * The `out_data` pointer is owned by Rust and is valid until the
 * next call to cmln_capture_* or cmln_capture_free_frame().
 *
 * The `out_data_len` bytes at `out_data` are the raw image bytes
 * (e.g. YUV422_8).
 */
int cmln_capture_async(CmlnCapture *cap,
                       uint32_t *out_width,
                       uint32_t *out_height,
                       uint32_t *out_data_len,
                       uint64_t *out_timestamp_us,
                       uint64_t *out_frame_id,
                       const uint8_t **out_data);

/*
 * Blocking frame grab (recv).
 *
 * Same output params as cmln_capture_async(), but blocks until
 * a frame is available or the capture is stopped.
 *
 * Returns:
 *   1  — frame ready
 *   0  — capture stopped (channel closed)
 *  -1  — error
 */
int cmln_capture_block(CmlnCapture *cap,
                       uint32_t *out_width,
                       uint32_t *out_height,
                       uint32_t *out_data_len,
                       uint64_t *out_timestamp_us,
                       uint64_t *out_frame_id,
                       const uint8_t **out_data);

/*
 * Free the most recently received frame (drops the Rust-owned buffer).
 * After this call the data pointer from the last frame is no longer valid.
 */
void cmln_capture_free_frame(CmlnCapture *cap);

/*
 * Stop the capture thread (sets running=false signal).
 */
void cmln_capture_stop(CmlnCapture *cap);

/*
  * Destroy the capture handle (stops capture if running, joins thread,
  * frees the camera handle). This is the recommended cleanup function.
  */
 void cmln_capture_destroy(CmlnCapture *cap);


 /* ── Parameter access ───────────────────────────────────────── */

 /*
  * Set and get camera parameters (names follow GenICam naming convention).
  * Must be called after cmln_camera_load_context() and before cmln_capture_start().
  *
  * Parameter names: "Gain", "ExposureTime", "TriggerMode", etc.
  */

 /* Set operations — returns CMLN_OK on success */
 int cmln_param_set_int(CmlnCamera *cam, const char *name, int64_t value);
 int cmln_param_set_float(CmlnCamera *cam, const char *name, double value);
 int cmln_param_set_bool(CmlnCamera *cam, const char *name, int value);
 int cmln_param_set_string(CmlnCamera *cam, const char *name, const char *value);
 int cmln_param_set_enum(CmlnCamera *cam, const char *name, const char *symbolic);

 /* Get operations — writes result via out pointer, returns CMLN_OK on success */
 int cmln_param_get_int(CmlnCamera *cam, const char *name, int64_t *out);
 int cmln_param_get_float(CmlnCamera *cam, const char *name, double *out);
 int cmln_param_get_bool(CmlnCamera *cam, const char *name, int *out);

 /* Get enum value as symbolic name — max CMLN_MAX_STRING_LEN bytes */
 int cmln_param_get_enum(CmlnCamera *cam, const char *name, char *buf, size_t buf_len);

 /*
  * Get list of supported parameter names.
  * Returns count of supported params.
  * If out_names != NULL and capacity >= count, fills array of C string pointers.
  * Strings are static — caller must NOT free them.
  */
 int cmln_param_get_supported(CmlnCamera *cam,
                               const char **out_names,
                               size_t capacity,
                               uint32_t *out_count);


#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* CAMELEON_FFI_H */