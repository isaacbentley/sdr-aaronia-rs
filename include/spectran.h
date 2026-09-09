#ifndef SPECTRAN_H
#define SPECTRAN_H

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// --- FFI Error Handling --- //

typedef enum SpectranFfiError {
    Success = 0,
    NullPointer = 1,
    InvalidString = 2,
    InternalError = 3,
    BuildFailed = 4,
    ReadError = 5,
    // Entry point invoked from a thread context that cannot block
    // (a current-thread tokio runtime). Call from a plain thread instead.
    RuntimeContext = 6,
} SpectranFfiError;

// --- C-compatible SourceType --- //
typedef enum CSpectranSourceType {
    NativeSdk,
    Http,
    File,
} CSpectranSourceType;

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
    CSpectranSourceType source_type;
    double center_frequency_hz;
    // IQ sample rate (Fs) in Hz.
    double sample_rate_hz;
    // Usable RX/real-time bandwidth in Hz; 0.0 = unknown.
    // Always <= sample_rate_hz.
    double bandwidth_hz;
    double reference_level_dbm;
    const char* device_serial;
} FfiSourceInfo;

// --- What the device reports about itself --- //
//
// Optionality is explicit: 0.0 is a legitimate reference level and a
// legitimate step, so no numeric sentinel could distinguish "the device
// declares 0" from "the device did not say". Strings are NULL when
// absent, the ranges carry a has_ flag, and sample_rate_count == 0
// means the ladder could not be derived. Fall back per field, not on
// the whole struct.
typedef struct FfiDeviceCapabilities {
    const char* model;    // e.g. "SPECTRAN V6 ECO"; NULL when unknown
    const char* serial;   // NULL when unknown
    const char* version;  // firmware/FPGA revisions; NULL when unknown

    bool   has_center_frequency;
    double center_frequency_min_hz;
    double center_frequency_max_hz;
    double center_frequency_step_hz;  // 0.0 = device declares no step

    bool   has_reference_level;
    double reference_level_min_dbm;
    double reference_level_max_dbm;
    double reference_level_step_db;   // 0.0 = device declares no step

    // Settable IQ sample rates in Hz, highest first. Owned by this
    // struct; NULL and 0 when the device could not be asked.
    size_t        sample_rate_count;
    const double* sample_rates;

    // Stream-clock sources in the device's own vocabulary (a V6 ECO:
    // Consumer, Oscillator, GPS, PPS, 10MHz, and three "... Provider"
    // variants). Owned by this struct; NULL and 0 when it did not say.
    size_t              clock_source_count;
    const char* const*  clock_sources;
    const char*         clock_source;   // currently selected; NULL if unknown
    const char*         rx_antenna;     // e.g. "RX1"; NULL if the mode names none
} FfiDeviceCapabilities;

/* Live sensor readings. A plain value struct the caller allocates; every
 * field is a reading or NaN for "not reported". No ownership, nothing to
 * free. Fill it with spectran_source_get_sensors. */
typedef struct FfiDeviceSensors {
    double fpga_temp_c;
    double frontend_temp_c;
    double board_power_w;
    double adc_range_db;              // ADC headroom below full scale, dB
    double usb_buffer_fill;          // fraction 0.0-1.0
    double dsp_buffer_fill;          // fraction 0.0-1.0
    double errors_per_second;
    double usb_overflows_per_second;
    double dsp_overflows_per_second;
    double gps_satellites;
    double gps_latitude;             // degrees; 0 with no fix
    double gps_longitude;            // degrees; 0 with no fix
} FfiDeviceSensors;

// Opaque pointers
typedef struct SpectranSourceBuilder SpectranSourceBuilder;
typedef struct SpectranSource SpectranSource;
typedef struct HttpEndpointsClient HttpEndpointsClient;

// --- SpectranSourceBuilder FFI --- //
//
// Ownership: `spectran_source_build` BORROWS the builder — the caller
// retains ownership and must still free it with
// `spectran_source_builder_free`. The same convention applies to the
// sink builder below.

SpectranSourceBuilder* spectran_source_builder_new();
void spectran_source_builder_free(SpectranSourceBuilder* builder);
void spectran_source_builder_center_frequency_hz(SpectranSourceBuilder* builder, double hz);
void spectran_source_builder_sample_rate_hz(SpectranSourceBuilder* builder, double hz);
void spectran_source_builder_reference_level_dbm(SpectranSourceBuilder* builder, double dbm);
void spectran_source_builder_http_source(SpectranSourceBuilder* builder, const char* base_url);
void spectran_source_builder_file_source(SpectranSourceBuilder* builder, const char* file_path);
void spectran_source_builder_device_serial(SpectranSourceBuilder* builder, const char* serial);

/* Pin the source to one backend instead of auto-detecting. Forcing
 * NativeSdk makes a missing SDK a build error rather than a silent
 * fallback to localhost HTTP. */
