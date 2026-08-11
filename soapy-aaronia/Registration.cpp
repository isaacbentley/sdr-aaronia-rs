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
        // Default HTTP endpoint
        device["url"] = "http://localhost:54664";
    }

    results.push_back(device);
    return results;
}

static SoapySDR::Device *makeAaronia(const SoapySDR::Kwargs &args) {
    AaroniaSourceBuilder* builder = aaronia_source_builder_new();
    if (!builder) {
        throw std::runtime_error("Failed to create AaroniaSourceBuilder");
    }

    if (args.count("url") != 0) {
        aaronia_source_builder_http_source(builder, args.at("url").c_str());
    } else if (args.count("file") != 0) {
        aaronia_source_builder_file_source(builder, args.at("file").c_str());
    } else {
        // Default to HTTP localhost if nothing specified
        aaronia_source_builder_http_source(builder, "http://localhost:54664");
    }

    if (args.count("freq") != 0) {
        aaronia_source_builder_center_frequency(builder, std::stod(args.at("freq")));
    }
    if (args.count("rate") != 0) {
        aaronia_source_builder_span_frequency(builder, std::stod(args.at("rate")));
    }

    AaroniaSource* source = aaronia_source_build(builder);
    aaronia_source_builder_free(builder);

    if (!source) {
        char* msg = aaronia_last_error();
        std::string err = msg ? msg : "Failed to build AaroniaSource";
        if (msg) aaronia_string_free(msg);
        throw std::runtime_error(err);
    }

    return new AaroniaSoapyDevice(source, args);
}

static SoapySDR::Registry registerAaronia("aaronia", &findAaronia, &makeAaronia, SOAPY_SDR_ABI_VERSION);
