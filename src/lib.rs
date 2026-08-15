#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc = include_str!("../README.md")]
//! Aaronia RTSA Suite SDK and HTTP Streaming Client
//!
//! Provides a unified interface for connecting to Aaronia Spectran V6 units over
//! the Aaronia RTSA HTTP API, the Native C++ SDK, or reading raw IQ data from files.

/// Compile-checks the Rust snippets in the standalone guides.
///
/// `README.md` is doctested by the `include_str!` above; the guides in
/// `docs/` live outside the API documentation but must not rot either —
/// a snippet that no longer compiles is worse than no snippet.
/// `cfg(doctest)` means this module exists only while doctests run, so
/// nothing is added to the rendered docs.
#[cfg(doctest)]
#[doc = include_str!("../docs/QUICKSTART.md")]
mod quickstart_guide {}

#[cfg(doctest)]
#[doc = include_str!("../docs/USAGE.md")]
mod usage_guide {}

#[cfg(feature = "ffi")]
#[cfg_attr(docsrs, doc(cfg(feature = "ffi")))]
pub mod c_api;
#[cfg(feature = "file")]
#[cfg_attr(docsrs, doc(cfg(feature = "file")))]
pub mod decompression;
pub mod detection;
pub mod error;
#[cfg(feature = "file")]
#[cfg_attr(docsrs, doc(cfg(feature = "file")))]
pub mod file_source;
#[cfg(feature = "http")]
#[cfg_attr(docsrs, doc(cfg(feature = "http")))]
pub mod http_endpoints;
#[cfg(feature = "futuresdr")]
#[cfg_attr(docsrs, doc(cfg(feature = "futuresdr")))]
pub mod http_sink;
#[cfg(feature = "futuresdr")]
#[cfg_attr(docsrs, doc(cfg(feature = "futuresdr")))]
pub mod http_source;
#[cfg(feature = "http")]
#[cfg_attr(docsrs, doc(cfg(feature = "http")))]
pub mod http_streaming;
#[cfg(feature = "sdr-source")]
#[cfg_attr(docsrs, doc(cfg(feature = "sdr-source")))]
pub mod sdr_source {
    pub use orecchiette_sdr_source_rs::*;
}
#[cfg(feature = "sdr-source")]
#[cfg_attr(docsrs, doc(cfg(feature = "sdr-source")))]
pub mod sdr_source_impl;
#[cfg(feature = "seify")]
#[cfg_attr(docsrs, doc(cfg(feature = "seify")))]
pub mod seify_impl;
pub mod unified_sink;
#[cfg(all(feature = "http", feature = "file"))]
#[cfg_attr(docsrs, doc(cfg(all(feature = "http", feature = "file"))))]
pub mod unified_source;
pub mod utils;

pub use error::{Error, Result};
pub use num_complex::Complex32;
pub use utils::RxChannel;

#[cfg(feature = "native-sdk")]
#[cfg_attr(docsrs, doc(cfg(feature = "native-sdk")))]
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub mod native_sdk;

#[cfg(feature = "native-sdk")]
#[cfg_attr(docsrs, doc(cfg(feature = "native-sdk")))]
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub mod sdk_source;

#[cfg(feature = "native-sdk")]
#[cfg_attr(docsrs, doc(cfg(feature = "native-sdk")))]
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub mod sdk_sink;

#[cfg(feature = "native-sdk")]
#[cfg_attr(docsrs, doc(cfg(feature = "native-sdk")))]
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use native_sdk::{NativeSdkClient, NativeSdkSource};

#[cfg(feature = "ffi")]
#[cfg_attr(docsrs, doc(cfg(feature = "ffi")))]
pub use c_api::{
    AaroniaFfiError, CAaroniaSourceType, FfiComplex, FfiServerInfo, FfiSourceInfo,
    aaronia_endpoints_client_control_recording, aaronia_endpoints_client_control_streaming,
    aaronia_endpoints_client_free, aaronia_endpoints_client_get_info, aaronia_endpoints_client_new,
    aaronia_get_error_message, aaronia_server_info_free, aaronia_source_build,
    aaronia_source_builder_center_frequency, aaronia_source_builder_file_source,
    aaronia_source_builder_free, aaronia_source_builder_http_source, aaronia_source_builder_new,
    aaronia_source_builder_reference_level, aaronia_source_builder_span_frequency,
    aaronia_source_free, aaronia_source_get_source_info, aaronia_source_info_free,
    aaronia_source_read_samples, aaronia_source_start_streaming, aaronia_source_stop_streaming,
    aaronia_string_free,
};
pub use detection::{get_sdk_library_path, get_sdk_path, get_xml_config_path, is_sdk_installed};
#[cfg(feature = "file")]
#[cfg_attr(docsrs, doc(cfg(feature = "file")))]
pub use file_source::{RtsaMetadata, RtsaSource, SampleData};
/// HTTP Endpoints management
#[cfg(feature = "http")]
#[cfg_attr(docsrs, doc(cfg(feature = "http")))]
pub use http_endpoints::{AuthMethod, HttpEndpointsClient, ServerInfo, UserInfo};
#[cfg(feature = "futuresdr")]
#[cfg_attr(docsrs, doc(cfg(feature = "futuresdr")))]
pub use http_sink::{HttpSink, HttpSinkBuilder};
/// HTTP IQ streaming client
#[cfg(feature = "futuresdr")]
#[cfg_attr(docsrs, doc(cfg(feature = "futuresdr")))]
pub use http_source::{HttpSource, HttpSourceBuilder, StreamStats};
/// Stream parsing for HTTP IQ packets
#[cfg(feature = "http")]
#[cfg_attr(docsrs, doc(cfg(feature = "http")))]
pub use http_streaming::{
    PacketMetadata, PayloadType, StreamFormat, StreamPacket, StreamParser,
    StreamingPerformanceStats,
};
/// `SdrSource`-trait facade — wraps the unified async source so the
/// orchestrator can dispatch through `Box<dyn SdrSource>` uniformly.
#[cfg(feature = "sdr-source")]
#[cfg_attr(docsrs, doc(cfg(feature = "sdr-source")))]
pub use sdr_source_impl::{AaroniaBackend, AaroniaSdrSource};
/// Unified SDR Source abstraction
#[cfg(all(feature = "http", feature = "file"))]
#[cfg_attr(docsrs, doc(cfg(all(feature = "http", feature = "file"))))]
pub use unified_source::{
    AaroniaConfig, AaroniaSource, AaroniaSourceBuilder, SourceInfo, SourceType,
};
/// Utilities for DB/linear conversions and string parsing
pub use utils::{
    IQ_CLOCK_HZ, USABLE_BANDWIDTH_RATIO, db_to_linear, decimation_index_for_bandwidth,
    decimation_index_for_rate, format_frequency, format_sample_rate, iq_sample_rate_for_bandwidth,
    iq_sample_rate_for_decimation_index, iq_sample_rates, iq_sample_rates_for_clock, linear_to_db,
    nearest_iq_sample_rate, parse_frequency, parse_sample_rate, usable_bandwidth_hz,
};
