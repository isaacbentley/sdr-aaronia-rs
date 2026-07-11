#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc = include_str!("../README.md")]
//! Aaronia RTSA Suite SDK and HTTP Streaming Client
//!
//! Provides a unified interface for connecting to Aaronia Spectran V6 units over
//! the Aaronia RTSA HTTP API, the Native C++ SDK, or reading raw IQ data from files.

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
#[cfg(all(feature = "http", feature = "file"))]
#[cfg_attr(docsrs, doc(cfg(all(feature = "http", feature = "file"))))]
pub mod unified_source;
pub mod utils;

pub use error::{Error, Result};
pub use num_complex::Complex32;

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
    db_to_linear, format_frequency, format_sample_rate, linear_to_db, parse_frequency,
    parse_sample_rate,
};
