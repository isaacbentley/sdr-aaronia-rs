#ifndef AARONIA_H
#define AARONIA_H

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

// --- FFI Error Handling --- //

typedef enum AaroniaFfiError {
    Success = 0,
    NullPointer = 1,
    InvalidString = 2,
    InternalError = 3,
    BuildFailed = 4,
    ReadError = 5,
    // Entry point invoked from a thread context that cannot block
    // (a current-thread tokio runtime). Call from a plain thread instead.
    RuntimeContext = 6,
} AaroniaFfiError;

// --- C-compatible SourceType --- //
typedef enum CAaroniaSourceType {
    NativeSdk,
    Http,
    File,
} CAaroniaSourceType;

// --- C-compatible Complex struct --- //
typedef struct FfiComplex {
    float re;
    float im;
} FfiComplex;

// --- C-compatible ServerInfo struct --- //
typedef struct FfiServerInfo {
    const char* name;
    const char* version;
    const char* build;
    const char* serial;
    const char* title;
    const char* mission;
} FfiServerInfo;

// --- C-compatible SourceInfo struct --- //
typedef struct FfiSourceInfo {
    CAaroniaSourceType source_type;
    double center_frequency;
    // IQ sample rate (Fs) in Hz.
    double span_frequency;
    // Usable RX/real-time bandwidth in Hz; 0.0 = unknown.
    // Always <= span_frequency.
    double bandwidth_hz;
    double reference_level;
    const char* device_serial;
} FfiSourceInfo;

// Opaque pointers
typedef struct AaroniaSourceBuilder AaroniaSourceBuilder;
typedef struct AaroniaSource AaroniaSource;
typedef struct HttpEndpointsClient HttpEndpointsClient;

// --- AaroniaSourceBuilder FFI --- //
//
// Ownership: `aaronia_source_build` BORROWS the builder — the caller
// retains ownership and must still free it with
// `aaronia_source_builder_free`. The same convention applies to the
// sink builder below.

AaroniaSourceBuilder* aaronia_source_builder_new();
void aaronia_source_builder_free(AaroniaSourceBuilder* builder);
void aaronia_source_builder_center_frequency(AaroniaSourceBuilder* builder, double freq);
void aaronia_source_builder_span_frequency(AaroniaSourceBuilder* builder, double freq);
void aaronia_source_builder_reference_level(AaroniaSourceBuilder* builder, double level);
void aaronia_source_builder_http_source(AaroniaSourceBuilder* builder, const char* base_url);
void aaronia_source_builder_file_source(AaroniaSourceBuilder* builder, const char* file_path);
void aaronia_source_builder_device_serial(AaroniaSourceBuilder* builder, const char* serial);
// RX channel selection (native-SDK backend): 0 = Rx1 (default),
// 1 = Rx2, 2 = Rx1+Rx2 dual capture (read with
// aaronia_source_read_samples_dual). Other values ignored.
void aaronia_source_builder_receiver_channel(AaroniaSourceBuilder* builder, int32_t channel);
// HTTP wire format: "F32" (default), "F16", or "I16" (true
// low-bandwidth wire mode). Unknown strings ignored.
void aaronia_source_builder_stream_format(AaroniaSourceBuilder* builder, const char* format);
// Server-side integer encode multiplier for integer wire formats.
void aaronia_source_builder_stream_scale(AaroniaSourceBuilder* builder, double scale);
AaroniaSource* aaronia_source_build(AaroniaSourceBuilder* builder);

// --- AaroniaSource FFI --- //
//
// Read return codes: >= 0 samples read; -1 generic error (details via
// aaronia_last_error); -3 timeout — a private convention of this API,
// chosen so SoapySDR wrappers can map it 1:1 onto SOAPY_SDR_TIMEOUT.

