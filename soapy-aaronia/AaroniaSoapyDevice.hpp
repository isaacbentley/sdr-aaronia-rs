#ifndef AARONIA_SOAPY_DEVICE_HPP
#define AARONIA_SOAPY_DEVICE_HPP

#include <SoapySDR/Device.hpp>
#include <SoapySDR/Logger.hpp>
#include <SoapySDR/Formats.hpp>
#include "../include/aaronia.h"

#include <mutex>
#include <string>
#include <vector>

class AaroniaSoapyDevice : public SoapySDR::Device {
public:
    AaroniaSoapyDevice(AaroniaSource* source, AaroniaSink* sink, const SoapySDR::Kwargs &args);
    ~AaroniaSoapyDevice() override;

    // Identification API
    std::string getDriverKey() const override;
    std::string getHardwareKey() const override;
    SoapySDR::Kwargs getHardwareInfo() const override;

    // Channels API
    size_t getNumChannels(const int direction) const override;

    // Stream API
    std::vector<std::string> getStreamFormats(const int direction, const size_t channel) const override;
    std::string getNativeStreamFormat(const int direction, const size_t channel, double &fullScale) const override;
    
    SoapySDR::Stream *setupStream(
        const int direction,
        const std::string &format,
        const std::vector<size_t> &channels = std::vector<size_t>(),
        const SoapySDR::Kwargs &args = SoapySDR::Kwargs()) override;
        
    void closeStream(SoapySDR::Stream *stream) override;
    
    int activateStream(
        SoapySDR::Stream *stream,
        const int flags = 0,
        const long long timeNs = 0,
        const size_t numElems = 0) override;
        
    int deactivateStream(
        SoapySDR::Stream *stream,
        const int flags = 0,
        const long long timeNs = 0) override;
        
    int readStream(
        SoapySDR::Stream *stream,
        void * const *buffs,
        const size_t numElems,
        int &flags,
        long long &timeNs,
        const long timeoutUs = 100000) override;

    int writeStream(
        SoapySDR::Stream *stream,
        const void * const *buffs,
        const size_t numElems,
        int &flags,
        const long long timeNs = 0,
        const long timeoutUs = 100000) override;

    // Time API
    bool hasHardwareTime(const std::string &what = "") const override;
    long long getHardwareTime(const std::string &what = "") const override;

    // Clocking API
    std::vector<std::string> listClockSources(void) const override;
    void setClockSource(const std::string &source) override;
    std::string getClockSource(void) const override;

    // Antenna API
    std::vector<std::string> listAntennas(const int direction, const size_t channel) const override;
    void setAntenna(const int direction, const size_t channel, const std::string &name) override;
    std::string getAntenna(const int direction, const size_t channel) const override;

    // Frequency API
    void setFrequency(const int direction, const size_t channel, const std::string &name, const double frequency, const SoapySDR::Kwargs &args = SoapySDR::Kwargs()) override;
    double getFrequency(const int direction, const size_t channel, const std::string &name) const override;
    std::vector<std::string> listFrequencies(const int direction, const size_t channel) const override;
    SoapySDR::RangeList getFrequencyRange(const int direction, const size_t channel, const std::string &name) const override;

    // Sample Rate API
    void setSampleRate(const int direction, const size_t channel, const double rate) override;
    double getSampleRate(const int direction, const size_t channel) const override;
    SoapySDR::RangeList getSampleRateRange(const int direction, const size_t channel) const override;
    std::vector<double> listSampleRates(const int direction, const size_t channel) const override;

    // Stream geometry
    size_t getStreamMTU(SoapySDR::Stream *stream) const override;

    // Gain API
    std::vector<std::string> listGains(const int direction, const size_t channel) const override;
    void setGain(const int direction, const size_t channel, const std::string &name, const double value) override;
    double getGain(const int direction, const size_t channel, const std::string &name) const override;
    SoapySDR::Range getGainRange(const int direction, const size_t channel, const std::string &name) const override;

    // Sensor API
    std::vector<std::string> listSensors(void) const override;
    SoapySDR::ArgInfo getSensorInfo(const std::string &name) const override;
    std::string readSensor(const std::string &name) const override;

private:
    AaroniaSource *_source;
    AaroniaSink *_sink;
    // One mutex serializes every FFI call into the Rust objects: the C
    // ABI materializes `&mut` references, so concurrent calls from a
    // GUI thread (retune) and the streaming thread (readStream) would
    // be undefined behaviour. readStream/writeStream/getHardwareTime/
    // readSensor must take this lock too, not only the setters.
    mutable std::mutex _mutex;
    double _centerFrequency;
    double _sampleRate;    // RX sample rate
    double _txSampleRate;  // TX sample rate (defaults to RX rate)
    double _referenceLevel;
    // Per-direction stream state. RX and TX streams are distinct
    // handles (&_rxStreamTag / &_txStreamTag) with their own formats —
    // a single shared format let an RX CS16 + TX CF32 app corrupt its
    // own buffers when the second setupStream overwrote the first.
    std::string _rxFormat;
    std::string _txFormat;
    bool _rxSetup;
    bool _txSetup;
    int _rxStreamTag;
    int _txStreamTag;
    bool _isStreaming;
    std::vector<FfiComplex> _tempFloatBuffer;
};

#endif // AARONIA_SOAPY_DEVICE_HPP
