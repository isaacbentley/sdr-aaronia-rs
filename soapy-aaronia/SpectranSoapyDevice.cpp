#include "SpectranSoapyDevice.hpp"
#include <SoapySDR/Logger.hpp>
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <stdexcept>

// Fetch-and-free the thread-local Rust error string.
static std::string lastErrorOr(const char *fallback) {
    char *msg = spectran_last_error();
    std::string out = msg ? msg : fallback;
    if (msg) spectran_string_free(msg);
    return out;
}

SpectranSoapyDevice::SpectranSoapyDevice(SpectranSource* source, SpectranSink* sink, const SoapySDR::Kwargs &args)
    : _source(source), _sink(sink), _centerFrequency(100e6), _sampleRate(1e6),
      _txSampleRate(0.0), _referenceLevel(-20.0),
      _rxSetup(false), _txSetup(false), _rxStreamTag(0), _txStreamTag(0),
      _isStreaming(false),
      _hasFreqRange(false), _freqMinHz(0.0), _freqMaxHz(0.0), _freqStepHz(0.0),
      _hasRefRange(false), _refMinDbm(0.0), _refMaxDbm(0.0), _refStepDb(0.0),
      _sourceType(Http)
{
    if (!_source) {
        throw std::runtime_error("SpectranSoapyDevice initialized with null source pointer");
    }

    // The URL the source streams from, for the standalone sensor client
    // built lazily on first sensor read (HTTP backend only). Defaults to
    // the RTSA server's own default when the caller named none.
    if (args.count("url") != 0) {
        _httpUrl = args.at("url");
    } else if (args.count("file") == 0) {
        _httpUrl = "http://localhost:54664";
    }

    FfiSourceInfo* info = spectran_source_get_source_info(_source);
    if (info) {
        _centerFrequency = info->center_frequency_hz;
        _sampleRate = info->sample_rate_hz;
        _referenceLevel = info->reference_level_dbm;
        _sourceType = info->source_type;
        spectran_source_info_free(info);
    }

    // Ask the device what it can do, once, before anything is opened —
    // a probe queries ranges without ever streaming. Everything here is
    // best-effort: a null result, or any field the device did not
    // report, leaves the corresponding getter on its own constant.
    if (FfiDeviceCapabilities* caps = spectran_source_get_capabilities(_source)) {
        if (caps->model)   _model   = caps->model;
        if (caps->serial)  _serial  = caps->serial;
        if (caps->version) _version = caps->version;

        _hasFreqRange = caps->has_center_frequency;
        _freqMinHz    = caps->center_frequency_min_hz;
        _freqMaxHz    = caps->center_frequency_max_hz;
        _freqStepHz   = caps->center_frequency_step_hz;

        _hasRefRange  = caps->has_reference_level;
        _refMinDbm    = caps->reference_level_min_dbm;
        _refMaxDbm    = caps->reference_level_max_dbm;
        _refStepDb    = caps->reference_level_step_db;

        if (caps->sample_rates && caps->sample_rate_count > 0) {
            _sampleRates.assign(caps->sample_rates,
                                caps->sample_rates + caps->sample_rate_count);
        }
        if (caps->clock_sources) {
            for (size_t i = 0; i < caps->clock_source_count; ++i) {
                if (caps->clock_sources[i]) _clockSources.emplace_back(caps->clock_sources[i]);
            }
        }
        if (caps->clock_source) _clockSource = caps->clock_source;
        if (caps->rx_antenna)   _rxAntenna   = caps->rx_antenna;
        spectran_source_capabilities_free(caps);

        SoapySDR::logf(SOAPY_SDR_INFO,
                       "aaronia: device reports %s%s%s, %zu sample rates",
                       _model.empty() ? "no model" : _model.c_str(),
                       _serial.empty() ? "" : " serial ",
                       _serial.empty() ? "" : _serial.c_str(),
                       _sampleRates.size());
    } else {
        SoapySDR::logf(SOAPY_SDR_INFO,
                       "aaronia: device did not report capabilities (%s); "
                       "advertising this driver's defaults",
                       lastErrorOr("no detail").c_str());
    }
}

