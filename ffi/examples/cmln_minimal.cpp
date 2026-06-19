/*
 * Minimal example: enumerate → open → load_context → stream 10 frames → cleanup.
 * Build:
 *   g++ -std=c++17 -I<cameleon-root>/ffi/include \
 *       -L<cameleon-root>/target/release \
 *       -Wl,-rpath,<cameleon-root>/target/release \
 *       -o /tmp/cmln_minimal \
 *       cmln_minimal.cpp -lcameleon_ffi -ldl -lpthread
 */

#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include "cameleon_ffi.h"

int main() {
    // 1. Enumerate
    CmlnCamera **cams = nullptr;
    uint32_t count = 0;
    int rc = cmln_enumerate(&cams, &count);
    if (rc != CMLN_OK) { printf("enumerate failed\n"); exit(1); }
    if (count == 0) { printf("no cameras found\n"); cmln_free_enum(cams, count); exit(0); }

    printf("Found %u camera(s):\n", count);
    for (uint32_t i = 0; i < count; i++) {
        char vendor[128], model[128], serial[128];
        cmln_camera_info(cams[i], vendor, sizeof(vendor),
                         model, sizeof(model), serial, sizeof(serial));
        printf("  [%u] %s  model=%s  serial=%s\n", i, vendor, model, serial);
    }

    CmlnCamera *cam = cams[0];

    // 2. Open camera (includes retries on failure)
    rc = cmln_camera_open(cam);
    if (rc != CMLN_OK) { printf("open failed rc=%d\n", rc); cmln_free_enum(cams, count); exit(1); }

    // 3. Load GenApi context
    rc = cmln_camera_load_context(cam);
    if (rc != CMLN_OK) { printf("load_context failed\n"); cmln_camera_close(cam); cmln_free_enum(cams, count); exit(1); }

    // 4. Start capture worker thread
    CmlnCapture *cap = cmln_capture_start(cam);
    if (!cap) { printf("capture_start failed\n"); cmln_camera_close(cam); cmln_free_enum(cams, count); exit(1); }

    // 5. Grab frames asynchronously
    printf("Grabbing 10 frames...\n");
    for (int f = 0; f < 10; f++) {
        uint32_t w = 0, h = 0, len = 0;
        uint64_t ts = 0, fid = 0;
        const uint8_t *data = nullptr;

        int attempts = 0;
        while (attempts < 1000) { // up to ~1s
            rc = cmln_capture_async(cap, &w, &h, &len, &ts, &fid, &data);
            if (rc == 1) break;
            struct timespec tv = {0, 1000000}; // 1ms
            nanosleep(&tv, nullptr);
            attempts++;
        }

        if (rc == 1) {
            printf("  Frame %d: %ux%u  %u bytes  ts=%.3fs  id=%llu\n",
                   f + 1, w, h, len, ts / 1000000.0, (unsigned long long)fid);
            cmln_capture_free_frame(cap);  // drop Rust-owned buffer
        } else {
            printf("  (timed out after 1s)\n");
            break;
        }
    }

    // 6. Cleanup
    cmln_capture_destroy(cap);
    cmln_camera_close(cam);
    cmln_free_enum(cams, count);

    printf("Done.\n");
    return 0;
}
