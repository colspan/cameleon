# cameleon-ffi — C / C++ Bindings for cameleon

C API bindings for the [cameleon](https://github.com/cameleon-rs/cameleon) USB3 Vision camera library.

## Supported Cameras

| Camera | Resolution | Interface |
|--------|-----------|-----------|
| TOOLSTAR SVA-030c | 640x480 | USB3 Vision |

Frame payload type: `YUV422_8` (614400 bytes for 640x480)
Device class: `2bdf:0001`

## API Reference

All functions are declared in `include/cameleon_ffi.h`.

### Lifecycle Overview

1. `cmln_enumerate()` — scan for USB3 Vision cameras
2. `cmln_camera_info()` — read vendor/model/serial from a camera handle
3. `cmln_camera_open()` — establish control + stream channel
4. `cmln_camera_load_context()` — parse device's GenICam XML
5. `cmln_capture_start()` — start worker thread, take camera ownership
6. `cmln_capture_async()` / `cmln_capture_block()` — grab frames
7. `cmln_capture_free_frame()` — free the frame buffer
8. `cmln_capture_destroy()` — stop stream, release handles
9. `cmln_camera_close()` — close control channel
10. `cmln_camera_destroy()` — release camera handle
11. `cmln_free_enum()` — free the enum array

### Return Values

| Value | Meaning |
|-------|---------|
| `0` (CMLN_OK) | Success |
| `-1` (CMLN_ERR) | Generic error |
| `-2` (CMLN_ERR_NULL_HANDLE) | NULL pointer argument |
| `-3` (CMLN_ERR_ALREADY_OPENED) | Camera already opened |
| `-4` (CMLN_ERR_NOT_OPENED) | Operation requires opened camera |
| `-5` (CMLN_ERR_CAPTURE_STOPPED) | Capture already stopped |

## Examples

### Minimal usage (`examples/cmln_minimal.cpp`)

```bash
g++ -std=c++17 -Iinclude -L<cameleon-root>/target/release \
    -Wl,-rpath,<cameleon-root>/target/release \
    -o cmln_minimal cmln_minimal.cpp \
    -lcameleon_ffi -ldl -lpthread
LD_LIBRARY_PATH=<cameleon-root>/target/release ./cmln_minimal
```

### Full feature example (`examples/cmln_example.cpp`)

```bash
./cmln_example --count 20              # async mode, 20 frames
./cmln_example --count 20 --block      # blocking mode, 20 frames
```

## Build

```bash
cargo build --release -p cameleon-ffi
cargo test --release -p cameleon-ffi
```

## Linking

Static (rlib) linking:
```bash
-L<path>/target/release -lcameleon_ffi -l<bundled deps>
```

Requires `libusb` system dependency for `libusb` feature.

## Limitations

- **USB handle release after capture**: When `cmln_capture_start()` / `cmln_capture_destroy()` are called, the USB device handles are released asynchronously (via worker thread join). The next `cmln_camera_open()` may fail on the first attempt with `CMLN_ERR`. Retry 1-2 times or use a short `sleep` between destroy and next open.
- **Single camera per process**: Only one process can have the USB camera open at a time. Enumerate does not acquire any handles, but open does.
- **YUV data is raw bytes**: No image conversion is performed. Clients must handle YUV422_8 decoding themselves.