SpectranSoapyDevice::~SpectranSoapyDevice() {
    // Two separate lock scopes, `_mutex` released before `_sensorMutex` is
    // taken: no live code path takes them in the other order, and keeping
    // them un-nested here means none can deadlock the destructor if one
    // ever does.
    {
        std::lock_guard<std::mutex> lock(_mutex);
        if (_source) {
            if (_isStreaming) {
                spectran_source_stop_streaming(_source);
            }
            spectran_source_free(_source);
            _source = nullptr;
        }
        if (_sink) {
            // Stop unconditionally: the sink's stream lifecycle (started
            // at setupStream) is independent of the RX _isStreaming flag.
            spectran_sink_stop_streaming(_sink);
            spectran_sink_free(_sink);
            _sink = nullptr;
        }
    }
    {
        std::lock_guard<std::mutex> slock(_sensorMutex);
        if (_sensorClient) {
            spectran_endpoints_client_free(_sensorClient);
            _sensorClient = nullptr;
        }
    }
}

std::string SpectranSoapyDevice::getDriverKey() const {
    return "Aaronia";
}

std::string SpectranSoapyDevice::getHardwareKey() const {
    // The device's own name for itself. "Spectran V6" was hardcoded, so
    // every model reported as the base V6 — a V6 ECO included, which is
    // a different instrument with a different frequency range and no
    // transmitter. The constant remains only for a backend that cannot
    // be asked (file playback, native SDK).
    return _model.empty() ? std::string("Spectran V6") : _model;
}

SoapySDR::Kwargs SpectranSoapyDevice::getHardwareInfo() const {
    SoapySDR::Kwargs info;
    info["driver"] = "Aaronia";
    info["hardware"] = getHardwareKey();
    // Only what the device actually reported: an empty value would show
    // in --probe as a key the device answered with nothing.
    if (!_serial.empty())  info["serial"] = _serial;
    if (!_version.empty()) info["version"] = _version;
    return info;
}

size_t SpectranSoapyDevice::getNumChannels(const int direction) const {
    if (direction == SOAPY_SDR_RX) return 1;
    // TX exists only when a sink backend was constructed (native-sdk
    // builds on Windows/Linux). Advertising a TX channel that every
    // write rejects (the old behaviour) breaks apps at stream time
    // instead of letting them see there is no TX.
    if (direction == SOAPY_SDR_TX) return _sink ? 1 : 0;
    return 0;
}

