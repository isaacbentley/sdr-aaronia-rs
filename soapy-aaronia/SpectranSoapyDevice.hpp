#ifndef AARONIA_SOAPY_DEVICE_HPP
#define AARONIA_SOAPY_DEVICE_HPP

#include <SoapySDR/Device.hpp>
#include <SoapySDR/Logger.hpp>
#include <SoapySDR/Formats.hpp>
#include "../include/spectran.h"

#include <chrono>
#include <mutex>
#include <string>
#include <vector>

class SpectranSoapyDevice : public SoapySDR::Device {
public:
    SpectranSoapyDevice(SpectranSource* source, SpectranSink* sink, const SoapySDR::Kwargs &args);
    ~SpectranSoapyDevice() override;

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
    // The selected clock source with _mutex already held; shared by
    // getClockSource and setClockSource so neither re-enters the lock.
    std::string currentClockSourceLocked(void) const;
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

    // Bandwidth API. The RTSA's alias-free bandwidth is narrower than its
    // sample rate; these expose that as SoapySDR's separate bandwidth
    // knob, mapping through the already-verified sample-rate control.
    void setBandwidth(const int direction, const size_t channel, const double bw) override;
    double getBandwidth(const int direction, const size_t channel) const override;
    std::vector<double> listBandwidths(const int direction, const size_t channel) const override;
    SoapySDR::RangeList getBandwidthRange(const int direction, const size_t channel) const override;

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
    SpectranSource *_source;
    SpectranSink *_sink;
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

    // What the device said about itself, read once at construction.
    //
    // Cached rather than fetched per call because reading it is two
    // control-plane GETs, and SoapySDR asks for ranges repeatedly while
    // an application builds its UI. Empty strings and empty vectors
    // mean "the device did not say", and every getter falls back to its
    // own constant for that field alone — a device that answers about
    // frequency but not gain still gets its frequency range published.
    std::string _model;
    std::string _serial;
    std::string _version;
    bool _hasFreqRange;
    double _freqMinHz, _freqMaxHz, _freqStepHz;
    bool _hasRefRange;
    double _refMinDbm, _refMaxDbm, _refStepDb;
    std::vector<double> _sampleRates;
    std::vector<std::string> _clockSources;
    std::string _clockSource;
    std::string _rxAntenna;
    // Which backend the source resolved to. Gates the capabilities
    // that only one backend has: timestamps come from the HTTP
    // packet headers and nowhere else.
    CSpectranSourceType _sourceType;

    // Live sensors (HTTP backend only), read through a dedicated
    // endpoints client and guarded by their own `_sensorMutex` — never
    // `_mutex`. `_mutex` is what readStream holds, and the sensor fetch is
    // a blocking /healthstatus GET; sharing the lock would stall sample
    // delivery and risk drops. The client is created lazily on first use,
    // from `_httpUrl`. Over the native SDK there is no client — its raw
    // health tree reads all zeros — so only `cumulative_drops` (a fast
    // local read off `_source`, under `_mutex`) is offered there. Briefly
    // cached (250 ms) so a probe's burst of readSensor calls is one GET.
    // `mutable`: these const methods memoize.
    std::string _httpUrl;
    mutable std::mutex _sensorMutex;
    mutable HttpEndpointsClient *_sensorClient = nullptr;
    mutable FfiDeviceSensors _sensorsCache;
    mutable std::chrono::steady_clock::time_point _sensorsCacheTime;
    mutable bool _sensorsCacheValid = false;
    // Fetch the sensors if the cache is stale, and return whether a
    // reading is available. Caller must hold `_sensorMutex`.
    bool refreshSensorsLocked(void) const;
};

#endif // AARONIA_SOAPY_DEVICE_HPP
