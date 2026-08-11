#include "AaroniaSoapyDevice.hpp"
#include <SoapySDR/Logger.hpp>
#include <iostream>
#include <stdexcept>
#include <algorithm>

AaroniaSoapyDevice::AaroniaSoapyDevice(AaroniaSource* source, AaroniaSink* sink, const SoapySDR::Kwargs &args)
    : _source(source), _sink(sink), _centerFrequency(100e6), _sampleRate(1e6), _referenceLevel(-20.0), _isStreaming(false)
{
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
    if (_source) {
        if (_isStreaming) {
            aaronia_source_stop_streaming(_source);
        }
        aaronia_source_free(_source);
        _source = nullptr;
    }
    if (_sink) {
        if (_isStreaming) {
            aaronia_sink_stop_streaming(_sink);
        }
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
    return (direction == SOAPY_SDR_RX || direction == SOAPY_SDR_TX) ? 1 : 0;
}

std::vector<std::string> AaroniaSoapyDevice::getStreamFormats(const int direction, const size_t channel) const {
    std::vector<std::string> formats;
    if ((direction == SOAPY_SDR_RX || direction == SOAPY_SDR_TX) && channel == 0) {
        formats.push_back(SOAPY_SDR_CF32);
        formats.push_back(SOAPY_SDR_CS16);
    }
    return formats;
}

std::string AaroniaSoapyDevice::getNativeStreamFormat(const int direction, const size_t channel, double &fullScale) const {
    fullScale = 32767.0; // 16-bit signed integer max value
    return SOAPY_SDR_CS16;
}

SoapySDR::Stream *AaroniaSoapyDevice::setupStream(
    const int direction,
    const std::string &format,
    const std::vector<size_t> &channels,
    const SoapySDR::Kwargs &args)
{
    if (direction != SOAPY_SDR_RX && direction != SOAPY_SDR_TX) {
        throw std::runtime_error("Only RX and TX streams are supported");
    }
    if (!channels.empty() && channels[0] != 0) {
        throw std::runtime_error("Invalid channel");
    }

    if (format != SOAPY_SDR_CF32 && format != SOAPY_SDR_CS16) {
        throw std::runtime_error("Unsupported stream format: " + format);
    }

    _streamFormat = format;
    
    if (direction == SOAPY_SDR_TX) {
        if (!_sink) {
            throw std::runtime_error("TX sink not initialized");
        }
        AaroniaFfiError err = aaronia_sink_initialize(_sink);
        if (err != Success) {
            char* msg = aaronia_last_error();
            std::string errMsg = msg ? msg : "Failed to initialize sink";
            if (msg) aaronia_string_free(msg);
            throw std::runtime_error(errMsg);
        }
    }
    
    // SoapySDR allows returning an opaque pointer to represent the stream
    // Since we only support one stream at a time (TX or RX), we return this.
    return (SoapySDR::Stream *)this;
}

void AaroniaSoapyDevice::closeStream(SoapySDR::Stream *stream) {
    // No explicit teardown required beyond deactivateStream
}

int AaroniaSoapyDevice::activateStream(
    SoapySDR::Stream *stream,
    const int flags,
    const long long timeNs,
    const size_t numElems)
{
    std::lock_guard<std::mutex> lock(_mutex);
    if (_isStreaming) return 0;
    
    if (stream != (SoapySDR::Stream *)this) {
        // Technically it could be TX, but Aaronia doesn't have a strict start_streaming for sink except initialization.
    }
    
    AaroniaFfiError err = aaronia_source_start_streaming(_source);
    if (err != Success) {
        char* msg = aaronia_last_error();
        std::string errMsg = msg ? msg : "Failed to start streaming";
        if (msg) aaronia_string_free(msg);
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
    std::lock_guard<std::mutex> lock(_mutex);
    if (!_isStreaming) return 0;

    aaronia_source_stop_streaming(_source);
    if (_sink) {
        aaronia_sink_stop_streaming(_sink);
    }
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
    if (!buffs || !buffs[0]) return SOAPY_SDR_STREAM_ERROR;

    intptr_t read = -1;
    if (_streamFormat == SOAPY_SDR_CF32) {
        FfiComplex *out = static_cast<FfiComplex *>(buffs[0]);
        read = aaronia_source_read_samples(_source, out, numElems);
    } else if (_streamFormat == SOAPY_SDR_CS16) {
        // Read into temporary float buffer and scale to int16
        if (_tempFloatBuffer.size() < numElems) {
            _tempFloatBuffer.resize(numElems);
        }
        read = aaronia_source_read_samples(_source, _tempFloatBuffer.data(), numElems);
        if (read > 0) {
            int16_t *out = static_cast<int16_t *>(buffs[0]);
            for (intptr_t i = 0; i < read; ++i) {
                out[i * 2]     = static_cast<int16_t>(std::clamp(_tempFloatBuffer[i].re * 32767.0f, -32768.0f, 32767.0f));
                out[i * 2 + 1] = static_cast<int16_t>(std::clamp(_tempFloatBuffer[i].im * 32767.0f, -32768.0f, 32767.0f));
            }
        }
    } else {
        return SOAPY_SDR_STREAM_ERROR;
    }

    if (read < 0) {
        if (read == -3) return SOAPY_SDR_TIMEOUT;
        return SOAPY_SDR_STREAM_ERROR;
    }

    if (read > 0) {
        if (aaronia_source_take_overrun(_source)) {
            flags |= SOAPY_SDR_END_ABRUPT | SOAPY_SDR_OVERFLOW;
        }
        timeNs = aaronia_source_get_last_timestamp_ns(_source);
        flags |= SOAPY_SDR_HAS_TIME;
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
    if (!_sink) {
        return SOAPY_SDR_STREAM_ERROR;
    }

    double start_time_s = timeNs == 0 ? 0.0 : timeNs / 1e9;
    double end_time_s = start_time_s + (numElems / _sampleRate);

    AaroniaFfiError err = Success;
    if (_streamFormat == SOAPY_SDR_CF32) {
        err = aaronia_sink_write_samples(
            _sink,
            0,
            start_time_s,
            end_time_s,
            static_cast<const float _Complex*>(buffs[0]),
            numElems
        );
    } else {
        // Not implemented for CS16 yet on TX
        return SOAPY_SDR_STREAM_ERROR;
    }

    if (err != Success) {
        return SOAPY_SDR_STREAM_ERROR;
    }
    
    return (int)numElems;
}

std::vector<std::string> AaroniaSoapyDevice::listAntennas(const int direction, const size_t channel) const {
    std::vector<std::string> ant;
    if (direction == SOAPY_SDR_RX) {
        ant.push_back("RX1");
    }
    return ant;
}

void AaroniaSoapyDevice::setAntenna(const int direction, const size_t channel, const std::string &name) {
    // Single antenna default
}

std::string AaroniaSoapyDevice::getAntenna(const int direction, const size_t channel) const {
    return "RX1";
}

void AaroniaSoapyDevice::setFrequency(const int direction, const size_t channel, const std::string &name, const double frequency, const SoapySDR::Kwargs &args) {
    if (direction != SOAPY_SDR_RX) return;
    std::lock_guard<std::mutex> lock(_mutex);
    
    AaroniaFfiError err = aaronia_source_set_center_frequency(_source, frequency);
    if (err == Success) {
        _centerFrequency = frequency;
    } else {
        char* msg = aaronia_last_error();
        if (msg) {
            SoapySDR::logf(SOAPY_SDR_ERROR, "setFrequency failed: %s", msg);
            aaronia_string_free(msg);
        }
    }
}

double AaroniaSoapyDevice::getFrequency(const int direction, const size_t channel, const std::string &name) const {
    return _centerFrequency;
}

std::vector<std::string> AaroniaSoapyDevice::listFrequencies(const int direction, const size_t channel) const {
    std::vector<std::string> names;
    names.push_back("RF");
    return names;
}

SoapySDR::RangeList AaroniaSoapyDevice::getFrequencyRange(const int direction, const size_t channel, const std::string &name) const {
    SoapySDR::RangeList ranges;
    ranges.push_back(SoapySDR::Range(10.0, 6.0e9)); // 10 Hz to 6 GHz (Spectran V6 range)
    return ranges;
}

void AaroniaSoapyDevice::setSampleRate(const int direction, const size_t channel, const double rate) {
    if (direction != SOAPY_SDR_RX) return;
    std::lock_guard<std::mutex> lock(_mutex);
    
    AaroniaFfiError err = aaronia_source_set_span_frequency(_source, rate);
    if (err == Success) {
        _sampleRate = rate;
    } else {
        char* msg = aaronia_last_error();
        if (msg) {
            SoapySDR::logf(SOAPY_SDR_ERROR, "setSampleRate failed: %s", msg);
            aaronia_string_free(msg);
        }
    }
}

double AaroniaSoapyDevice::getSampleRate(const int direction, const size_t channel) const {
    return _sampleRate;
}

SoapySDR::RangeList AaroniaSoapyDevice::getSampleRateRange(const int direction, const size_t channel) const {
    SoapySDR::RangeList ranges;
    ranges.push_back(SoapySDR::Range(10e3, 92e6)); // 10 kHz to 92 MHz span
    return ranges;
}

std::vector<std::string> AaroniaSoapyDevice::listGains(const int direction, const size_t channel) const {
    std::vector<std::string> gains;
    gains.push_back("REF"); // Reference level
    return gains;
}

void AaroniaSoapyDevice::setGain(const int direction, const size_t channel, const std::string &name, const double value) {
    if (direction != SOAPY_SDR_RX) return;
    std::lock_guard<std::mutex> lock(_mutex);
    
    // In Aaronia hardware, gain is configured via Reference Level (dBm)
    AaroniaFfiError err = aaronia_source_set_reference_level(_source, value);
    if (err == Success) {
        _referenceLevel = value;
    }
}

double AaroniaSoapyDevice::getGain(const int direction, const size_t channel, const std::string &name) const {
    return _referenceLevel;
}

SoapySDR::Range AaroniaSoapyDevice::getGainRange(const int direction, const size_t channel, const std::string &name) const {
    return SoapySDR::Range(-100.0, 10.0); // -100 dBm to +10 dBm
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
        uint64_t drops = aaronia_source_get_cumulative_drops(_source);
        return std::to_string(drops);
    }
    return "";
}