std::vector<std::string> SpectranSoapyDevice::getStreamFormats(const int direction, const size_t channel) const {
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

std::string SpectranSoapyDevice::getNativeStreamFormat(const int direction, const size_t channel, double &fullScale) const {
    (void)direction;
    (void)channel;
    // The C ABI transfers CF32 in both directions; CS16 is a
    // client-side conversion in this plugin, not the wire/native
    // format (an earlier revision claimed CS16-native, steering apps
    // toward the *more* expensive path).
    fullScale = 1.0;
    return SOAPY_SDR_CF32;
}

SoapySDR::Stream *SpectranSoapyDevice::setupStream(
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
    SpectranFfiError err = spectran_sink_initialize(_sink);
    if (err != Success) {
        throw std::runtime_error(lastErrorOr("Failed to initialize sink"));
    }
    _txFormat = format;
    _txSetup = true;
    return reinterpret_cast<SoapySDR::Stream *>(&_txStreamTag);
}

void SpectranSoapyDevice::closeStream(SoapySDR::Stream *stream) {
    std::lock_guard<std::mutex> lock(_mutex);
    if (stream == reinterpret_cast<SoapySDR::Stream *>(&_rxStreamTag)) {
        _rxSetup = false;
    } else if (stream == reinterpret_cast<SoapySDR::Stream *>(&_txStreamTag)) {
        if (_sink) spectran_sink_stop_streaming(_sink);
        _txSetup = false;
    }
}

int SpectranSoapyDevice::activateStream(
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
    SpectranFfiError err = spectran_source_start_streaming(_source);
    if (err != Success) {
        SoapySDR::logf(SOAPY_SDR_ERROR, "activateStream failed: %s",
                       lastErrorOr("Failed to start streaming").c_str());
        return SOAPY_SDR_STREAM_ERROR;
    }
    _isStreaming = true;
    return 0;
}

int SpectranSoapyDevice::deactivateStream(
    SoapySDR::Stream *stream,
    const int flags,
    const long long timeNs)
{
    (void)flags;
    (void)timeNs;
    std::lock_guard<std::mutex> lock(_mutex);

    if (stream == reinterpret_cast<SoapySDR::Stream *>(&_txStreamTag)) {
        if (_sink) spectran_sink_stop_streaming(_sink);
        return 0;
    }
    if (stream != reinterpret_cast<SoapySDR::Stream *>(&_rxStreamTag)) {
        return SOAPY_SDR_STREAM_ERROR;
    }
    if (!_isStreaming) return 0;
    spectran_source_stop_streaming(_source);
    _isStreaming = false;
    return 0;
}

int SpectranSoapyDevice::readStream(
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
        read = spectran_source_read_samples_timeout(_source, out, numElems, timeout_us);
    } else if (_rxFormat == SOAPY_SDR_CS16) {
        if (_tempFloatBuffer.size() < numElems) {
            _tempFloatBuffer.resize(numElems);
        }
        read = spectran_source_read_samples_timeout(_source, _tempFloatBuffer.data(), numElems, timeout_us);
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
        if (spectran_source_take_overrun(_source)) {
            flags |= SOAPY_SDR_END_ABRUPT;
        }
        // Timestamp of the most recently received network block — an
        // approximation for the first returned sample when older
        // buffered samples are included (HTTP backend only; 0 when
        // unavailable, in which case HAS_TIME stays unset).
        timeNs = spectran_source_get_last_timestamp_ns(_source);
        if (timeNs != 0) flags |= SOAPY_SDR_HAS_TIME;
    }

    return static_cast<int>(read);
}

