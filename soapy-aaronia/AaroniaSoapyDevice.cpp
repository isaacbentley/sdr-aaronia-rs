#include "AaroniaSoapyDevice.hpp"
#include <SoapySDR/Logger.hpp>
#include <algorithm>
#include <cmath>
#include <stdexcept>

// Fetch-and-free the thread-local Rust error string.
static std::string lastErrorOr(const char *fallback) {
    char *msg = aaronia_last_error();
    std::string out = msg ? msg : fallback;
    if (msg) aaronia_string_free(msg);
    return out;
}

AaroniaSoapyDevice::AaroniaSoapyDevice(AaroniaSource* source, AaroniaSink* sink, const SoapySDR::Kwargs &args)
    : _source(source), _sink(sink), _centerFrequency(100e6), _sampleRate(1e6),
      _txSampleRate(0.0), _referenceLevel(-20.0),
      _rxSetup(false), _txSetup(false), _rxStreamTag(0), _txStreamTag(0),
      _isStreaming(false)
{
    (void)args;
    if (!_source) {
        throw std::runtime_error("AaroniaSoapyDevice initialized with null source pointer");
    }

    FfiSourceInfo* info = aaronia_source_get_source_info(_source);
    if (info) {
        _centerFrequency = info->center_frequency;
        _sampleRate = info->span_frequency;
        _referenceLevel = info->reference_level;
        aaronia_source_info_free(info);
    }
}

AaroniaSoapyDevice::~AaroniaSoapyDevice() {
    std::lock_guard<std::mutex> lock(_mutex);
    if (_source) {
        if (_isStreaming) {
            aaronia_source_stop_streaming(_source);
        }
        aaronia_source_free(_source);
        _source = nullptr;
    }
    if (_sink) {
        // Stop unconditionally: the sink's stream lifecycle (started at
        // setupStream) is independent of the RX _isStreaming flag.
        aaronia_sink_stop_streaming(_sink);
        aaronia_sink_free(_sink);
        _sink = nullptr;
    }
}

std::string AaroniaSoapyDevice::getDriverKey() const {
    return "Aaronia";
}

std::string AaroniaSoapyDevice::getHardwareKey() const {
    return "Spectran V6";
}

SoapySDR::Kwargs AaroniaSoapyDevice::getHardwareInfo() const {
    SoapySDR::Kwargs info;
    info["driver"] = "Aaronia";
    info["hardware"] = "Spectran V6";
    return info;
}

size_t AaroniaSoapyDevice::getNumChannels(const int direction) const {
    if (direction == SOAPY_SDR_RX) return 1;
    // TX exists only when a sink backend was constructed (native-sdk
    // builds on Windows/Linux). Advertising a TX channel that every
    // write rejects (the old behaviour) breaks apps at stream time
    // instead of letting them see there is no TX.
    if (direction == SOAPY_SDR_TX) return _sink ? 1 : 0;
    return 0;
}

std::vector<std::string> AaroniaSoapyDevice::getStreamFormats(const int direction, const size_t channel) const {
    std::vector<std::string> formats;
    if (channel != 0) return formats;
    if (direction == SOAPY_SDR_RX) {
        formats.push_back(SOAPY_SDR_CF32);
        formats.push_back(SOAPY_SDR_CS16);
    } else if (direction == SOAPY_SDR_TX && _sink) {
        formats.push_back(SOAPY_SDR_CF32);
    }
    return formats;
}

std::string AaroniaSoapyDevice::getNativeStreamFormat(const int direction, const size_t channel, double &fullScale) const {
    (void)direction;
    (void)channel;
    // The C ABI transfers CF32 in both directions; CS16 is a
    // client-side conversion in this plugin, not the wire/native
    // format (an earlier revision claimed CS16-native, steering apps
    // toward the *more* expensive path).
    fullScale = 1.0;
    return SOAPY_SDR_CF32;
}