void spectran_source_builder_force_source_type(SpectranSourceBuilder *builder,
                                              CSpectranSourceType source_type);

/* Whether the native SDK library is present, by the search the builder
 * uses; does not load it. */
bool spectran_sdk_installed(void);

/* Alias-free bandwidth (Hz) delivered at an IQ sample rate; smaller than
 * the rate. Stateless. */
double spectran_usable_bandwidth_hz(double sample_rate_hz);

/* The IQ sample rate (Hz) whose alias-free bandwidth covers bandwidth_hz;
 * the inverse of spectran_usable_bandwidth_hz. Stateless. */
double spectran_iq_sample_rate_for_bandwidth(double bandwidth_hz);
// RX channel selection (native-SDK backend): 0 = Rx1 (default),
// 1 = Rx2, 2 = Rx1+Rx2 dual capture (read with
// spectran_source_read_samples_dual). Other values ignored.
void spectran_source_builder_receiver_channel(SpectranSourceBuilder* builder, int32_t channel);
// HTTP wire format: "F32" (default), "F16", or "I16" (true
// low-bandwidth wire mode). Unknown strings ignored.
void spectran_source_builder_stream_format(SpectranSourceBuilder* builder, const char* format);
// Server-side integer encode multiplier for integer wire formats.
void spectran_source_builder_stream_scale(SpectranSourceBuilder* builder, double scale);
// Blocking-read timeout in microseconds (default 30 s). Applies to
// spectran_source_read_samples; spectran_source_read_samples_timeout uses
// its own per-call deadline. 0 is ignored.
void spectran_source_builder_read_timeout_us(SpectranSourceBuilder* builder, uint64_t timeout_us);
// Automatic reconnection of the HTTP sample stream (default true). When
// false, a dropped stream ends the session and later reads error out.
void spectran_source_builder_auto_reconnect(SpectranSourceBuilder* builder, bool enabled);
SpectranSource* spectran_source_build(SpectranSourceBuilder* builder);

// --- SpectranSource FFI --- //
//
// Read return codes: >= 0 samples read; -1 generic error (details via
// spectran_last_error); -3 timeout — a private convention of this API,
// chosen so SoapySDR wrappers can map it 1:1 onto SOAPY_SDR_TIMEOUT.

void spectran_source_free(SpectranSource* source);
intptr_t spectran_source_read_samples(SpectranSource* source, FfiComplex* buffer, uintptr_t len);
// Deadline-bounded read: waits at most timeout_us microseconds and
// returns partial data collected within the deadline; returns -3 only
// when the deadline passes with zero samples. timeout_us == 0 drains
// already-buffered samples without waiting.
intptr_t spectran_source_read_samples_timeout(SpectranSource* source, FfiComplex* buffer, uintptr_t len, uint64_t timeout_us);
// Dual-channel read (requires receiver_channel == 2 at build time and
// the native-SDK backend): fills rx1/rx2 with equal numbers of
// time-aligned samples; returns the pair count or -1.
intptr_t spectran_source_read_samples_dual(SpectranSource* source, FfiComplex* rx1, FfiComplex* rx2, uintptr_t len);
bool spectran_source_take_overrun(SpectranSource* source);
uint64_t spectran_source_get_cumulative_drops(SpectranSource* source);
int64_t spectran_source_get_last_timestamp_ns(SpectranSource* source);
// Nanoseconds since the Unix epoch, matching get_last_timestamp_ns.
// out_gps_time_ns may be NULL to probe only whether a fix exists.
// The device reports seconds as a double; the conversion lives in the
// library now, so callers no longer split whole/fractional seconds
// themselves. Resolution is ~240 ns, set by the vendor's own double.
bool spectran_source_get_gps_time_ns(SpectranSource* source, int64_t* out_gps_time_ns);
SpectranFfiError spectran_source_start_streaming(SpectranSource* source);
SpectranFfiError spectran_source_stop_streaming(SpectranSource* source);
SpectranFfiError spectran_source_set_center_frequency_hz(SpectranSource* source, double hz);
SpectranFfiError spectran_source_set_sample_rate_hz(SpectranSource* source, double hz);
SpectranFfiError spectran_source_set_reference_level_dbm(SpectranSource* source, double dbm);
SpectranFfiError spectran_source_set_clock_source(SpectranSource* source, const char* clock_source);
FfiSourceInfo* spectran_source_get_source_info(SpectranSource* source);
void spectran_source_info_free(FfiSourceInfo* info);

// Read the device's declared capabilities. BLOCKING: two control-plane
// GETs, so call it once and cache. Returns NULL only for a null source
// or when called from a current-thread tokio runtime; a device that
// cannot be asked yields a struct with every field absent. Answers for
// the HTTP backend — file and native-SDK sources report nothing, having
// no equivalent surface to ask.
FfiDeviceCapabilities* spectran_source_get_capabilities(SpectranSource* source);
void spectran_source_capabilities_free(FfiDeviceCapabilities* caps);

