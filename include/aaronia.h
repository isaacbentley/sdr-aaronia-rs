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

AaroniaSourceBuilder* aaronia_source_builder_new();
void aaronia_source_builder_free(AaroniaSourceBuilder* builder);
void aaronia_source_builder_center_frequency(AaroniaSourceBuilder* builder, double freq);
void aaronia_source_builder_span_frequency(AaroniaSourceBuilder* builder, double freq);
void aaronia_source_builder_reference_level(AaroniaSourceBuilder* builder, double level);
void aaronia_source_builder_http_source(AaroniaSourceBuilder* builder, const char* base_url);
void aaronia_source_builder_file_source(AaroniaSourceBuilder* builder, const char* file_path);
AaroniaSource* aaronia_source_build(AaroniaSourceBuilder* builder);

// --- AaroniaSource FFI --- //

void aaronia_source_free(AaroniaSource* source);
intptr_t aaronia_source_read_samples(AaroniaSource* source, FfiComplex* buffer, uintptr_t len);
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

typedef struct AaroniaSinkBuilder AaroniaSinkBuilder;
typedef struct AaroniaSink AaroniaSink; // Opaque UnifiedSink

AaroniaSinkBuilder* aaronia_sink_builder_new(void);
void aaronia_sink_builder_free(AaroniaSinkBuilder* builder);
AaroniaSink* aaronia_sink_build(AaroniaSinkBuilder* builder);
void aaronia_sink_free(AaroniaSink* sink);
AaroniaFfiError aaronia_sink_initialize(AaroniaSink* sink);
AaroniaFfiError aaronia_sink_stop_streaming(AaroniaSink* sink);
AaroniaFfiError aaronia_sink_write_samples(
    AaroniaSink* sink,
    int32_t channel,
    double start_time_s,
    double end_time_s,
    const float _Complex* samples,
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