void aaronia_source_free(AaroniaSource* source);
intptr_t aaronia_source_read_samples(AaroniaSource* source, FfiComplex* buffer, uintptr_t len);
// Deadline-bounded read: waits at most timeout_us microseconds and
// returns partial data collected within the deadline; returns -3 only
// when the deadline passes with zero samples. timeout_us == 0 drains
// already-buffered samples without waiting.
intptr_t aaronia_source_read_samples_timeout(AaroniaSource* source, FfiComplex* buffer, uintptr_t len, uint64_t timeout_us);
// Dual-channel read (requires receiver_channel == 2 at build time and
// the native-SDK backend): fills rx1/rx2 with equal numbers of
// time-aligned samples; returns the pair count or -1.
intptr_t aaronia_source_read_samples_dual(AaroniaSource* source, FfiComplex* rx1, FfiComplex* rx2, uintptr_t len);
bool aaronia_source_take_overrun(AaroniaSource* source);
uint64_t aaronia_source_get_cumulative_drops(AaroniaSource* source);
int64_t aaronia_source_get_last_timestamp_ns(AaroniaSource* source);
bool aaronia_source_get_gps_time(AaroniaSource* source, double* out_gps_time);
AaroniaFfiError aaronia_source_start_streaming(AaroniaSource* source);
AaroniaFfiError aaronia_source_stop_streaming(AaroniaSource* source);
AaroniaFfiError aaronia_source_set_center_frequency(AaroniaSource* source, double freq_hz);
AaroniaFfiError aaronia_source_set_span_frequency(AaroniaSource* source, double span_hz);
AaroniaFfiError aaronia_source_set_reference_level(AaroniaSource* source, double ref_level_dbm);
FfiSourceInfo* aaronia_source_get_source_info(AaroniaSource* source);
void aaronia_source_info_free(FfiSourceInfo* info);

// --- Sink FFI --- //
//
// WARNING: the whole TX path is hardware-unverified (driven per the
// vendor samples, never confirmed to emit RF on a live device) and
// requires the native SDK: it works only in `native-sdk` builds on
// Windows/Linux. Elsewhere aaronia_sink_initialize fails with a
// descriptive error retrievable via aaronia_last_error().
//
// Ownership: aaronia_sink_build BORROWS the builder (same convention
// as the source builder — free it with aaronia_sink_builder_free).

typedef struct AaroniaSinkBuilder AaroniaSinkBuilder;
typedef struct AaroniaSink AaroniaSink; // Opaque UnifiedSink

// TX packet-boundary flags for aaronia_sink_write_samples. Pass
// START|END|PUSH for a self-contained burst; continuous multi-packet
// streams mark only the first/last packet.
#define AARONIA_TX_STREAM_START  ((uint64_t)0x00000001)
#define AARONIA_TX_STREAM_END    ((uint64_t)0x00000002)
#define AARONIA_TX_SEGMENT_START ((uint64_t)0x00000004)
#define AARONIA_TX_SEGMENT_END   ((uint64_t)0x00000008)
#define AARONIA_TX_PUSH          ((uint64_t)0x00008000)

AaroniaSinkBuilder* aaronia_sink_builder_new(void);
void aaronia_sink_builder_free(AaroniaSinkBuilder* builder);
void aaronia_sink_builder_center_frequency(AaroniaSinkBuilder* builder, double hz);
void aaronia_sink_builder_sample_rate(AaroniaSinkBuilder* builder, double hz);
void aaronia_sink_builder_trans_gain(AaroniaSinkBuilder* builder, double db);
AaroniaSink* aaronia_sink_build(AaroniaSinkBuilder* builder);
void aaronia_sink_free(AaroniaSink* sink);
// Loads the native SDK, opens the first matching device, configures
// the IQ transmitter from the builder settings, and starts the TX
// stream. Blocking.
AaroniaFfiError aaronia_sink_initialize(AaroniaSink* sink);
AaroniaFfiError aaronia_sink_stop_streaming(AaroniaSink* sink);
// start_time_s / end_time_s are in device MASTER STREAM TIME seconds,
// not wall-clock epoch time. Samples use the same FfiComplex layout as
// the read path.
AaroniaFfiError aaronia_sink_write_samples(
    AaroniaSink* sink,
    int32_t channel,
    double start_time_s,
    double end_time_s,
    uint64_t flags,
    const FfiComplex* samples,
    size_t num_samples
);

// --- Remote Control FFI --- //

HttpEndpointsClient* aaronia_endpoints_client_new(const char* base_url);
void aaronia_endpoints_client_free(HttpEndpointsClient* client);
FfiServerInfo* aaronia_endpoints_client_get_info(HttpEndpointsClient* client);
void aaronia_server_info_free(FfiServerInfo* info);
AaroniaFfiError aaronia_endpoints_client_control_streaming(HttpEndpointsClient* client, bool start);
AaroniaFfiError aaronia_endpoints_client_control_recording(HttpEndpointsClient* client, bool start, const char* name);

// --- General FFI Utilities --- //

void aaronia_string_free(char* s);
char* aaronia_get_error_message(AaroniaFfiError error_code);

// Returns the last error message recorded on the calling thread, or NULL
// if no error has been recorded since the last successful call. Free the
// returned string with aaronia_string_free().
char* aaronia_last_error(void);

#ifdef __cplusplus
}
#endif

#endif // AARONIA_H
