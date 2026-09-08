#include "AaroniaSoapyDevice.hpp"
#include <SoapySDR/Registry.hpp>
#include <SoapySDR/Logger.hpp>

// Whether the args ask for the native SDK: a serial names an SDK device,
// and `sdk` must be truthy — `sdk=false` is a request *not* to use it, so
// checking mere presence would advertise/open the SDK for `sdk=false`.
static bool wantsNativeSdk(const SoapySDR::Kwargs &args) {
    if (args.count("serial") != 0) return true;
    auto it = args.find("sdk");
    return it != args.end() && (it->second == "true" || it->second == "1");
}

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
        results.push_back(device);
    } else if (args.count("file") != 0) {
        device["file"] = args.at("file");
        results.push_back(device);
    } else if (wantsNativeSdk(args)) {
        // An explicit native-SDK request: a serial names a device the SDK
        // enumerates, and sdk=true asks for it without naming one.
        device["sdk"] = "true";
        if (args.count("serial") != 0) device["serial"] = args.at("serial");
        device["label"] = "Aaronia Spectran V6 (sdr-aaronia-rs, native SDK)";
        results.push_back(device);
    } else {
        // Nothing chosen: the HTTP server's own default, as before. NOTE:
        // no reachability probe is performed here - find() must not block
        // on the network - so this candidate may not correspond to a live
        // server. When the native SDK is installed, advertise it too:
        // until it was, the only way to reach the SDK from Soapy was to
        // already know a serial.
        device["url"] = "http://localhost:54664";
        results.push_back(device);
        // Offer the SDK too when it is installed — but not when the
        // caller wrote `sdk=false`, which reached this branch precisely
        // because it does not want the SDK.
        if (aaronia_sdk_installed() && args.count("sdk") == 0) {
            SoapySDR::Kwargs sdk;
            sdk["driver"] = "aaronia";
            sdk["sdk"] = "true";
            sdk["label"] = "Aaronia Spectran V6 (sdr-aaronia-rs, native SDK)";
            results.push_back(sdk);
        }
    }

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

    const bool wantsSdk = wantsNativeSdk(args);
    if (args.count("url") != 0) {
        aaronia_source_builder_http_source(builder, args.at("url").c_str());
    } else if (args.count("file") != 0) {
        aaronia_source_builder_file_source(builder, args.at("file").c_str());
    } else if (wantsSdk) {
        // Pin the backend rather than leaving it to auto-detection: a
        // serial-only open used to fall back to localhost HTTP, silently,
        // whenever the SDK was absent. Forced, a missing SDK is an error.
        aaronia_source_builder_force_source_type(builder, NativeSdk);
    } else {
        // Nothing chosen: the HTTP server's own default, as before.
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
    //
    // Checked here rather than left to the C API, which ignores a value
    // it does not recognise and returns nothing to say so. A typo would
    // otherwise stream the default format while the device string
    // claims something else — and the server is no help either: it
    // answers an unknown `format=` with the RTSA file format rather
    // than an error.
    if (args.count("format") != 0) {
        const std::string &fmt = args.at("format");
        if (fmt != "F32" && fmt != "F16" && fmt != "I16") {
            SoapySDR::logf(SOAPY_SDR_WARNING,
                           "aaronia: ignoring format=%s; expected F32, F16 or I16",
                           fmt.c_str());
        } else {
            aaronia_source_builder_stream_format(builder, fmt.c_str());
        }
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
    // reconnect=0 opts out of automatic stream reconnection (on by
    // default): a dropped stream then ends the session instead of
    // recovering, and readStream reports an error.
    if (args.count("reconnect") != 0) {
        const std::string &v = args.at("reconnect");
        const bool enabled = !(v == "0" || v == "false" || v == "no");
        aaronia_source_builder_auto_reconnect(builder, enabled);
    }

    SourceGuard source(aaronia_source_build(builder));
    if (!source.p) {
        char* msg = aaronia_last_error();
        std::string err = msg ? msg : "Failed to build AaroniaSource";
        if (msg) aaronia_string_free(msg);
        throw std::runtime_error(err);
    }

    // A sink is built only when the source that was just constructed
    // is the native-SDK backend — the only one with a transmit path. Ask
    // the source itself rather than re-deriving the backend from the
    // arguments: a bare `driver=aaronia` auto-detects the SDK with no
    // `serial=` at all, and a `serial=` open falls back to HTTP when the
    // SDK is not installed, so the arguments get both cases wrong. A
    // live NativeSdk source cannot exist on a build without the TX code
    // (`AaroniaSource::new` refuses it), so this also covers the
    // compile-time half without a second check.
    //
    // Still not a hardware check: a V6 ECO has no transmitter and would
    // be advertised as having one here; settling that needs an SDK
    // query this crate does not yet make.
    bool txPossible = false;
    if (FfiSourceInfo* info = aaronia_source_get_source_info(source.p)) {
        txPossible = info->source_type == NativeSdk;
        aaronia_source_info_free(info);
    }
    SinkGuard sink(nullptr);
    if (txPossible) {
        AaroniaSinkBuilder* sink_builder = aaronia_sink_builder_new();
        SinkBuilderGuard sinkBuilderGuard(sink_builder);
        if (sink_builder) {
            // TX shares the tuning args with RX unless retuned later.
            if (hasFreq) aaronia_sink_builder_center_frequency(sink_builder, freq);
            if (hasRate) aaronia_sink_builder_sample_rate(sink_builder, rate);
            sink.p = aaronia_sink_build(sink_builder);
        }
    }

    // Device takes ownership of both on successful construction.
    AaroniaSoapyDevice *device = new AaroniaSoapyDevice(source.p, sink.p, args);
    source.release();
    sink.release();
    return device;
}

static SoapySDR::Registry registerAaronia("aaronia", &findAaronia, &makeAaronia, SOAPY_SDR_ABI_VERSION);
