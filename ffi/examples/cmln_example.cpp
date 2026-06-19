/*
 * Full example: enumerate → open → load_context → block capture → async → free_frame.
 *
 * Build (same as cmln_minimal):
 *   g++ -std=c++17 -I<cameleon-root>/ffi/include \
 *       -L<cameleon-root>/target/release \
 *       -Wl,-rpath <path> \
 *       -o cmln_example.cpp -lcameleon_ffi -ldl -lpthread
 */

#include <cstdio>
#include <cstdint>
#include <cstring>
#include <cstdlib>
#include <signal.h>
#include <cstring>
#include <ctime>

// Make sure we can use strsignal
#include <string.h>

#include "cameleon_ffi.h"

static volatile sig_atomic_t s_got_signal = 0;
static void sighandler(int) { s_got_signal = 1; }

int main_signal(int argc, char **argv) {
    fprintf(stderr, "cameleon FFI example\n");
    fprintf(stderr, "Usage: %s [--block] [--count N]\n", argv[0]);
    fprintf(stderr, "\n");
    fprintf(stderr, "Options:\n");
    fprintf(stderr, "  --block     Use blocking frame receive\n");
    fprintf(stderr, "  --count N   Number of frames (default: 10)\n");
    return 0;
}

int main_run(int argc, char **argv) {
    bool blocking = false;
    int frame_count = 10;

    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--block") == 0) { blocking = true; }
        else if (strcmp(argv[i], "--count") == 0 && i + 1 < argc) {
            frame_count = atoi(argv[i + 1]);
            i++;
        }
        else { return main_signal(argc, argv); }
    }

    signal(SIGINT, sighandler);
    signal(SIGTERM, sighandler);

    // ── 1. Enumerate ─────────────────────────────────────────────
    fprintf(stderr, "[enumerate] scanning...\n");
    CmlnCamera **cams = nullptr;
    uint32_t count = 0;
    int rc = cmln_enumerate(&cams, &count);
    if (rc != CMLN_OK) {
        fprintf(stderr, "enumerate: FAILED\n");
        return 1;
    }
    fprintf(stderr, "  found %u camera(s)\n", count);

    if (count == 0) {
        cmln_free_enum(cams, count);
        return 0;
    }

    // ── 2. Open ──────────────────────────────────────────────────
    CmlnCamera *cam = cams[0];
    char vendor[128], model[128], serial[128];
    cmln_camera_info(cam, vendor, sizeof(vendor),
                     model, sizeof(model),
                     serial, sizeof(serial));
    fprintf(stderr, "  %s / %s / %s\n", vendor, model, serial);

    rc = cmln_camera_open(cam);
    if (rc != CMLN_OK) {
        fprintf(stderr, "open: FAILED (rc=%d)\n", rc);
        cmln_free_enum(cams, count);
        return 1;
    }
    fprintf(stderr, "  opened\n");

    // ── 3. Load context ──────────────────────────────────────────
    rc = cmln_camera_load_context(cam);
    if (rc != CMLN_OK) {
        fprintf(stderr, "load_context: FAILED\n");
        cmln_camera_close(cam);
        cmln_free_enum(cams, count);
        return 1;
    }
    fprintf(stderr, "  context loaded\n");

    // ── 4. Start capture ─────────────────────────────────────────
    CmlnCapture *cap = cmln_capture_start(cam);
    if (!cap) {
        fprintf(stderr, "capture_start: FAILED\n");
        cmln_camera_close(cam);
        cmln_free_enum(cams, count);
        return 1;
    }
    fprintf(stderr, "  capture started\n");

    // ── 5. Grab frames ───────────────────────────────────────────
    fprintf(stderr, "  grabbing %d frames (%s mode)...\n", frame_count,
            blocking ? "block" : "async");

    uint32_t w = 0, h = 0, len = 0;
    for (int f = 0; f < frame_count && !s_got_signal; f++) {
        uint64_t ts = 0, fid = 0;
        const uint8_t *data = nullptr;

        if (blocking) {
            rc = cmln_capture_block(cap, &w, &h, &len, &ts, &fid, &data);
        } else {
            int attempts = 0;
            while (attempts < 200 && !s_got_signal) {
                rc = cmln_capture_async(cap, &w, &h, &len, &ts, &fid, &data);
                if (rc == 1) break;
                struct timespec tv = {0, 5000000}; // 5ms
                nanosleep(&tv, nullptr);
                attempts++;
            }
        }

        if (rc == 1 && data && len > 0) {
            fprintf(stdout,
                    "%2d  %4ux%4u  %7u bytes  ts%12.3fs  id=%llu\n",
                    f + 1, w, h, len, ts / 1000000.0, (unsigned long long)fid);
            cmln_capture_free_frame(cap);
        } else if (rc == 0) {
            fprintf(stderr, "  (stopped or timed out)\n");
            break;
        }
    }

    // ── 6. Cleanup ───────────────────────────────────────────────
    fprintf(stderr, "  cleaning up...\n");
    cmln_capture_destroy(cap);
    cmln_camera_close(cam);
    cmln_free_enum(cams, count);
    fprintf(stderr, "  done\n");

    return 0;
}

int main(int argc, char **argv) {
    return (argc <= 1) ? main_signal(argc, argv) : main_run(argc, argv);
}