SoapySDR::Stream *AaroniaSoapyDevice::setupStream(
    const int direction,
    const std::string &format,
    const std::vector<size_t> &channels,
    const SoapySDR::Kwargs &args)
{
    (void)args;
    if (direction != SOAPY_SDR_RX && direction != SOAPY_SDR_TX) {
        throw std::runtime_error("Only RX and TX streams are supported");
    }
    if (!channels.empty() && (channels.size() != 1 || channels[0] != 0)) {
        throw std::runtime_error("Invalid channel selection; only channel 0 exists");
    }

    std::lock_guard<std::mutex> lock(_mutex);

    if (direction == SOAPY_SDR_RX) {
        if (format != SOAPY_SDR_CF32 && format != SOAPY_SDR_CS16) {
            throw std::runtime_error("Unsupported RX stream format: " + format);
        }
        _rxFormat = format;
        _rxSetup = true;
        return reinterpret_cast<SoapySDR::Stream *>(&_rxStreamTag);
    }

    // TX
    if (!_sink) {
        throw std::runtime_error("TX not available: no sink backend in this build");
    }
    if (format != SOAPY_SDR_CF32) {
        // Reject unsupported TX formats here, at setup time, per the
        // Soapy contract — not on the first write.
        throw std::runtime_error("Unsupported TX stream format: " + format + " (TX is CF32-only)");
    }
    AaroniaFfiError err = aaronia_sink_initialize(_sink);
    if (err != Success) {
        throw std::runtime_error(lastErrorOr("Failed to initialize sink"));
    }
    _txFormat = format;
    _txSetup = true;
    return reinterpret_cast<SoapySDR::Stream *>(&_txStreamTag);
}

void AaroniaSoapyDevice::closeStream(SoapySDR::Stream *stream) {
    std::lock_guard<std::mutex> lock(_mutex);
    if (stream == reinterpret_cast<SoapySDR::Stream *>(&_rxStreamTag)) {
        _rxSetup = false;
    } else if (stream == reinterpret_cast<SoapySDR::Stream *>(&_txStreamTag)) {
        if (_sink) aaronia_sink_stop_streaming(_sink);
        _txSetup = false;
    }
}

int AaroniaSoapyDevice::activateStream(
    SoapySDR::Stream *stream,
    const int flags,
    const long long timeNs,
    const size_t numElems)
{
    // Burst arguments are not supported by this hardware path; say so
    // instead of silently ignoring them.
    if (flags != 0 || timeNs != 0 || numElems != 0) {
        return SOAPY_SDR_NOT_SUPPORTED;
    }
    std::lock_guard<std::mutex> lock(_mutex);

    if (stream == reinterpret_cast<SoapySDR::Stream *>(&_txStreamTag)) {
        // The TX stream was brought up in setupStream (the sink's
        // initialize opens, configures, and starts the transmitter).
        return _txSetup ? 0 : SOAPY_SDR_STREAM_ERROR;
    }
    if (stream != reinterpret_cast<SoapySDR::Stream *>(&_rxStreamTag) || !_rxSetup) {
        return SOAPY_SDR_STREAM_ERROR;
    }

    if (_isStreaming) return 0;
    AaroniaFfiError err = aaronia_source_start_streaming(_source);
    if (err != Success) {
        SoapySDR::logf(SOAPY_SDR_ERROR, "activateStream failed: %s",
                       lastErrorOr("Failed to start streaming").c_str());
        return SOAPY_SDR_STREAM_ERROR;
    }
    _isStreaming = true;
    return 0;
}

int AaroniaSoapyDevice::deactivateStream(
    SoapySDR::Stream *stream,
    const int flags,
    const long long timeNs)
{
    (void)flags;
    (void)timeNs;
    std::lock_guard<std::mutex> lock(_mutex);

    if (stream == reinterpret_cast<SoapySDR::Stream *>(&_txStreamTag)) {
        if (_sink) aaronia_sink_stop_streaming(_sink);
        return 0;
    }
    if (stream != reinterpret_cast<SoapySDR::Stream *>(&_rxStreamTag)) {
        return SOAPY_SDR_STREAM_ERROR;
    }
    if (!_isStreaming) return 0;
    aaronia_source_stop_streaming(_source);
    _isStreaming = false;
    return 0;
}

