/*
 * ASAN/UBSAN smoke test for the sdr-aaronia-rs C FFI boundary.
 *
 * Exercises the lifecycle entry points exposed by `include/spectran.h`
 * without requiring a real Spectran device, HTTP server, or RTSA file.
 * The goal is to surface use-after-free, double-free, and other
 * cross-language UB that Miri can't see because Miri rejects FFI.
 *
 * Build & run via `tests/asan/run_asan.sh`. The runner sets RUSTFLAGS
 * to enable AddressSanitizer + UndefinedBehaviorSanitizer on both the
 * Rust cdylib and this C program.
 *
 * Any sanitizer hit aborts the process and the CI step fails.
 */

#include <assert.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "spectran.h"

/* ----- Test helpers ----- */

static int failures = 0;

#define CHECK(cond, msg)                                                       \
    do {                                                                       \
        if (!(cond)) {                                                         \
            fprintf(stderr, "FAIL: %s (%s:%d)\n", (msg), __FILE__, __LINE__);  \
            failures++;                                                        \
        } else {                                                               \
            printf("  OK: %s\n", (msg));                                       \
        }                                                                      \
    } while (0)

/* ----- Cases ----- */

/* All `*_free` entry points must accept NULL without faulting. */
static void test_null_safe_frees(void) {
    printf("test_null_safe_frees:\n");
    spectran_source_builder_free(NULL);
    spectran_source_free(NULL);
    spectran_endpoints_client_free(NULL);
    spectran_server_info_free(NULL);
    spectran_source_info_free(NULL);
    spectran_source_capabilities_free(NULL);
    spectran_string_free(NULL);
    /* Reaching here without crashing is the success criterion. */
    CHECK(1, "all *_free(NULL) returned without UB");
}

/* Builder lifecycle: new → set fields → free. No build() call: that
 * path would touch a tokio runtime and a file/network resource, which
 * is too much surface for a portable smoke test. */
static void test_builder_lifecycle(void) {
    printf("test_builder_lifecycle:\n");
    SpectranSourceBuilder *b = spectran_source_builder_new();
    CHECK(b != NULL, "spectran_source_builder_new returned non-null");
    if (!b) return;

    spectran_source_builder_center_frequency_hz(b, 2.4e9);
    spectran_source_builder_sample_rate_hz(b, 20e6);
    spectran_source_builder_reference_level_dbm(b, -20.0);
    spectran_source_builder_file_source(b, "/dev/null");

    /* Passing NULL strings should not crash. */
    spectran_source_builder_http_source(b, NULL);
    spectran_source_builder_file_source(b, NULL);

    spectran_source_builder_free(b);
    CHECK(1, "builder lifecycle (new -> setters -> free) completed");
}

/* Setters accept NULL builder gracefully (no segfault). */
static void test_setters_null_builder(void) {
    printf("test_setters_null_builder:\n");
    spectran_source_builder_center_frequency_hz(NULL, 1e9);
    spectran_source_builder_sample_rate_hz(NULL, 1e6);
    spectran_source_builder_reference_level_dbm(NULL, 0.0);
    spectran_source_builder_http_source(NULL, "http://example.com");
    spectran_source_builder_file_source(NULL, "/dev/null");
    CHECK(1, "setters with NULL builder returned without UB");
}

/* Error-code → human string round-trip. The returned string is owned
 * by the caller and must be freed via spectran_string_free. */
static void test_error_message_roundtrip(void) {
    printf("test_error_message_roundtrip:\n");
    SpectranFfiError codes[] = {
        Success, NullPointer, InvalidString, InternalError, BuildFailed, ReadError
    };
    for (size_t i = 0; i < sizeof(codes) / sizeof(codes[0]); ++i) {
        char *msg = spectran_get_error_message(codes[i]);
        CHECK(msg != NULL, "spectran_get_error_message returned non-null");
        if (msg) {
            CHECK(strlen(msg) > 0, "error message has positive length");
            spectran_string_free(msg);
        }
    }
}

/* Endpoints client lifecycle: new with garbage URL → free. The new()
 * call may return NULL for malformed URLs; we only check that whichever
 * path it takes is sanitizer-clean. */
static void test_endpoints_client_lifecycle(void) {
    printf("test_endpoints_client_lifecycle:\n");
    HttpEndpointsClient *c = spectran_endpoints_client_new("http://127.0.0.1:1");
    if (c) {
        spectran_endpoints_client_free(c);
        CHECK(1, "endpoints client (new -> free) completed");
    } else {
        CHECK(1, "endpoints client new returned NULL (acceptable for unreachable URL)");
    }

    /* Malformed URL should return NULL, not crash. */
    HttpEndpointsClient *bad = spectran_endpoints_client_new("not a url");
    if (bad) spectran_endpoints_client_free(bad);
    CHECK(1, "endpoints client with malformed URL returned without UB");
}

/* Capability entry points on a NULL source. `get_capabilities` must
 * answer NULL rather than dereference, and the free that follows must
 * accept it — the struct owns three C strings plus two heap arrays, so
 * a partially-populated free is where a double-free would surface. */
static void test_capabilities_null_source(void) {
    printf("test_capabilities_null_source:\n");
    FfiDeviceCapabilities *caps = spectran_source_get_capabilities(NULL);
    CHECK(caps == NULL, "get_capabilities(NULL) returned NULL");
    spectran_source_capabilities_free(caps);
    CHECK(1, "capabilities_free of a NULL result returned without UB");

    /* The error string the failed call left behind is caller-owned. */
    char *err = spectran_last_error();
    if (err) spectran_string_free(err);
    CHECK(1, "last_error after a failed get_capabilities freed cleanly");
}

/* Compile-time capability query: no arguments, no state, so the only
 * thing to check is that it links and answers a valid bool. */
static void test_sink_supported(void) {
    printf("test_sink_supported:\n");
    bool tx = spectran_sink_supported();
    CHECK(tx == true || tx == false, "sink_supported returned a valid bool");
}

int main(void) {
    printf("=== sdr-aaronia-rs ASAN/UBSAN smoke test ===\n");
    test_null_safe_frees();
    test_builder_lifecycle();
    test_setters_null_builder();
    test_error_message_roundtrip();
    test_endpoints_client_lifecycle();
    test_capabilities_null_source();
    test_sink_supported();
    printf("=== %d failure%s ===\n", failures, failures == 1 ? "" : "s");
    return failures == 0 ? 0 : 1;
}