int SpectranSoapyDevice::writeStream(
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

    SpectranFfiError err = spectran_sink_write_samples(
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
bool SpectranSoapyDevice::hasHardwareTime(const std::string &what) const {
    if (what == "GPS") {
        // Truthful probe: GPS time exists only on the native-SDK
        // backend with a valid fix. Answering "true" unconditionally
        // (the old behaviour) made apps timestamp data to 1970.
        std::lock_guard<std::mutex> lock(_mutex);
        return spectran_source_get_gps_time_ns(_source, nullptr);
    }
    if (what.empty()) {
        // A capability, not a current value. Every RTSA packet header
        // carries startTime/endTime, so this device timestamps its
        // stream on every backend — unlike GPS above, where a fix may
        // genuinely not exist, which is what the value test belongs to.
        //
        // Answering with the last timestamp instead reported the
        // capability as absent until a packet had arrived, so
        // `SoapySDRUtil --probe` — which never streams — printed
        // "Timestamps: NO" for a device whose every buffer comes back
        // flagged SOAPY_SDR_HAS_TIME. An application choosing at setup
        // whether to record timestamps read that as "cannot".
        //
        // Whether a timestamp is available *yet* is a separate question
        // and readStream already answers it per buffer, which is the
        // signal that governs the data an application actually stamps.
        //
        // HTTP only: the timestamp is read from the RTSA packet header,
        // and the file and native-SDK sources never set one — answering
        // true for them is the 1970 bug again, on two backends.
        return _sourceType == Http;
    }
    return false;
}

long long SpectranSoapyDevice::getHardwareTime(const std::string &what) const {
    std::lock_guard<std::mutex> lock(_mutex);
    if (what == "GPS") {
        // Nanoseconds straight from the ABI. This used to receive
        // seconds as a double and split whole from fractional before
        // scaling, because (s * 1e9) lands where a double's step is
        // 256 ns. That arithmetic now lives in the library, where every
        // caller gets it rather than only this one.
        // int64_t, not long long: on LP64 Linux int64_t is `long`, and
        // `long long*` will not convert to `int64_t*` in C++. macOS makes
        // them the same type, so the mismatch only shows up on the Linux
        // build.
        int64_t gps_time_ns = 0;
        if (spectran_source_get_gps_time_ns(_source, &gps_time_ns)) {
            return gps_time_ns;
        }
        return 0;
    }
    // Default: the last stream timestamp, and `0` before any packet has
    // arrived. SoapySDR has no way to say "no time yet" here, so a
    // caller that must not stamp data to 1970 should take its time from
    // readStream's SOAPY_SDR_HAS_TIME buffers rather than polling this
    // before the stream is running.
    return spectran_source_get_last_timestamp_ns(_source);
}

// --- Clocking API ---
std::vector<std::string> SpectranSoapyDevice::listClockSources(void) const {
    // The device's own vocabulary, from `device/sclksource`: a V6 ECO
    // offers Consumer, Oscillator, GPS, PPS, 10MHz and three
    // "... Provider" variants. "Internal" was not among them — it was
    // this driver's invention, and it was reported on a device running
    // off an external 10 MHz house reference.
    std::lock_guard<std::mutex> lock(_mutex);
    if (!_clockSources.empty()) return _clockSources;
    return {"Internal"};
}

void SpectranSoapyDevice::setClockSource(const std::string &source) {
    std::lock_guard<std::mutex> lock(_mutex);

    // Refresh the cached list/selection first — an operator may have
    // changed it out of band — so a no-op write is recognised and any
    // warning names the real current source.
    if (FfiDeviceCapabilities* caps = spectran_source_get_capabilities(_source)) {
        if (caps->clock_source) _clockSource = caps->clock_source;
        if (caps->clock_sources) {
            _clockSources.clear();
            for (size_t i = 0; i < caps->clock_source_count; ++i) {
                if (caps->clock_sources[i]) _clockSources.emplace_back(caps->clock_sources[i]);
            }
        }
        spectran_source_capabilities_free(caps);
    }
    if (source == currentClockSourceLocked()) return;

    // `device/sclksource` is settable on both the native SDK and HTTP
    // backends; the crate reads the value back to confirm the write.
    SpectranFfiError err = spectran_source_set_clock_source(_source, source.c_str());
    if (err != Success) {
        SoapySDR::logf(SOAPY_SDR_ERROR, "setClockSource('%s') failed: %s",
                       source.c_str(), lastErrorOr("clock-source write failed").c_str());
        return;
    }
    _clockSource = source;
    SoapySDR::logf(SOAPY_SDR_INFO,
                   "setClockSource: stream clock source set to '%s'", source.c_str());
}

std::string SpectranSoapyDevice::currentClockSourceLocked(void) const {
    // Always a member of listClockSources(). When the device gave a
    // list but no valid selection, the first entry keeps the contract
    // rather than "Internal", which would be in neither list.
    if (!_clockSource.empty()) return _clockSource;
    if (!_clockSources.empty()) return _clockSources.front();
    return "Internal";
}

std::string SpectranSoapyDevice::getClockSource(void) const {
    // setClockSource refreshes the cache after construction, so reads
    // take the lock too.
    std::lock_guard<std::mutex> lock(_mutex);
    return currentClockSourceLocked();
}

std::vector<std::string> SpectranSoapyDevice::listAntennas(const int direction, const size_t channel) const {
    std::vector<std::string> ant;
    if (channel != 0) return ant;
    if (direction == SOAPY_SDR_RX) {
        // Named by the device's own `devicemode` — "RX1 LO1 SWEEP" on a
        // V6 ECO, where the mode is read-only. Hardcoding RX1 happened
        // to be right there and would misname a V6 running an RX2 mode.
        ant.push_back(_rxAntenna.empty() ? std::string("RX1") : _rxAntenna);
    } else if (direction == SOAPY_SDR_TX && _sink) {
        ant.push_back("TX1");
    }
    return ant;
}

void SpectranSoapyDevice::setAntenna(const int direction, const size_t channel, const std::string &name) {
    (void)direction;
    (void)channel;
    (void)name;
    // Single antenna per direction.
}

std::string SpectranSoapyDevice::getAntenna(const int direction, const size_t channel) const {
    (void)channel;
    // Must be a member of listAntennas(): applications select the
    // combo entry matching this, and gr-soapy validates set_antenna
    // against the list.
    if (direction == SOAPY_SDR_TX) return _sink ? "TX1" : "";
    return _rxAntenna.empty() ? std::string("RX1") : _rxAntenna;
}

void SpectranSoapyDevice::setFrequency(const int direction, const size_t channel, const std::string &name, const double frequency, const SoapySDR::Kwargs &args) {
    (void)channel;
    (void)name;
    (void)args;
    if (direction != SOAPY_SDR_RX) return;
    std::lock_guard<std::mutex> lock(_mutex);

    SpectranFfiError err = spectran_source_set_center_frequency_hz(_source, frequency);
    if (err == Success) {
        _centerFrequency = frequency;
    } else {
        SoapySDR::logf(SOAPY_SDR_ERROR, "setFrequency failed: %s",
                       lastErrorOr("unknown error").c_str());
    }
}

double SpectranSoapyDevice::getFrequency(const int direction, const size_t channel, const std::string &name) const {
    (void)direction;
    (void)channel;
    (void)name;
    std::lock_guard<std::mutex> lock(_mutex);
    return _centerFrequency;
}

std::vector<std::string> SpectranSoapyDevice::listFrequencies(const int direction, const size_t channel) const {
    (void)direction;
    (void)channel;
    std::vector<std::string> names;
    names.push_back("RF");
    return names;
}

SoapySDR::RangeList SpectranSoapyDevice::getFrequencyRange(const int direction, const size_t channel, const std::string &name) const {
    (void)direction;
    (void)channel;
    (void)name;
    SoapySDR::RangeList ranges;
    if (_hasFreqRange) {
        // As declared by centerfreq0. A V6 ECO says 5.5 MHz - 8 GHz,
        // where the constant below claimed 10 Hz - 6 GHz: too low at one
        // end for any tune to succeed, and short at the other of two
        // whole GHz the device can reach.
        ranges.push_back(SoapySDR::Range(_freqMinHz, _freqMaxHz, _freqStepHz));
    } else {
        ranges.push_back(SoapySDR::Range(10.0, 6.0e9));
    }
    return ranges;
}

void SpectranSoapyDevice::setSampleRate(const int direction, const size_t channel, const double rate) {
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

    // Snap to a rate the device can actually produce. It decimates a
    // 61.44 MHz clock by powers of two and silently adjusts anything
    // else, so caching the requested value made getSampleRate report a
    // rate the hardware was not running: an application asking for
    // 10 MHz displayed 10 MHz while receiving 7.68.
    const double wanted = rate;
    const std::vector<double> supported = listSampleRates(SOAPY_SDR_RX, 0);
    double snapped = supported.front();
    for (const double candidate : supported) {
        if (std::fabs(candidate - wanted) < std::fabs(snapped - wanted)) {
            snapped = candidate;
        }
    }
    if (std::fabs(snapped - wanted) > 1.0) {
        SoapySDR::logf(SOAPY_SDR_WARNING,
                       "setSampleRate: %g Hz is not a supported rate; using %g Hz",
                       wanted, snapped);
    }

    SpectranFfiError err = spectran_source_set_sample_rate_hz(_source, snapped);
    if (err == Success) {
        _sampleRate = snapped;
    } else {
        SoapySDR::logf(SOAPY_SDR_ERROR, "setSampleRate failed: %s",
                       lastErrorOr("unknown error").c_str());
    }
}

double SpectranSoapyDevice::getSampleRate(const int direction, const size_t channel) const {
    (void)channel;
    std::lock_guard<std::mutex> lock(_mutex);
    if (direction == SOAPY_SDR_TX) {
        return _txSampleRate > 0.0 ? _txSampleRate : _sampleRate;
    }
    // While streaming, the crate reports the rate the server states in
    // its packet metadata, which is authoritative. Fall back to the
    // snapped request before the first packet arrives.
    if (_isStreaming && _source) {
        if (FfiSourceInfo *info = spectran_source_get_source_info(_source)) {
            const double actual = info->sample_rate_hz;
            spectran_source_info_free(info);
            if (actual > 0.0) return actual;
        }
    }
    return _sampleRate;
}

SoapySDR::RangeList SpectranSoapyDevice::getSampleRateRange(const int direction, const size_t channel) const {
    (void)direction;
    (void)channel;
    SoapySDR::RangeList ranges;
    // One zero-width Range per rung: the ladder is discrete, and a
    // single span from floor to ceiling advertised 5 MHz as valid only
    // for setSampleRate to snap it to 3.84 with a warning. This is what
    // the RangeList form of the API exists for.
    for (const double rate : listSampleRates(SOAPY_SDR_RX, 0)) {
        ranges.push_back(SoapySDR::Range(rate, rate));
    }
    return ranges;
}

std::vector<double> SpectranSoapyDevice::listSampleRates(const int direction, const size_t channel) const {
    (void)direction;
    (void)channel;
    // Applications build their rate dropdowns from this list, so it has
    // to be rates the hardware can actually produce. It once advertised
    // round numbers (1, 2, 5, 10, 20 MHz) the device cannot produce:
    // requesting one silently ran it at a neighbouring rate while the
    // application displayed the rate it asked for.
    //
    // Preferred source is the device: its native undecimated rate,
    // snapped to an exact rung because the reported figure is a running
    // measurement, halved once per rung its decimation enum offers. That
    // is right for a model whose ladder is not the ECO's — a full V6 on
    // a faster receiver clock tops out at 163.84 MHz, not 61.44.
    if (!_sampleRates.empty()) {
        return _sampleRates;
    }
    // Fallback: the V6 ECO ladder, verified against its decimation enum.
    return {
        61.44e6,    // Full
        30.72e6,    // 1 / 2
        15.36e6,    // 1 / 4
        7.68e6,     // 1 / 8
        3.84e6,     // 1 / 16
        1.92e6,     // 1 / 32
        960e3,      // 1 / 64
        480e3,      // 1 / 128
        240e3,      // 1 / 256
        120e3,      // 1 / 512
    };
}

size_t SpectranSoapyDevice::getStreamMTU(SoapySDR::Stream *stream) const {
    (void)stream;
    // Matches the HTTP reader's chunking; larger requests are served
    // by looping internally.
    return 65536;
}

std::vector<std::string> SpectranSoapyDevice::listGains(const int direction, const size_t channel) const {
    (void)direction;
    (void)channel;
    std::vector<std::string> gains;
    gains.push_back("REF"); // Reference level
    return gains;
}

void SpectranSoapyDevice::setGain(const int direction, const size_t channel, const std::string &name, const double value) {
    (void)channel;
    (void)name;
    if (direction != SOAPY_SDR_RX) return;
    std::lock_guard<std::mutex> lock(_mutex);

    // NOTE: "gain" here is the Aaronia *reference level* in dBm, not an
    // amplifier gain: RAISING it reduces sensitivity. Exposed under the
    // name "REF" so applications' generic gain sliders at least carry
    // the correct label.
    SpectranFfiError err = spectran_source_set_reference_level_dbm(_source, value);
    if (err == Success) {
        _referenceLevel = value;
    } else {
        SoapySDR::logf(SOAPY_SDR_ERROR, "setGain(REF) failed: %s",
                       lastErrorOr("unknown error").c_str());
    }
}

double SpectranSoapyDevice::getGain(const int direction, const size_t channel, const std::string &name) const {
    (void)direction;
    (void)channel;
    (void)name;
    std::lock_guard<std::mutex> lock(_mutex);
    return _referenceLevel;
}

SoapySDR::Range SpectranSoapyDevice::getGainRange(const int direction, const size_t channel, const std::string &name) const {
    (void)direction;
    (void)channel;
    (void)name;
    // Reference level in dBm, as reflevel0 declares it. A V6 ECO says
    // -55 to +23; the constant said -100 to +10, so an application's
    // gain slider spanned values the device silently clamped at one end
    // and stopped 13 dB short of usable headroom at the other.
    if (_hasRefRange) {
        return SoapySDR::Range(_refMinDbm, _refMaxDbm, _refStepDb);
    }
    return SoapySDR::Range(-100.0, 10.0);
}

// ---------------------------------------------------------------------
// Bandwidth API
//
// The device's alias-free bandwidth is narrower than its sample rate;
// SoapySDR keeps the two as separate knobs. `setBandwidth` maps the
// requested bandwidth to the sample rate that delivers it and drives the
// already-verified `setSampleRate`, so no new device-write path appears.
// ---------------------------------------------------------------------

void SpectranSoapyDevice::setBandwidth(const int direction, const size_t channel, const double bw) {
    if (bw <= 0.0) return;
    setSampleRate(direction, channel, spectran_iq_sample_rate_for_bandwidth(bw));
}

double SpectranSoapyDevice::getBandwidth(const int direction, const size_t channel) const {
    return spectran_usable_bandwidth_hz(getSampleRate(direction, channel));
}

std::vector<double> SpectranSoapyDevice::listBandwidths(const int direction, const size_t channel) const {
    std::vector<double> bandwidths;
    for (const double rate : listSampleRates(direction, channel)) {
        bandwidths.push_back(spectran_usable_bandwidth_hz(rate));
    }
    return bandwidths;
}

SoapySDR::RangeList SpectranSoapyDevice::getBandwidthRange(const int direction, const size_t channel) const {
    SoapySDR::RangeList ranges;
    for (const double bw : listBandwidths(direction, channel)) {
        ranges.push_back(SoapySDR::Range(bw, bw));
    }
    return ranges;
}

// ---------------------------------------------------------------------
// Sensor API
// ---------------------------------------------------------------------

namespace {

// The device's live sensors, each a field of FfiDeviceSensors. NaN means
// the device did not report it, and such a sensor is not listed.
struct SensorDef {
    const char *key;
    const char *name;
    const char *units;
    const char *description;
    double FfiDeviceSensors::*field;
};

const SensorDef kSensorDefs[] = {
    {"fpga_temp", "FPGA Temperature", "C", "FPGA die temperature", &FfiDeviceSensors::fpga_temp_c},
    {"frontend_temp", "Frontend Temperature", "C", "RF frontend temperature", &FfiDeviceSensors::frontend_temp_c},
    {"board_power", "Board Power", "W", "Board power draw", &FfiDeviceSensors::board_power_w},
    {"adc_range", "ADC Range", "dB", "ADC headroom below full scale; near zero is close to clipping", &FfiDeviceSensors::adc_range_db},
    {"usb_buffer", "USB Buffer Fill", "", "USB transfer buffer fill, fraction 0-1", &FfiDeviceSensors::usb_buffer_fill},
    {"dsp_buffer", "DSP Buffer Fill", "", "DSP buffer fill, fraction 0-1", &FfiDeviceSensors::dsp_buffer_fill},
    {"errors", "Errors/s", "", "Device errors per second", &FfiDeviceSensors::errors_per_second},
    {"usb_overflows", "USB Overflows/s", "", "USB overflows per second", &FfiDeviceSensors::usb_overflows_per_second},
    {"dsp_overflows", "DSP Overflows/s", "", "DSP overflows per second", &FfiDeviceSensors::dsp_overflows_per_second},
    {"gps_satellites", "GPS Satellites", "", "GPS satellites in view", &FfiDeviceSensors::gps_satellites},
    {"gps_latitude", "GPS Latitude", "deg", "GPS latitude; 0 with no fix", &FfiDeviceSensors::gps_latitude},
    {"gps_longitude", "GPS Longitude", "deg", "GPS longitude; 0 with no fix", &FfiDeviceSensors::gps_longitude},
};

const SensorDef *findSensorDef(const std::string &key) {
    for (const auto &def : kSensorDefs) {
        if (key == def.key) return &def;
    }
    return nullptr;
}

std::string formatSensorValue(double v) {
    char buf[32];
    std::snprintf(buf, sizeof(buf), "%g", v);
    return std::string(buf);
}

} // namespace

bool SpectranSoapyDevice::refreshSensorsLocked(void) const {
    // The rich sensors come from /healthstatus, which only the HTTP
    // backend serves — the raw native SDK's own health tree reads all
    // zeros. No client means no reading, and the caller then offers just
    // cumulative_drops. The client is built once, lazily, so a capture
    // that never reads sensors pays nothing.
    if (_sourceType != Http || _httpUrl.empty()) {
        return false;
    }
    if (!_sensorClient) {
        _sensorClient = spectran_endpoints_client_new(_httpUrl.c_str());
        if (!_sensorClient) {
            return false;
        }
    }
    using namespace std::chrono;
    const auto now = steady_clock::now();
    if (_sensorsCacheValid && now - _sensorsCacheTime < milliseconds(250)) {
        return true;
    }
    if (spectran_endpoints_client_get_sensors(_sensorClient, &_sensorsCache)) {
        _sensorsCacheTime = now;
        _sensorsCacheValid = true;
        return true;
    }
    return false;
}

std::vector<std::string> SpectranSoapyDevice::listSensors(void) const {
    std::vector<std::string> sensors;
    // Always present: the client-side drop detector, which every backend
    // feeds and which needs no health fetch.
    sensors.push_back("cumulative_drops");
    std::lock_guard<std::mutex> lock(_sensorMutex);
    if (refreshSensorsLocked()) {
        for (const auto &def : kSensorDefs) {
            if (!std::isnan(_sensorsCache.*def.field)) {
                sensors.push_back(def.key);
            }
        }
    }
    return sensors;
}

SoapySDR::ArgInfo SpectranSoapyDevice::getSensorInfo(const std::string &name) const {
    SoapySDR::ArgInfo info;
    if (name == "cumulative_drops") {
        info.key = "cumulative_drops";
        info.name = "Cumulative Drops";
        info.type = SoapySDR::ArgInfo::INT;
        info.units = "";
        info.description = "Timestamp gaps the client's drop detector has seen in the stream";
        return info;
    }
    if (const SensorDef *def = findSensorDef(name)) {
        info.key = def->key;
        info.name = def->name;
        info.type = SoapySDR::ArgInfo::FLOAT;
        info.units = def->units;
        info.description = def->description;
    }
    return info;
}

std::string SpectranSoapyDevice::readSensor(const std::string &name) const {
    if (name == "cumulative_drops") {
        // A local counter read, not an HTTP fetch: brief enough to take
        // the streaming lock for.
        std::lock_guard<std::mutex> lock(_mutex);
        return std::to_string(spectran_source_get_cumulative_drops(_source));
    }
    if (const SensorDef *def = findSensorDef(name)) {
        // The health fetch stays off `_mutex` — see the member comment.
        std::lock_guard<std::mutex> lock(_sensorMutex);
        if (refreshSensorsLocked() && !std::isnan(_sensorsCache.*def->field)) {
            return formatSensorValue(_sensorsCache.*def->field);
        }
    }
    return "";
}