int AaroniaSoapyDevice::readStream(
    SoapySDR::Stream *stream,
    void * const *buffs,
    const size_t numElems,
    int &flags,
    long long &timeNs,
    const long timeoutUs)
{
    if (stream != reinterpret_cast<SoapySDR::Stream *>(&_rxStreamTag)) {
        return SOAPY_SDR_STREAM_ERROR;
    }
    if (!buffs || !buffs[0]) return SOAPY_SDR_STREAM_ERROR;

    // Output flags describe *this* read; clear whatever the caller
    // passed in rather than OR-ing into it.
    flags = 0;

    // Serialize against retunes: the C ABI takes `&mut` on the Rust
    // side, so an unlocked readStream racing setFrequency is UB.
    std::lock_guard<std::mutex> lock(_mutex);

    const uint64_t timeout_us = timeoutUs > 0 ? static_cast<uint64_t>(timeoutUs) : 0;

    intptr_t read = -1;
    if (_rxFormat == SOAPY_SDR_CF32) {
        FfiComplex *out = static_cast<FfiComplex *>(buffs[0]);
        read = aaronia_source_read_samples_timeout(_source, out, numElems, timeout_us);
    } else if (_rxFormat == SOAPY_SDR_CS16) {
        if (_tempFloatBuffer.size() < numElems) {
            _tempFloatBuffer.resize(numElems);
        }
        read = aaronia_source_read_samples_timeout(_source, _tempFloatBuffer.data(), numElems, timeout_us);
        if (read > 0) {
            int16_t *out = static_cast<int16_t *>(buffs[0]);
            for (intptr_t i = 0; i < read; ++i) {
                // lrintf: round-to-nearest instead of truncation.
                out[i * 2]     = static_cast<int16_t>(std::lrintf(std::clamp(_tempFloatBuffer[i].re * 32767.0f, -32768.0f, 32767.0f)));
                out[i * 2 + 1] = static_cast<int16_t>(std::lrintf(std::clamp(_tempFloatBuffer[i].im * 32767.0f, -32768.0f, 32767.0f)));
            }
        }
    } else {
        return SOAPY_SDR_STREAM_ERROR;
    }

    if (read < 0) {
        if (read == -3) return SOAPY_SDR_TIMEOUT;
        SoapySDR::logf(SOAPY_SDR_ERROR, "readStream failed: %s",
                       lastErrorOr("stream error").c_str());
        return SOAPY_SDR_STREAM_ERROR;
    }

    if (read > 0) {
        if (aaronia_source_take_overrun(_source)) {
            flags |= SOAPY_SDR_END_ABRUPT;
        }
        // Timestamp of the most recently received network block — an
        // approximation for the first returned sample when older
        // buffered samples are included (HTTP backend only; 0 when
        // unavailable, in which case HAS_TIME stays unset).
        timeNs = aaronia_source_get_last_timestamp_ns(_source);
        if (timeNs != 0) flags |= SOAPY_SDR_HAS_TIME;
    }

    return static_cast<int>(read);
}

int AaroniaSoapyDevice::writeStream(
    SoapySDR::Stream *stream,
    const void * const *buffs,
    const size_t numElems,
    int &flags,
    const long long timeNs,
    const long timeoutUs)
{
    (void)timeNs;
    (void)timeoutUs;
    if (stream != reinterpret_cast<SoapySDR::Stream *>(&_txStreamTag)) {
        return SOAPY_SDR_STREAM_ERROR;
    }
    if (!buffs || !buffs[0]) return SOAPY_SDR_STREAM_ERROR;

    std::lock_guard<std::mutex> lock(_mutex);
    if (!_sink || !_txSetup) {
        return SOAPY_SDR_STREAM_ERROR;
    }

    // Burst timing is in the device's master-stream clock domain; a
    // caller-provided epoch timeNs cannot be mapped onto it without a
    // clock-transfer step this plugin does not implement, so timed
    // bursts are refused rather than transmitted at the wrong time.
    if (flags & SOAPY_SDR_HAS_TIME) {
        SoapySDR::logf(SOAPY_SDR_WARNING,
                       "writeStream: timed TX (SOAPY_SDR_HAS_TIME) is not supported; "
                       "samples are pushed for immediate transmission");
    }

    const double rate = _txSampleRate > 0.0 ? _txSampleRate : _sampleRate;
    const double duration_s = rate > 0.0 ? static_cast<double>(numElems) / rate : 0.0;

    // Each writeStream call is one self-contained burst, pushed for
    // immediate transmission (times relative to "now" in the device
    // clock are handled by the PUSH flag path in the SDK).
    const uint64_t burstFlags =
        AARONIA_TX_SEGMENT_START | AARONIA_TX_SEGMENT_END | AARONIA_TX_PUSH;

    AaroniaFfiError err = aaronia_sink_write_samples(
        _sink,
        0,
        0.0,
        duration_s,
        burstFlags,
        static_cast<const FfiComplex*>(buffs[0]),
        numElems
    );

    flags = 0;
    if (err != Success) {
        SoapySDR::logf(SOAPY_SDR_ERROR, "writeStream failed: %s",
                       lastErrorOr("write error").c_str());
        return SOAPY_SDR_STREAM_ERROR;
    }

    return static_cast<int>(numElems);
}

