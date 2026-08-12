#include "AaroniaSoapyDevice.hpp"
#include <SoapySDR/Registry.hpp>
#include <SoapySDR/Logger.hpp>

static std::vector<SoapySDR::Kwargs> findAaronia(const SoapySDR::Kwargs &args) {
    std::vector<SoapySDR::Kwargs> results;

    // If driver filter specified and it's not aaronia, skip
    if (args.count("driver") != 0 && args.at("driver") != "aaronia") {
        return results;
    }

    SoapySDR::Kwargs device;
    device["driver"] = "aaronia";
    device["label"] = "Aaronia Spectran V6 (sdr-aaronia-rs)";

    if (args.count("url") != 0) {
        device["url"] = args.at("url");
    } else if (args.count("file") != 0) {
        device["file"] = args.at("file");
    } else if (args.count("serial") != 0) {
        device["serial"] = args.at("serial");
    } else {
        // Default HTTP endpoint. NOTE: no reachability probe is
        // performed here — find() must not block on the network — so
        // this candidate may not correspond to a live server.
        device["url"] = "http://localhost:54664";
    }

    results.push_back(device);
    return results;
}

// RAII holders so a throw anywhere in makeAaronia (bad args, bad_alloc
// in `new AaroniaSoapyDevice`) cannot leak the Rust objects — the old
// code leaked the builder on a malformed "freq" and leaked source+sink
// on construction failure.
namespace {
struct SourceBuilderGuard {
    AaroniaSourceBuilder *p;
    explicit SourceBuilderGuard(AaroniaSourceBuilder *b) : p(b) {}
    ~SourceBuilderGuard() { if (p) aaronia_source_builder_free(p); }
};
struct SinkBuilderGuard {
    AaroniaSinkBuilder *p;
    explicit SinkBuilderGuard(AaroniaSinkBuilder *b) : p(b) {}
    ~SinkBuilderGuard() { if (p) aaronia_sink_builder_free(p); }
};
struct SourceGuard {
    AaroniaSource *p;
    explicit SourceGuard(AaroniaSource *s) : p(s) {}
    ~SourceGuard() { if (p) aaronia_source_free(p); }
    AaroniaSource *release() { AaroniaSource *out = p; p = nullptr; return out; }
};
struct SinkGuard {
    AaroniaSink *p;
    explicit SinkGuard(AaroniaSink *s) : p(s) {}
    ~SinkGuard() { if (p) aaronia_sink_free(p); }
    AaroniaSink *release() { AaroniaSink *out = p; p = nullptr; return out; }
};

double parseArgDouble(const SoapySDR::Kwargs &args, const char *key) {
    // Validate before any Rust allocation exists; std::stod throws
    // std::invalid_argument/out_of_range with an unhelpful message, so
    // wrap it with the offending key/value.
    try {
        return std::stod(args.at(key));
    } catch (const std::exception &) {
        throw std::runtime_error(std::string("invalid numeric value for device arg '")
                                 + key + "': " + args.at(key));
    }
}
} // namespace

static SoapySDR::Device *makeAaronia(const SoapySDR::Kwargs &args) {
    // Parse all numeric args up front (may throw; nothing to leak yet).
    const bool hasFreq = args.count("freq") != 0;
    const bool hasRate = args.count("rate") != 0;
    const double freq = hasFreq ? parseArgDouble(args, "freq") : 0.0;
    const double rate = hasRate ? parseArgDouble(args, "rate") : 0.0;

    AaroniaSourceBuilder* builder = aaronia_source_builder_new();
    if (!builder) {
        throw std::runtime_error("Failed to create AaroniaSourceBuilder");
    }
    SourceBuilderGuard builderGuard(builder);

    if (args.count("url") != 0) {
        aaronia_source_builder_http_source(builder, args.at("url").c_str());
    } else if (args.count("file") != 0) {
        aaronia_source_builder_file_source(builder, args.at("file").c_str());
    } else if (args.count("serial") == 0) {
        // Default to HTTP localhost only when the caller didn't select
        // a device by serial: a serial-only open should let the crate
        // auto-detect the native-SDK backend (the only one that can
        // honor a serial), which a forced HTTP URL made unreachable.
        aaronia_source_builder_http_source(builder, "http://localhost:54664");
    }

    // Honor the serial arg findAaronia advertises (native-SDK backend
    // device selection); the old code echoed it in find() and then
    // silently ignored it here.
    if (args.count("serial") != 0) {
        aaronia_source_builder_device_serial(builder, args.at("serial").c_str());
    }

    if (hasFreq) aaronia_source_builder_center_frequency(builder, freq);
    if (hasRate) aaronia_source_builder_span_frequency(builder, rate);
    if (args.count("ref_level") != 0) {
        aaronia_source_builder_reference_level(builder, parseArgDouble(args, "ref_level"));
    }
    // format=I16 enables the genuine low-bandwidth HTTP wire mode
    // (int16 from the server), optionally with scale=N.
    if (args.count("format") != 0) {
        aaronia_source_builder_stream_format(builder, args.at("format").c_str());
    }
    if (args.count("scale") != 0) {
        aaronia_source_builder_stream_scale(builder, parseArgDouble(args, "scale"));
    }
    // rx_channel=Rx1|Rx2|Rx1And2 (native-SDK backend only). The plugin
    // itself streams channel 0; Rx2 selects the second antenna input.
    if (args.count("rx_channel") != 0) {
        const std::string &ch = args.at("rx_channel");
        int32_t sel = ch == "Rx2" ? 1 : (ch == "Rx1And2" ? 2 : 0);
        aaronia_source_builder_receiver_channel(builder, sel);
    }
    // read_timeout=<seconds>: only affects the crate's own blocking
    // reads. readStream always passes SoapySDR's per-call timeoutUs, so
    // this is a backstop for the non-Soapy paths rather than a knob most
    // Soapy applications need.
    if (args.count("read_timeout") != 0) {
        const double seconds = parseArgDouble(args, "read_timeout");
        if (seconds > 0.0) {
            aaronia_source_builder_read_timeout_us(
                builder, static_cast<uint64_t>(seconds * 1e6));
        }
    }

    SourceGuard source(aaronia_source_build(builder));
    if (!source.p) {
        char* msg = aaronia_last_error();
        std::string err = msg ? msg : "Failed to build AaroniaSource";
        if (msg) aaronia_string_free(msg);
        throw std::runtime_error(err);
    }

    AaroniaSinkBuilder* sink_builder = aaronia_sink_builder_new();
    SinkBuilderGuard sinkBuilderGuard(sink_builder);
    SinkGuard sink(nullptr);
    if (sink_builder) {
        // TX shares the tuning args with RX unless retuned later.
        if (hasFreq) aaronia_sink_builder_center_frequency(sink_builder, freq);
        if (hasRate) aaronia_sink_builder_sample_rate(sink_builder, rate);
        sink.p = aaronia_sink_build(sink_builder);
    }

    // Device takes ownership of both on successful construction.
    AaroniaSoapyDevice *device = new AaroniaSoapyDevice(source.p, sink.p, args);
    source.release();
    sink.release();
    return device;
}

static SoapySDR::Registry registerAaronia("aaronia", &findAaronia, &makeAaronia, SOAPY_SDR_ABI_VERSION);