/* Fill *out with the device's live sensors. Returns true when the read
 * completed (fields may be NaN where unreported; file/native-SDK backends
 * complete all-NaN), false (out untouched) for a null pointer or a
 * current-thread runtime. Blocking: one /healthstatus GET on HTTP. */
bool spectran_source_get_sensors(SpectranSource* source, FfiDeviceSensors* out);

// --- Sink FFI --- //
//
// WARNING: the whole TX path is hardware-unverified (driven per the
// vendor samples, never confirmed to emit RF on a live device) and
// requires the native SDK: it works only in `native-sdk` builds on
// Windows/Linux. Elsewhere spectran_sink_initialize fails with a
// descriptive error retrievable via spectran_last_error().
//
// Ownership: spectran_sink_build BORROWS the builder (same convention
// as the source builder — free it with spectran_sink_builder_free).

typedef struct SpectranSinkBuilder SpectranSinkBuilder;
typedef struct SpectranSink SpectranSink; // Opaque UnifiedSink

// TX packet-boundary flags for spectran_sink_write_samples. Pass
// START|END|PUSH for a self-contained burst; continuous multi-packet
// streams mark only the first/last packet.
#define SPECTRAN_TX_STREAM_START  ((uint64_t)0x00000001)
#define SPECTRAN_TX_STREAM_END    ((uint64_t)0x00000002)
#define SPECTRAN_TX_SEGMENT_START ((uint64_t)0x00000004)
#define SPECTRAN_TX_SEGMENT_END   ((uint64_t)0x00000008)
#define SPECTRAN_TX_PUSH          ((uint64_t)0x00008000)

// True when this build carries the native-SDK transmit path. Ask before
// advertising a TX capability: spectran_sink_build succeeds everywhere,
// so a non-null sink proves nothing. Compile-time only — it cannot say
// whether the attached device has a transmitter (a V6 ECO does not), and
// TX also requires the native-SDK source backend, so a device opened
// over HTTP or from a file has no TX path regardless.
bool spectran_sink_supported(void);

SpectranSinkBuilder* spectran_sink_builder_new(void);
void spectran_sink_builder_free(SpectranSinkBuilder* builder);
void spectran_sink_builder_center_frequency_hz(SpectranSinkBuilder* builder, double hz);
void spectran_sink_builder_sample_rate_hz(SpectranSinkBuilder* builder, double hz);
void spectran_sink_builder_trans_gain_db(SpectranSinkBuilder* builder, double db);
SpectranSink* spectran_sink_build(SpectranSinkBuilder* builder);
void spectran_sink_free(SpectranSink* sink);
// Loads the native SDK, opens the first matching device, configures
// the IQ transmitter from the builder settings, and starts the TX
// stream. Blocking.
SpectranFfiError spectran_sink_initialize(SpectranSink* sink);
SpectranFfiError spectran_sink_stop_streaming(SpectranSink* sink);
// start_time_s / end_time_s are in device MASTER STREAM TIME seconds,
// not wall-clock epoch time. Samples use the same FfiComplex layout as
// the read path.
SpectranFfiError spectran_sink_write_samples(
    SpectranSink* sink,
    int32_t channel,
    double start_time_s,
    double end_time_s,
    uint64_t flags,
    const FfiComplex* samples,
    size_t num_samples
);

// --- Remote Control FFI --- //

HttpEndpointsClient* spectran_endpoints_client_new(const char* base_url);
void spectran_endpoints_client_free(HttpEndpointsClient* client);
FfiServerInfo* spectran_endpoints_client_get_info(HttpEndpointsClient* client);

/* Fill *out with the device's live sensors through a standalone client,
 * off the streaming source's read lock. Same reading and return contract
 * as spectran_source_get_sensors. */
bool spectran_endpoints_client_get_sensors(HttpEndpointsClient* client, FfiDeviceSensors* out);
void spectran_server_info_free(FfiServerInfo* info);
SpectranFfiError spectran_endpoints_client_control_streaming(HttpEndpointsClient* client, bool start);
SpectranFfiError spectran_endpoints_client_control_recording(HttpEndpointsClient* client, bool start, const char* name);

// --- General FFI Utilities --- //

void spectran_string_free(char* s);
// Takes the code as an int so any value is safe to pass; unknown codes
// yield "Unknown error code".
char* spectran_get_error_message(int error_code);

// Returns the last error message recorded on the calling thread, or NULL
// if no error has been recorded since the last successful call. Free the
// returned string with spectran_string_free().
char* spectran_last_error(void);

#ifdef __cplusplus
}
#endif

#endif // SPECTRAN_H