// --- Time API ---
bool AaroniaSoapyDevice::hasHardwareTime(const std::string &what) const {
    if (what == "GPS") {
        // Truthful probe: GPS time exists only on the native-SDK
        // backend with a valid fix. Answering "true" unconditionally
        // (the old behaviour) made apps timestamp data to 1970.
        std::lock_guard<std::mutex> lock(_mutex);
        double unused = 0.0;
        return aaronia_source_get_gps_time(_source, &unused);
    }
    if (what.empty()) {
        std::lock_guard<std::mutex> lock(_mutex);
        return aaronia_source_get_last_timestamp_ns(_source) != 0;
    }
    return false;
}

long long AaroniaSoapyDevice::getHardwareTime(const std::string &what) const {
    std::lock_guard<std::mutex> lock(_mutex);
    if (what == "GPS") {
        double gps_time_s = 0.0;
        if (aaronia_source_get_gps_time(_source, &gps_time_s)) {
            // Split integer/fractional seconds before scaling: a single
            // (s * 1e9) double multiply cannot represent epoch-scale
            // nanoseconds (53-bit mantissa vs ~61 bits needed) and
            // quantized GPS time to ~256 ns steps.
            const double whole_s = std::floor(gps_time_s);
            const double frac_s = gps_time_s - whole_s;
            return static_cast<long long>(whole_s) * 1000000000LL
                 + static_cast<long long>(std::lrint(frac_s * 1e9));
        }
        return 0;
    }
    // Default: the last stream timestamp.
    return aaronia_source_get_last_timestamp_ns(_source);
}

// --- Clocking API ---
std::vector<std::string> AaroniaSoapyDevice::listClockSources(void) const {
    return {"Internal"};
}

void AaroniaSoapyDevice::setClockSource(const std::string &source) {
    if (source != "Internal") {
        SoapySDR::logf(SOAPY_SDR_WARNING, "setClockSource('%s') not currently supported by Aaronia API bindings", source.c_str());
    }
}

std::string AaroniaSoapyDevice::getClockSource(void) const {
    return "Internal";
}

std::vector<std::string> AaroniaSoapyDevice::listAntennas(const int direction, const size_t channel) const {
    std::vector<std::string> ant;
    if (channel != 0) return ant;
    if (direction == SOAPY_SDR_RX) {
        ant.push_back("RX1");
    } else if (direction == SOAPY_SDR_TX && _sink) {
        ant.push_back("TX1");
    }
    return ant;
}

void AaroniaSoapyDevice::setAntenna(const int direction, const size_t channel, const std::string &name) {
    (void)direction;
    (void)channel;
    (void)name;
    // Single antenna per direction.
}

std::string AaroniaSoapyDevice::getAntenna(const int direction, const size_t channel) const {
    (void)channel;
    return direction == SOAPY_SDR_TX ? "TX1" : "RX1";
}

void AaroniaSoapyDevice::setFrequency(const int direction, const size_t channel, const std::string &name, const double frequency, const SoapySDR::Kwargs &args) {
    (void)channel;
    (void)name;
    (void)args;
    if (direction != SOAPY_SDR_RX) return;
    std::lock_guard<std::mutex> lock(_mutex);

    AaroniaFfiError err = aaronia_source_set_center_frequency(_source, frequency);
    if (err == Success) {
        _centerFrequency = frequency;
    } else {
        SoapySDR::logf(SOAPY_SDR_ERROR, "setFrequency failed: %s",
                       lastErrorOr("unknown error").c_str());
    }
}

double AaroniaSoapyDevice::getFrequency(const int direction, const size_t channel, const std::string &name) const {
    (void)direction;
    (void)channel;
    (void)name;
    std::lock_guard<std::mutex> lock(_mutex);
    return _centerFrequency;
}

std::vector<std::string> AaroniaSoapyDevice::listFrequencies(const int direction, const size_t channel) const {
    (void)direction;
    (void)channel;
    std::vector<std::string> names;
    names.push_back("RF");
    return names;
}

SoapySDR::RangeList AaroniaSoapyDevice::getFrequencyRange(const int direction, const size_t channel, const std::string &name) const {
    (void)direction;
    (void)channel;
    (void)name;
    SoapySDR::RangeList ranges;
    ranges.push_back(SoapySDR::Range(10.0, 6.0e9)); // 10 Hz to 6 GHz (Spectran V6 range)
    return ranges;
}

void AaroniaSoapyDevice::setSampleRate(const int direction, const size_t channel, const double rate) {
    (void)channel;
    std::lock_guard<std::mutex> lock(_mutex);

    if (direction == SOAPY_SDR_TX) {
        // The TX rate feeds burst-duration computation in writeStream;
        // silently dropping it (old behaviour) derived TX timing from
        // the RX rate.
        _txSampleRate = rate;
        return;
    }
    if (direction != SOAPY_SDR_RX) return;

    AaroniaFfiError err = aaronia_source_set_span_frequency(_source, rate);
    if (err == Success) {
        _sampleRate = rate;
    } else {
        SoapySDR::logf(SOAPY_SDR_ERROR, "setSampleRate failed: %s",
                       lastErrorOr("unknown error").c_str());
    }
}

double AaroniaSoapyDevice::getSampleRate(const int direction, const size_t channel) const {
    (void)channel;
    std::lock_guard<std::mutex> lock(_mutex);
    if (direction == SOAPY_SDR_TX) {
        return _txSampleRate > 0.0 ? _txSampleRate : _sampleRate;
    }
    return _sampleRate;
}

SoapySDR::RangeList AaroniaSoapyDevice::getSampleRateRange(const int direction, const size_t channel) const {
    (void)direction;
    (void)channel;
    SoapySDR::RangeList ranges;
    // Capped at the IQ-mode constraint (span * 1.5 <= 92.16 MHz clock):
    // the old 92 MHz upper bound advertised rates the crate itself
    // rejects at construction/retune time.
    ranges.push_back(SoapySDR::Range(10e3, 61.44e6));
    return ranges;
}

std::vector<double> AaroniaSoapyDevice::listSampleRates(const int direction, const size_t channel) const {
    (void)direction;
    (void)channel;
    // Discrete suggestions for apps that build rate dropdowns from
    // listSampleRates (SDR++ shows an empty list without this). The
    // ladder mirrors the V6 decimation steps below the IQ-mode cap;
    // arbitrary rates within getSampleRateRange also work over HTTP.
    return {
        250e3, 500e3, 1e6, 2e6, 5e6, 10e6, 15.36e6, 20e6, 30.72e6, 61.44e6,
    };
}

size_t AaroniaSoapyDevice::getStreamMTU(SoapySDR::Stream *stream) const {
    (void)stream;
    // Matches the HTTP reader's chunking; larger requests are served
    // by looping internally.
    return 65536;
}

std::vector<std::string> AaroniaSoapyDevice::listGains(const int direction, const size_t channel) const {
    (void)direction;
    (void)channel;
    std::vector<std::string> gains;
    gains.push_back("REF"); // Reference level
    return gains;
}

void AaroniaSoapyDevice::setGain(const int direction, const size_t channel, const std::string &name, const double value) {
    (void)channel;
    (void)name;
    if (direction != SOAPY_SDR_RX) return;
    std::lock_guard<std::mutex> lock(_mutex);

    // NOTE: "gain" here is the Aaronia *reference level* in dBm, not an
    // amplifier gain: RAISING it reduces sensitivity. Exposed under the
    // name "REF" so applications' generic gain sliders at least carry
    // the correct label.
    AaroniaFfiError err = aaronia_source_set_reference_level(_source, value);
    if (err == Success) {
        _referenceLevel = value;
    } else {
        SoapySDR::logf(SOAPY_SDR_ERROR, "setGain(REF) failed: %s",
                       lastErrorOr("unknown error").c_str());
    }
}

double AaroniaSoapyDevice::getGain(const int direction, const size_t channel, const std::string &name) const {
    (void)direction;
    (void)channel;
    (void)name;
    std::lock_guard<std::mutex> lock(_mutex);
    return _referenceLevel;
}

SoapySDR::Range AaroniaSoapyDevice::getGainRange(const int direction, const size_t channel, const std::string &name) const {
    (void)direction;
    (void)channel;
    (void)name;
    return SoapySDR::Range(-100.0, 10.0); // Reference level, dBm
}

std::vector<std::string> AaroniaSoapyDevice::listSensors(void) const {
    std::vector<std::string> sensors;
    sensors.push_back("cumulative_drops");
    return sensors;
}

SoapySDR::ArgInfo AaroniaSoapyDevice::getSensorInfo(const std::string &name) const {
    SoapySDR::ArgInfo info;
    if (name == "cumulative_drops") {
        info.key = "cumulative_drops";
        info.name = "Cumulative Drops";
        info.type = SoapySDR::ArgInfo::INT;
        info.description = "Total number of packet drops detected in the streaming connection";
    }
    return info;
}

std::string AaroniaSoapyDevice::readSensor(const std::string &name) const {
    if (name == "cumulative_drops") {
        std::lock_guard<std::mutex> lock(_mutex);
        uint64_t drops = aaronia_source_get_cumulative_drops(_source);
        return std::to_string(drops);
    }
    return "";
}
