//! Unified IQ File Source Interface
//!
//! This module provides a unified interface for reading IQ data from various file formats
//! including WAV files and Aaronia RTSA files. It automatically detects the file format
//! and provides a consistent API for accessing IQ samples.

use crate::{Error, Result};
use bitflags::bitflags;

use byteorder::{LittleEndian, ReadBytesExt};
use num_complex::Complex32;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use tracing::debug;

// --- RTSA Flags and Enums ---

bitflags! {
    /// These flags provide metadata about the stream and individual packets.
    #[derive(Debug, Hash, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub struct DspPacketFlags: u32 {
        /// A new stream starts with this packet.

        const STREAM_START = 0x00000001;
        /// The current stream ends after this packet.

        const STREAM_END = 0x00000002;
        /// A new segment starts with this packet.

        const SEGMENT_START = 0x00000004;
        /// The current segment ends with this packet.

        const SEGMENT_END = 0x00000008;
        /// The content of the stream is broken before this packet.

        const BREAK = 0x00000010;
        /// Flush the processing pipe down stream.

        const FLUSH = 0x00000020;
        /// This is the first sample of a packet.

        const PACKET_START = 0x00000040;
        /// This is the last sample of a packet.

        const PACKET_END = 0x00000080;
        /// Data overflow, and most likely clipped.

        const WARN_OVERFLOW = 0x00000100;
        /// Data missing due to packet drop.

        const WARN_DROPPED = 0x00000200;
        /// Data is inaccurate e.g. due to missing calibration or unstable clock.

        const WARN_INACCURATE = 0x00000400;
        /// Data has been resampled.

        const WARN_RESAMPLED = 0x00000800;
        /// The media sample is the start of a replay.

        const REPLAY = 0x00001000;
        /// The media sample is supposed to be processed immediately and displayed as a single update.

        const IMMEDIATE = 0x00002000;
        /// Start time of this sample may be before end time of previous sample.

        const TIME_OVERLAP = 0x00004000;
        /// Push the packet through the chain to the display, do not delay or combine.

        const PUSH = 0x00008000;
        /// There is a time discontinuity between this and the previous packet.

        const TIME_DISCONTINUITY = 0x00010000;
        /// The direction of the stream has changed (e.g. for direction finding antennas).

        const WARN_DIRECTION = 0x00020000;
        /// Eliminated by a filter.

        const REJECTED = 0x00100000;
        /// User defined flag 0.

        const USER_0 = 0x01000000;
        /// User defined flag 1.

        const USER_1 = 0x02000000;
        /// User defined flag 2.

        const USER_2 = 0x04000000;
        /// User defined flag 3.

        const USER_3 = 0x08000000;
        /// Condition flag 0.

        const CONDITION_0 = 0x10000000;
        /// Condition flag 1.

        const CONDITION_1 = 0x20000000;
        /// Condition flag 2.

        const CONDITION_2 = 0x40000000;
        /// Condition flag 3.

        const CONDITION_3 = 0x80000000;
    }
}

bitflags! {
    #[derive(Debug, Hash, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
    /// DsfhFlags struct.
    pub struct DsfhFlags: u32 {

        const HAS_TIMESTAMP = 0b0000_0001;

        const HAS_GPS_DATA = 0b0000_0010;

        const HAS_SPECTRUM_DATA = 0b0000_0100;

        const HAS_IQ_DATA = 0b0000_1000;
        // Add more flags as per PDF specification
    }
}

bitflags! {
    #[derive(Debug, Hash, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
    /// StrmFlags struct.
    pub struct StrmFlags: u32 {

        const IS_COMPRESSED = 0b0000_0001;

        const IS_ENCRYPTED = 0b0000_0010;
        // Add more flags as per PDF specification
    }
}

bitflags! {
    /// Flags for Sub Stream Category (SSCA) chunks.
    #[derive(Debug, Hash, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub struct DsscfFlags: u32 {

        const FREQUENCY_VALID = 0x00000001;

        const COLOR_VALID = 0x00000002;
    }
}

bitflags! {
    /// Flags for Antenna (ANTA) chunks.
    #[derive(Debug, Hash, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub struct DspafFlags: u32 {

        const LOCATION_VALID = 0x00000001;

        const TRANSFORM_VALID = 0x00000002;

        const DIRECTION_VALID = 0x00000004;

        const ROTATION = 0x00000008;
    }
}

/// SAMP Chunk: mSampleType
/// Specifies the data type of individual data elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum DspStreamSampleType {
    DsStU8,
    DsStU16,
    DsStU32,
    DsStS16,
    DsStS32,
    DsStF32,
    DsStU8N,
    DsStU16N,
    DsStS16N,
    DsStS32N,
    DsStF32N,
    Unknown,
}

impl TryFrom<u8> for DspStreamSampleType {
    type Error = crate::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(DspStreamSampleType::DsStU8),
            1 => Ok(DspStreamSampleType::DsStU16),
            2 => Ok(DspStreamSampleType::DsStU32),
            3 => Ok(DspStreamSampleType::DsStS16),
            4 => Ok(DspStreamSampleType::DsStS32),
            5 => Ok(DspStreamSampleType::DsStF32),
            6 => Ok(DspStreamSampleType::DsStU8N),
            7 => Ok(DspStreamSampleType::DsStU16N),
            8 => Ok(DspStreamSampleType::DsStS16N),
            10 => Ok(DspStreamSampleType::DsStS32N),
            11 => Ok(DspStreamSampleType::DsStF32N),
            _ => Err(Error::FileFormat {
                offset: 0,
                reason: format!("Invalid DspStreamSampleType value: {}", value),
            }),
        }
    }
}

/// SAMP Chunk: mSampleUnit
/// Specifies the physical unit for the sample data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum DspStreamSampleUnit {
    DssuGeneric,
    DssuDbm,
    DssuDbmHz,
    DssuPercentage,
    DssuHz,
    DssuWatt,
    DssuVolt,
    DssuTime,
    DssuDateTime,
    Unknown,
}

impl TryFrom<u8> for DspStreamSampleUnit {
    type Error = crate::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(DspStreamSampleUnit::DssuGeneric),
            1 => Ok(DspStreamSampleUnit::DssuDbm),
            2 => Ok(DspStreamSampleUnit::DssuDbmHz),
            3 => Ok(DspStreamSampleUnit::DssuPercentage),
            4 => Ok(DspStreamSampleUnit::DssuHz),
            5 => Ok(DspStreamSampleUnit::DssuWatt),
            6 => Ok(DspStreamSampleUnit::DssuVolt),
            7 => Ok(DspStreamSampleUnit::DssuTime),
            8 => Ok(DspStreamSampleUnit::DssuDateTime),
            19 => Ok(DspStreamSampleUnit::Unknown),
            _ => Err(Error::FileFormat {
                offset: 0,
                reason: format!("Invalid DspStreamSampleUnit value: {}", value),
            }),
        }
    }
}

/// SAMP Chunk: mPayloadType
/// Specifies the high-level structure of the sample data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum DspStreamPayloadType {
    DsptGeneric,
    DsptAudio,
    DsptIq,
    DsptSpectra,
    DsptDetection,
    DsptHistogram,
    DsptStructured,
    DsptImage,
    Unknown,
}

impl TryFrom<u8> for DspStreamPayloadType {
    type Error = crate::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(DspStreamPayloadType::DsptGeneric),
            1 => Ok(DspStreamPayloadType::DsptAudio),
            2 => Ok(DspStreamPayloadType::DsptIq),
            3 => Ok(DspStreamPayloadType::DsptSpectra),
            4 => Ok(DspStreamPayloadType::DsptDetection),
            5 => Ok(DspStreamPayloadType::DsptHistogram),
            6 => Ok(DspStreamPayloadType::DsptStructured),
            7 => Ok(DspStreamPayloadType::DsptImage),
            _ => Err(Error::FileFormat {
                offset: 0,
                reason: format!("Invalid DspStreamPayloadType value: {}", value),
            }),
        }
    }
}

// Add other flag structs as needed for ANTX, etc.

// --- RTSA Chunk Structures ---

/// Represents a generic RTSA file chunk header.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(C)]
pub struct RtsaChunkHeader {
    pub id: [u8; 4],
    pub size: u32,
    pub flags: u32,
    pub version: u16,
    pub header_size: u16,
}

/// DSFH (Data Stream File Header) Chunk
#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub struct DsfhChunk {
    pub header: RtsaChunkHeader,
    pub creation_time: f64,
}

/// DSFT (Data Stream File Trailer) Chunk
#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub struct DsftChunk {
    pub header: RtsaChunkHeader,
    pub completion_time: f64,
    pub stream_offset: u64,
    pub num_streams: u32,
}

/// STRM (Stream Head) Chunk - Supports both official and proximity-based formats.
#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub struct StrmChunk {
    pub header: RtsaChunkHeader,
    // Standard fields from RTSA spec
    pub stream_id: u64,
    pub start_time: f64,
    pub stream_offset: i64,
    // Proximity-based fields
    pub stream_type: Option<u32>,
    pub sample_rate: Option<f32>,
    pub center_frequency: Option<f32>,
    pub device_name: Option<[u8; 8]>,
}

/// SSTR (Sub Stream) Chunk
#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub struct SstrChunk {
    pub header: RtsaChunkHeader,
    pub stream_id: u64,
    pub sub_stream_id: u32,
    pub sub_stream_offset: i64,
    pub frequency_start: f64,
    pub frequency_step: f64,
    pub frequency_span: f64,
    pub value_minimum: f64,
    pub value_maximum: f64,
    pub direction: f64,
    pub antenna_index: u32,
    pub num_categories: u32,
    pub name: [u8; 128],
    pub antenna_id: u64,
    pub metadata_id: u64,
}

/// SSCA (Sub Stream Category) Chunk
#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub struct SscaChunk {
    pub header: RtsaChunkHeader,
    pub name: [u8; 128],
    pub flags: DsscfFlags,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
    pub start_frequency: f64,
    pub end_frequency: f64,
}

/// ANTA (Antenna) Chunk
#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub struct AntaChunk {
    pub header: RtsaChunkHeader,
    pub antenna_id: u64,
    pub antenna_offset: i64,
    pub name: [u8; 128],
    pub latitude: f64,
    pub longitude: f64,
    pub flags: DspafFlags,
    pub num_segments: u32,
    pub transform: [[f32; 4]; 4],
    pub antenna_uuid: [u8; 16],
}

/// ANTS (Antenna Segment) Chunk
#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub struct AntsChunk {
    pub header: RtsaChunkHeader,
    pub name: [u8; 128],
    pub orientation: [f32; 4],
    pub id: u32,
}

/// Defines the base types for structured data within MDTT chunks.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetaType {
    MtNone = 0,
    MtBool = 1,
    MtInteger = 2,
    MtFloat = 3,
    MtString = 4,
    MtVector = 5, // Fixed-size array
    MtArray = 6,  // Variable-size array
    MtObject = 7, // Structure with named child elements
}

impl TryFrom<u8> for MetaType {
    type Error = crate::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(MetaType::MtNone),
            1 => Ok(MetaType::MtBool),
            2 => Ok(MetaType::MtInteger),
            3 => Ok(MetaType::MtFloat),
            4 => Ok(MetaType::MtString),
            5 => Ok(MetaType::MtVector),
            6 => Ok(MetaType::MtArray),
            7 => Ok(MetaType::MtObject),
            _ => Err(Error::FileFormat {
                offset: 0,
                reason: format!("Invalid MetaType value: {}", value),
            }),
        }
    }
}

bitflags::bitflags! {
    /// Flags for MetaType definitions, particularly for integer types.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct MetaTypeFlags: u32 {

        const DSSMTF_8BIT = 0x00000001;

        const DSSMTF_16BIT = 0x00000002;

        const DSSMTF_32BIT = 0x00000004;

        const DSSMTF_64BIT = 0x00000008;

        const DSSMTF_SIGNED = 0x00000010;

        const DSSMEF_RECURSIVE = 0x00000020; // Meta Element Flag: indicates recursive definition
    }
}

/// Represents a single element within a MetaTypeDefinition (e.g., a field in an object).
#[derive(Debug, Clone, PartialEq)]
pub struct MetaTypeElement {
    pub name: String,
    pub flags: u32, // Not MetaTypeFlags, but general element flags
    pub definition: MetaTypeDefinition,
}

/// Represents the full definition of a structured data type from an MDTT chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct MetaTypeDefinition {
    pub id: u64,
    pub meta_type: MetaType,
    pub flags: MetaTypeFlags,
    pub count: u32, // For vectors/arrays, number of elements. For objects, number of fields.
    pub elements: Vec<MetaTypeElement>, // For objects, vector of child elements.
}

/// MDTT (Meta Data Type) Chunk
#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub struct MdttChunk {
    pub header: RtsaChunkHeader,
    pub metadata_id: u64,
    pub metadata_offset: i64,
    pub definition: Option<MetaTypeDefinition>, // The parsed type definition
}

/// SPRV (Preview) Chunk
#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub struct SprvChunk {
    pub header: RtsaChunkHeader,
    pub preview_level: u8,
    pub preview_count: u8,
    pub preview_offsets: [i64; 16],
    pub preview_times: [f64; 16],
    pub preview_samples: [u64; 16],
}

// SPRV Chunk Constants
// Preview chunk constants (currently unused but may be needed for future preview data processing)

/// STRT (Stream Tail) Chunk
#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub struct StrtChunk {
    pub header: RtsaChunkHeader,
    pub stream_offset: i64,
    pub sub_stream_offset: i64,
    pub preview_offset: i64,
    pub num_samples: u64,
    pub payload_size: u64,
    pub preview_levels: u32,
    pub num_previews: u32,
    pub num_preview_segments: u32,
    pub end_time: f64,
    pub antenna_offset: i64,
    pub metadata_offset: i64,
}

/// SAMP (Samples) Chunk
#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub struct SampChunk {
    pub header: RtsaChunkHeader,
    pub stream_id: u64,
    pub sub_stream_id: u32,
    pub sample_type: DspStreamSampleType,
    pub sample_unit: DspStreamSampleUnit,
    pub payload_type: DspStreamPayloadType,
    pub compression: i8,
    pub packet_start_time: f64,
    pub packet_end_time: f64,
    pub packet_flags: DspPacketFlags,
    pub sample_size: u32,
    pub sample_depth: u32,
    pub num_samples: u32,
}

/// Antenna information from ANTA chunks
#[derive(Debug, Clone)]
pub struct AntennaInfo {
    pub antenna_id: u64,
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub flags: DspafFlags,
    pub num_segments: u32,
    pub transform: [[f32; 4]; 4],
    pub antenna_uuid: [u8; 16],
}

/// Preview information from SPRV chunks
#[derive(Debug, Clone)]
pub struct PreviewInfo {
    pub preview_level: u8,
    pub preview_count: u8,
    pub preview_offsets: Vec<i64>,
    pub preview_times: Vec<f64>,
    pub preview_samples: Vec<u64>,
}

/// Stream tail information from STRT chunks
#[derive(Debug, Clone)]
pub struct StreamTailInfo {
    pub stream_offset: i64,
    pub sub_stream_offset: i64,
    pub preview_offset: i64,
    pub num_samples: u64,
    pub payload_size: u64,
    pub preview_levels: u32,
    pub num_previews: u32,
    pub num_preview_segments: u32,
    pub end_time: f64,
    pub antenna_offset: i64,
    pub metadata_offset: i64,
}

/// Sub-stream information from SSTR chunks
#[derive(Debug, Clone)]
pub struct SubStreamInfo {
    pub stream_id: u64,
    pub sub_stream_id: u32,
    pub sub_stream_offset: i64,
    pub frequency_start: f64,
    pub frequency_step: f64,
    pub frequency_span: f64,
    pub value_minimum: f64,
    pub value_maximum: f64,
    pub direction: f64,
    pub antenna_index: u32,
    pub num_categories: u32,
    pub name: String,
    pub antenna_id: u64,
    pub metadata_id: u64,
}

/// RTSA file validation report
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub metadata_completeness: f32,
}

/// Enhanced RTSA-specific metadata with comprehensive chunk information
#[derive(Debug, Clone)]
pub struct RtsaMetadata {
    // Core timing and frequency information
    pub sample_rate: f64,
    pub center_frequency: Option<f64>,
    pub bandwidth: f64,
    pub total_samples: u64,
    pub start_time_ns: u64,
    pub end_time_ns: u64,

    // File structure information
    pub creation_time: f64,
    pub num_streams: u32,
    pub file_format_version: String,

    // Stream information
    pub primary_stream_id: u64,
    pub stream_type: Option<String>,
    pub stream_sample_rate: Option<f32>,
    pub stream_center_frequency: Option<f32>,
    pub stream_start_time: f64,
    pub device_name: Option<String>,

    // Sub-stream information (for spectral data)
    pub sub_streams: Vec<SubStreamInfo>,

    // Antenna information
    pub antennas: Vec<AntennaInfo>,

    // Preview information
    pub previews: Vec<PreviewInfo>,

    // Stream tail information
    pub stream_tail: Option<StreamTailInfo>,

    // Sample chunk information
    pub total_sample_chunks: usize,
    pub sample_data_size: u64,

    // Metadata type definitions (MDTT chunks)
    pub metadata_definitions: Vec<(u64, MetaTypeDefinition)>,
}

/// Represents different types of sample data that can be read from an RTSA file.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SampleData {
    Iq(Vec<Complex32>),
    Spectra(Vec<f32>),
    Audio(Vec<i16>),
    Detection(Vec<f32>),
    Histogram(Vec<u32>),
    Structured(Vec<u8>),
    Image(Vec<u8>),
}

/// Heuristically normalise an RTSA file timestamp to *seconds since the
/// Unix epoch* as `f64`.
///
/// **Background**: the RTSA File Format v4 specification documents these
/// fields as `double` "seconds relative to the epoch", but as one forum
/// poster reported, some fields decode as **microseconds since the Unix
/// epoch** in real captures — specifically `DSFH::mCreationTime`
/// and `DSFT::mCompletionTime`. The official Aaronia tooling apparently
/// emits microseconds for those fields while `STRM::mStartTime` stays in
/// seconds. Without an authoritative way to tell from chunk metadata
/// alone, we apply a value-range heuristic:
///
/// * `>= 1e13` (which corresponds to roughly the year 2286 if interpreted
///   as seconds): treat as microseconds and divide by 1e6.
/// * Otherwise: treat as already-seconds (the spec's documented unit).
///
/// The cutoff sits well above any realistic Unix-seconds timestamp this
/// century (2025 ≈ 1.7e9, 2100 ≈ 4.1e9) and well below any plausible
/// Unix-microseconds timestamp from the same era (2025 ≈ 1.7e15). Inputs
/// that aren't finite or are non-positive are returned unchanged so
/// downstream code can decide how to handle them.
///
/// `0.0` returns `0.0` so "no timestamp" stays "no timestamp".
pub fn rtsa_epoch_seconds(raw: f64) -> f64 {
    if !raw.is_finite() || raw <= 0.0 {
        return raw;
    }
    if raw >= 1e13 { raw / 1e6 } else { raw }
}

/// All chunks accumulated during a single pass through an RTSA file.
///
/// This is an internal helper introduced to keep `parse_rtsa_with_tail`
/// and its `process_strt_based_structure` / `process_strh_based_structure`
/// callees inside Clippy's `too_many_arguments` budget. The fields mirror
/// the locals that `RtsaSource::open` previously declared one-by-one.
#[derive(Default)]
struct RtsaParseState {
    dsfh_chunk: Option<DsfhChunk>,
    strm_chunks: HashMap<u64, StrmChunk>,
    sstr_chunks: HashMap<u32, SstrChunk>,
    strt_chunk: Option<StrtChunk>,
    anta_chunks: HashMap<u64, AntaChunk>,
    mdtt_chunks: HashMap<u64, MdttChunk>,
    sprv_chunks: Vec<SprvChunk>,
    samp_chunk_offsets: Vec<(u64, SampChunk)>,
    iq_stream_id: Option<u64>,
}

/// Configuration for building comprehensive metadata from RTSA chunks
struct RtsaMetadataBuilder<'a> {
    dsfh_chunk: &'a DsfhChunk,
    primary_strm_chunk: &'a StrmChunk,
    strm_chunks: &'a HashMap<u64, StrmChunk>,
    sstr_chunks: &'a HashMap<u32, SstrChunk>,
    strt_chunk: &'a Option<StrtChunk>,
    anta_chunks: &'a HashMap<u64, AntaChunk>,
    mdtt_chunks: &'a HashMap<u64, MdttChunk>,
    sprv_chunks: &'a [SprvChunk],
    samp_chunk_offsets: &'a [(u64, SampChunk)],
    iq_stream_id: u64,
    stream_offset: Option<u64>,
}

/// Aaronia RTSA file source
pub struct RtsaSource {
    reader: std::io::BufReader<std::fs::File>,
    metadata: RtsaMetadata,
    current_sample_index: u64,
    iq_stream_id: u64,
    samp_chunk_offsets: Vec<(u64, SampChunk)>,
    samp_chunk_start_samples: std::collections::HashMap<u64, u64>,
    /// Index into `samp_chunk_offsets` where the last successful read left
    /// off. Sequential reads resume the chunk scan here instead of
    /// rescanning from the start of the list on every call.
    chunk_scan_hint: usize,
    /// For reverse-order RTSA files, this contains the offset where metadata starts
    /// (and thus where raw IQ data ends). If None, this is a standard RTSA format.
    raw_iq_data_end_offset: Option<u64>,
    /// Optional temp file holding uncompressed data from RTSAFileTool fallback, ensuring cleanup on drop.
    _temp_path: Option<tempfile::TempPath>,
}

impl RtsaSource {
    /// Open an RTSA file and parse its header and chunks efficiently.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open_internal(path, None)
    }

    fn open_internal<P: AsRef<Path>>(
        path: P,
        temp_path: Option<tempfile::TempPath>,
    ) -> Result<Self> {
        let file = std::fs::File::open(&path)?;
        let file_size = file.metadata()?.len();
        let mut reader = std::io::BufReader::new(file);
        debug!("Opening RTSA file: {} bytes", file_size);

        let mut state = RtsaParseState::default();
        let stream_offset = Self::parse_rtsa_with_tail(&mut reader, file_size, &mut state)?;

        let RtsaParseState {
            dsfh_chunk,
            strm_chunks,
            sstr_chunks,
            strt_chunk,
            anta_chunks,
            mdtt_chunks,
            sprv_chunks,
            samp_chunk_offsets,
            iq_stream_id,
        } = state;

        let iq_stream_id = iq_stream_id.ok_or_else(|| Error::FileFormat {
            offset: 0,
            reason: "RTSA file missing IQ stream.".to_string(),
        })?;
        let strm_chunk = strm_chunks
            .get(&iq_stream_id)
            .ok_or_else(|| Error::FileFormat {
                offset: 0,
                reason: "Missing STRM chunk for IQ stream".to_string(),
            })?;

        // Extract comprehensive metadata from all chunks
        let metadata = Self::build_comprehensive_metadata(RtsaMetadataBuilder {
            dsfh_chunk: &dsfh_chunk.ok_or_else(|| Error::FileFormat {
                offset: 0,
                reason: "Missing required DSFH chunk".to_string(),
            })?,
            primary_strm_chunk: strm_chunk,
            strm_chunks: &strm_chunks,
            sstr_chunks: &sstr_chunks,
            strt_chunk: &strt_chunk,
            anta_chunks: &anta_chunks,
            mdtt_chunks: &mdtt_chunks,
            sprv_chunks: &sprv_chunks,
            samp_chunk_offsets: &samp_chunk_offsets,
            iq_stream_id,
            stream_offset,
        })?;

        // Determine if this is a reverse-order format based on SAMP chunks
        let raw_iq_data_end_offset = if samp_chunk_offsets.is_empty() {
            debug!("Detected reverse-order RTSA format: no SAMP chunks found");
            // No SAMP chunks - this is reverse-order format with raw IQ data at start
            stream_offset // Use actual DSFT stream_offset
        } else {
            debug!(
                "Standard RTSA format: found {} SAMP chunks, STRT present: {}",
                samp_chunk_offsets.len(),
                strt_chunk.is_some()
            );
            None
        };

        let mut samp_chunk_start_samples = std::collections::HashMap::new();
        let mut stream_sample_counters = std::collections::HashMap::new();
        let mut is_compressed_iq = false;

        for &(offset, ref samp) in &samp_chunk_offsets {
            let key = (samp.stream_id, samp.sub_stream_id);
            let current_counter = stream_sample_counters.entry(key).or_insert(0u64);
            samp_chunk_start_samples.insert(offset, *current_counter);
            *current_counter += samp.num_samples as u64;

            if samp.payload_type == DspStreamPayloadType::DsptIq && samp.compression > 0 {
                is_compressed_iq = true;
            }
        }

        if is_compressed_iq {
            if temp_path.is_some() {
                return Err(Error::FileFormat {
                    offset: 0,
                    reason: "Failed to decompress RTSA file: the RTSAFileTool output is still compressed.".to_string()
                });
            }

            let tool_path = crate::detection::get_rtsa_file_tool_path().ok_or_else(|| {
                Error::FileFormat { offset: 0, reason: "DSPT_IQ compression detected, but RTSAFileTool was not found on this system. Please install Aaronia RTSA-Suite PRO to enable decompression.".to_string() }
            })?;

            let new_temp_path = tempfile::Builder::new()
                .suffix(".rtsa")
                .tempfile()?
                .into_temp_path();

            tracing::info!("DSPT_IQ compression detected. Executing RTSAFileTool to decompress...");
            let status = std::process::Command::new(&tool_path)
                .arg("repair")
                .arg("-compress=0")
                .arg(path.as_ref())
                .arg(&new_temp_path)
                .status()?;

            if !status.success() {
                return Err(Error::FileFormat {
                    offset: 0,
                    reason: format!("RTSAFileTool failed with status: {}", status),
                });
            }

            let new_path_buf = new_temp_path.to_path_buf();
            return Self::open_internal(new_path_buf, Some(new_temp_path));
        }

        Ok(Self {
            reader,
            metadata,
            current_sample_index: 0,
            iq_stream_id,
            samp_chunk_offsets,
            samp_chunk_start_samples,
            raw_iq_data_end_offset,
            chunk_scan_hint: 0,
            _temp_path: temp_path,
        })
    }

    /// Build comprehensive metadata from all available chunks
    fn build_comprehensive_metadata(builder: RtsaMetadataBuilder) -> Result<RtsaMetadata> {
        let RtsaMetadataBuilder {
            dsfh_chunk,
            primary_strm_chunk,
            strm_chunks,
            sstr_chunks,
            strt_chunk,
            anta_chunks,
            mdtt_chunks,
            sprv_chunks,
            samp_chunk_offsets,
            iq_stream_id,
            stream_offset,
        } = builder;
        // Extract antenna information
        let antennas: Vec<AntennaInfo> = anta_chunks
            .values()
            .map(|anta| AntennaInfo {
                antenna_id: anta.antenna_id,
                name: String::from_utf8_lossy(&anta.name)
                    .trim_end_matches('\0')
                    .to_string(),
                latitude: anta.latitude,
                longitude: anta.longitude,
                flags: anta.flags,
                num_segments: anta.num_segments,
                transform: anta.transform,
                antenna_uuid: anta.antenna_uuid,
            })
            .collect();

        // Extract preview information. The on-disk SPRV chunk advertises a
        // `preview_count` byte but stores the offsets/times/samples in
        // fixed-size arrays of length 16 — clamp the slice indices to that
        // capacity so a corrupt or out-of-range count can't panic the
        // whole parse.
        let previews: Vec<PreviewInfo> = sprv_chunks
            .iter()
            .map(|sprv| {
                let n = (sprv.preview_count as usize).min(sprv.preview_offsets.len());
                PreviewInfo {
                    preview_level: sprv.preview_level,
                    preview_count: sprv.preview_count,
                    preview_offsets: sprv.preview_offsets[..n].to_vec(),
                    preview_times: sprv.preview_times[..n].to_vec(),
                    preview_samples: sprv.preview_samples[..n].to_vec(),
                }
            })
            .collect();

        // Extract sub-stream information
        let sub_streams: Vec<SubStreamInfo> = sstr_chunks
            .values()
            .map(|sstr| SubStreamInfo {
                stream_id: sstr.stream_id,
                sub_stream_id: sstr.sub_stream_id,
                sub_stream_offset: sstr.sub_stream_offset,
                frequency_start: sstr.frequency_start,
                frequency_step: sstr.frequency_step,
                frequency_span: sstr.frequency_span,
                value_minimum: sstr.value_minimum,
                value_maximum: sstr.value_maximum,
                direction: sstr.direction,
                antenna_index: sstr.antenna_index,
                num_categories: sstr.num_categories,
                name: String::from_utf8_lossy(&sstr.name)
                    .trim_end_matches('\0')
                    .to_string(),
                antenna_id: sstr.antenna_id,
                metadata_id: sstr.metadata_id,
            })
            .collect();

        // Extract stream tail information. STRT.end_time is normalised the
        // same way DSFH.creation_time and DSFT.completion_time are — see
        // [`rtsa_epoch_seconds`] for the µs-vs-seconds heuristic.
        let stream_tail = strt_chunk.as_ref().map(|strt| StreamTailInfo {
            stream_offset: strt.stream_offset,
            sub_stream_offset: strt.sub_stream_offset,
            preview_offset: strt.preview_offset,
            num_samples: strt.num_samples,
            payload_size: strt.payload_size,
            preview_levels: strt.preview_levels,
            num_previews: strt.num_previews,
            num_preview_segments: strt.num_preview_segments,
            end_time: rtsa_epoch_seconds(strt.end_time),
            antenna_offset: strt.antenna_offset,
            metadata_offset: strt.metadata_offset,
        });

        // Extract metadata type definitions
        let metadata_definitions: Vec<(u64, MetaTypeDefinition)> = mdtt_chunks
            .iter()
            .filter_map(|(id, mdtt)| mdtt.definition.as_ref().map(|def| (*id, def.clone())))
            .collect();

        // Calculate sample data size
        let sample_data_size: u64 = samp_chunk_offsets
            .iter()
            .map(|(_, samp)| samp.header.size as u64)
            .sum();

        // Determine core timing and frequency information based on available chunks
        // Prioritize SAMP/STRT data when available as it's more detailed
        let (
            mut sample_rate,
            mut center_frequency,
            mut bandwidth,
            mut total_samples,
            mut start_time_ns,
            mut end_time_ns,
        ) = if let Some(samp_chunk) = samp_chunk_offsets
            .iter()
            .find(|(_, s)| s.stream_id == iq_stream_id)
            .map(|(_, chunk)| chunk)
        {
            // Standard RTSA format with SAMP chunks
            if let Some(sstr_chunk) = sstr_chunks.get(&samp_chunk.sub_stream_id) {
                let total_samples = strt_chunk.as_ref().map(|s| s.num_samples).unwrap_or(0);
                let end_time_ns = strt_chunk
                    .as_ref()
                    .map(|s| (rtsa_epoch_seconds(s.end_time) * 1_000_000_000.0) as u64)
                    .unwrap_or(0);
                (
                    primary_strm_chunk
                        .sample_rate
                        .map(|s| s as f64)
                        .unwrap_or(sstr_chunk.frequency_step),
                    primary_strm_chunk
                        .center_frequency
                        .map(|f| f as f64)
                        .or_else(|| {
                            Some(sstr_chunk.frequency_start + sstr_chunk.frequency_span / 2.0)
                        }),
                    sstr_chunk.frequency_span,
                    total_samples,
                    (primary_strm_chunk.start_time * 1_000_000_000.0) as u64,
                    end_time_ns,
                )
            } else {
                return Err(Error::FileFormat {
                    offset: 0,
                    reason: "Missing SSTR chunk for IQ stream".to_string(),
                });
            }
        } else if let (Some(sample_rate), Some(center_frequency)) = (
            primary_strm_chunk.sample_rate,
            primary_strm_chunk.center_frequency,
        ) {
            // Direct stream metadata available (fallback)
            (
                sample_rate as f64,
                Some(center_frequency as f64),
                0.0,
                0,
                (primary_strm_chunk.start_time * 1_000_000_000.0) as u64,
                0,
            )
        } else if strt_chunk.is_some() || samp_chunk_offsets.is_empty() {
            // Reverse-order RTSA format or missing SAMP chunks
            let raw_iq_bytes = stream_offset.unwrap_or(0u64);
            let total_samples = raw_iq_bytes / 8; // 8 bytes per Complex32
            (
                0.0,  // Unknown - must be determined by analysis
                None, // Unknown - must be determined by analysis
                0.0,  // Unknown - will be set based on sample rate when known
                total_samples,
                (primary_strm_chunk.start_time * 1_000_000_000.0) as u64,
                0, // Will be calculated based on total_samples and sample rate
            )
        } else {
            return Err(Error::FileFormat {
                offset: 0,
                reason: "No SAMP chunks or STRT metadata found for IQ stream".to_string(),
            });
        };

        // Helper: deterministic iteration over `strm_chunks` for the
        // tail-end fallbacks. `HashMap::values()` has nondeterministic
        // order, so if multiple non-IQ streams (e.g. audio, GPS) have
        // their own sample-rate / centre-frequency fields, picking
        // randomly between them would make the resolved metadata flip
        // run-to-run for the same input file. Sort by `stream_id` so
        // we always pick the same chunk on a given file.
        let sorted_strm_chunks: Vec<&StrmChunk> = {
            let mut v: Vec<&StrmChunk> = strm_chunks.values().collect();
            v.sort_by_key(|s| s.stream_id);
            v
        };

        // Fallback sample rate and center frequency logic
        if sample_rate <= 0.0 {
            if let Some(ssr) = sub_streams.iter().find(|s| s.frequency_step > 0.0) {
                sample_rate = ssr.frequency_step;
            } else if let Some(ssr_freq) = primary_strm_chunk.sample_rate {
                sample_rate = ssr_freq as f64;
            } else {
                // Fallback to any other stream chunk's sample rate if
                // available, iterating in deterministic stream_id order.
                for strm in &sorted_strm_chunks {
                    if let Some(sr) = strm.sample_rate
                        && sr > 0.0 {
                            sample_rate = sr as f64;
                            break;
                        }
                }
            }
        }

        if center_frequency.is_none() {
            if let Some(scf) = primary_strm_chunk.center_frequency {
                center_frequency = Some(scf as f64);
            } else if let Some(ssr) = sub_streams.first() {
                center_frequency = Some(ssr.frequency_start + ssr.frequency_span / 2.0);
            } else {
                // Fallback to any other stream chunk's center
                // frequency, iterating in deterministic stream_id
                // order.
                for strm in &sorted_strm_chunks {
                    if let Some(cf) = strm.center_frequency
                        && cf > 0.0 {
                            center_frequency = Some(cf as f64);
                            break;
                        }
                }
            }
        }

        // Fallback for total_samples: sum samples from parsed SAMP chunks if STRT was missing/reported 0
        if total_samples == 0 {
            total_samples = samp_chunk_offsets
                .iter()
                .filter(|(_, samp)| samp.stream_id == iq_stream_id)
                .map(|(_, samp)| samp.num_samples as u64)
                .sum();
        }

        // Fallback for start_time_ns: use DSFH creation time if start_time is 0
        let creation_time_val = rtsa_epoch_seconds(dsfh_chunk.creation_time);
        if start_time_ns == 0 {
            start_time_ns = (creation_time_val * 1_000_000_000.0) as u64;
        }

        // Fallback for end_time_ns: calculate using sample rate and total samples if missing
        if end_time_ns == 0 && sample_rate > 0.0 && total_samples > 0 {
            end_time_ns =
                start_time_ns + ((total_samples as f64 / sample_rate) * 1_000_000_000.0) as u64;
        }

        // Fallback for bandwidth: use the first sub-stream span, or fall back to the sample rate
        if bandwidth <= 0.0 {
            if let Some(ssr) = sub_streams.first() {
                bandwidth = ssr.frequency_span;
            } else {
                bandwidth = sample_rate;
            }
        }

        // Extract stream type information
        let stream_type = if !samp_chunk_offsets.is_empty() {
            Some("IQ_SAMPLES".to_string())
        } else {
            Some("RAW_IQ".to_string())
        };

        // Extract device name with fallbacks. Uses the
        // `sorted_strm_chunks` we computed above so the device-name
        // selection is deterministic across runs (same rationale as
        // the sample-rate / centre-frequency fallbacks).
        let device_name = primary_strm_chunk
            .device_name
            .map(|name| {
                String::from_utf8_lossy(&name)
                    .trim_end_matches('\0')
                    .trim()
                    .to_string()
            })
            .or_else(|| {
                sorted_strm_chunks.iter().find_map(|strm| {
                    strm.device_name.map(|name| {
                        String::from_utf8_lossy(&name)
                            .trim_end_matches('\0')
                            .trim()
                            .to_string()
                    })
                })
            });

        Ok(RtsaMetadata {
            // Core timing and frequency information
            sample_rate,
            center_frequency,
            bandwidth,
            total_samples,
            start_time_ns,
            end_time_ns,

            // File structure information
            // DSFH.creation_time has been observed to decode as
            // *microseconds* since the Unix epoch on real captures even
            // though the spec describes it as seconds. Normalise here.
            creation_time: rtsa_epoch_seconds(dsfh_chunk.creation_time),
            num_streams: strm_chunks.len() as u32,
            file_format_version: "RTSA".to_string(),

            // Stream information
            primary_stream_id: iq_stream_id,
            stream_type,
            stream_sample_rate: primary_strm_chunk.sample_rate,
            stream_center_frequency: primary_strm_chunk.center_frequency,
            stream_start_time: primary_strm_chunk.start_time,
            device_name,

            // Comprehensive chunk information
            sub_streams,
            antennas,
            previews,
            stream_tail,
            total_sample_chunks: samp_chunk_offsets.len(),
            sample_data_size,
            metadata_definitions,
        })
    }

    /// Main parsing function for RTSA files.
    ///
    /// All discovered chunks are accumulated into `state` so the caller can
    /// extract them after parsing completes. This struct exists so the
    /// signature stays under Clippy's `too_many_arguments` budget — see
    /// [`RtsaParseState`].
    fn parse_rtsa_with_tail(
        reader: &mut std::io::BufReader<std::fs::File>,
        file_size: u64,
        state: &mut RtsaParseState,
    ) -> Result<Option<u64>> {
        debug!(
            "Starting indexed sequential parsing for {} byte file",
            file_size
        );

        let dsft_chunk = Self::find_dsft_tail(reader, file_size)?;
        debug!(
            "Found DSFT chunk: stream_offset=0x{:08X}, num_streams={}",
            dsft_chunk.stream_offset, dsft_chunk.num_streams
        );

        let dsfh_offset = Self::find_dsfh_near_stream(reader, dsft_chunk.stream_offset)?;
        reader.seek(SeekFrom::Start(dsfh_offset))?;
        let header = RtsaChunkHeader::read_from(reader)?;
        if &header.id != b"DSFH" {
            return Err(Error::FileFormat {
                offset: 0,
                reason: format!(
                    "Expected DSFH chunk at offset 0x{:08X}, found {:?}",
                    dsfh_offset,
                    std::str::from_utf8(&header.id)
                ),
            });
        }
        let dsfh = DsfhChunk::read_from(reader, header.size)?;
        debug!("Found DSFH chunk: creation_time={}", dsfh.creation_time);
        state.dsfh_chunk = Some(dsfh);

        if dsft_chunk.stream_offset > 0 {
            match Self::find_strt_near_stream(reader, dsft_chunk.stream_offset) {
                Ok(found_strt_chunk) => {
                    debug!("Found STRT chunk, processing pointer-based stream structure.");
                    Self::process_strt_based_structure(
                        reader,
                        found_strt_chunk.clone(),
                        state,
                        &dsft_chunk,
                    )?;
                    state.strt_chunk = Some(found_strt_chunk);
                }
                Err(_) => {
                    debug!("No STRT chunk found, trying proximity-based discovery.");
                    Self::process_strh_based_structure(reader, dsft_chunk.stream_offset, state)?;
                }
            }
        } else {
            return Err(Error::FileFormat {
                offset: 0,
                reason: "No streams found in RTSA file (DSFT stream_offset is 0)".to_string(),
            });
        }

        debug!(
            "RTSA parsing complete. Found {} streams, {} SAMP chunks. IQ stream ID: {:?}",
            state.strm_chunks.len(),
            state.samp_chunk_offsets.len(),
            state.iq_stream_id
        );

        // Return stream_offset only for reverse-order format (no SAMP chunks)
        // Standard RTSA files with SAMP chunks don't need this information
        if state.samp_chunk_offsets.is_empty() && dsft_chunk.stream_offset > 0 {
            Ok(Some(dsft_chunk.stream_offset))
        } else {
            Ok(None)
        }
    }

    /// Find the DSFH (Data Stream File Header) chunk.
    fn find_dsfh_chunk(
        reader: &mut std::io::BufReader<std::fs::File>,
        file_size: u64,
    ) -> Result<u64> {
        // Try the file head first (where a well-formed DSFH lives), then
        // the midpoint as a heuristic for files with prepended data. The
        // broad linear search below is the authoritative fallback.
        let search_positions = vec![0u64, file_size / 2];
        for &pos in &search_positions {
            if pos >= file_size {
                continue;
            }
            let search_start = pos.saturating_sub(1024);
            let search_end = (pos + 1_000_000).min(file_size);
            reader.seek(SeekFrom::Start(search_start))?;
            let mut buffer = vec![0u8; (search_end - search_start) as usize];
            if reader.read(&mut buffer)? > 0
                && let Some(i) = buffer.windows(4).position(|w| w == b"DSFH") {
                    return Ok(search_start + i as u64);
                }
        }
        Self::broad_search_for_chunk(reader, file_size, b"DSFH")
    }

    /// Perform a broad search for a chunk signature.
    fn broad_search_for_chunk(
        reader: &mut std::io::BufReader<std::fs::File>,
        file_size: u64,
        signature: &[u8; 4],
    ) -> Result<u64> {
        const SEARCH_CHUNK_SIZE: usize = 10_000_000;
        let mut position = 0u64;
        let mut buffer = vec![0u8; SEARCH_CHUNK_SIZE];
        while position < file_size {
            reader.seek(SeekFrom::Start(position))?;
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            if let Some(i) = buffer[..bytes_read].windows(4).position(|w| w == signature) {
                return Ok(position + i as u64);
            }
            position += bytes_read.saturating_sub(3) as u64;
        }
        Err(Error::FileFormat {
            offset: 0,
            reason: format!(
                "Could not find '{:?}' chunk in RTSA file",
                std::str::from_utf8(signature)
            ),
        })
    }

    /// Process stream structure based on STRT chunk pointers.
    fn process_strt_based_structure(
        reader: &mut std::io::BufReader<std::fs::File>,
        strt_chunk: StrtChunk,
        state: &mut RtsaParseState,
        dsft_chunk: &DsftChunk,
    ) -> Result<()> {
        debug!(
            "Processing STRT chunk: stream_offset=0x{:08X}, num_samples={}",
            strt_chunk.stream_offset, strt_chunk.num_samples
        );

        if strt_chunk.sub_stream_offset > 0 {
            Self::parse_sstr_chain(
                reader,
                strt_chunk.sub_stream_offset as u64,
                &mut state.sstr_chunks,
            )?;
        }
        if strt_chunk.antenna_offset > 0 {
            Self::parse_anta_chain(
                reader,
                strt_chunk.antenna_offset as u64,
                &mut state.anta_chunks,
            )?;
        }
        if strt_chunk.metadata_offset > 0 {
            Self::parse_mdtt_chain(
                reader,
                strt_chunk.metadata_offset as u64,
                &mut state.mdtt_chunks,
            )?;
        }
        if strt_chunk.preview_offset > 0 {
            Self::parse_sprv_tree(
                reader,
                strt_chunk.preview_offset as u64,
                &mut state.sprv_chunks,
            )?;
        }

        reader.seek(SeekFrom::Start(strt_chunk.stream_offset as u64))?;
        let strm_header = RtsaChunkHeader::read_from(reader)?;
        if &strm_header.id == b"STRM" {
            let strm = StrmChunk::read_from(reader, strm_header.size)?;
            debug!(
                "Found STRM chunk: stream_id={}, start_time={}",
                strm.stream_id, strm.start_time
            );
            state.iq_stream_id = Some(strm.stream_id);
            state.strm_chunks.insert(strm.stream_id, strm);
        }

        Self::scan_for_samp_chunks(
            reader,
            strt_chunk.stream_offset as u64,
            dsft_chunk.stream_offset,
            &mut state.samp_chunk_offsets,
        )?;
        Ok(())
    }

    /// Process stream structure based on proximity discovery.
    fn process_strh_based_structure(
        reader: &mut std::io::BufReader<std::fs::File>,
        stream_area_offset: u64,
        state: &mut RtsaParseState,
    ) -> Result<()> {
        debug!(
            "Searching for STRM chunks near offset 0x{:08X}",
            stream_area_offset
        );
        let search_start = stream_area_offset.saturating_sub(4096);
        let search_size = 8192u64;
        reader.seek(SeekFrom::Start(search_start))?;
        let mut buffer = vec![0u8; search_size as usize];
        let bytes_read = reader.read(&mut buffer)?;

        if let Some(i) = buffer[..bytes_read].windows(4).position(|w| w == b"STRM") {
            let strm_offset = search_start + i as u64;
            if i + 8 <= bytes_read {
                let chunk_size = u32::from_le_bytes(buffer[i + 4..i + 8].try_into().unwrap());
                if (20..1000).contains(&chunk_size) {
                    reader.seek(SeekFrom::Start(strm_offset))?;
                    let strm_header = RtsaChunkHeader::read_from(reader)?;
                    if &strm_header.id == b"STRM" {
                        let strm = StrmChunk::read_from(reader, strm_header.size)?;
                        debug!(
                            "Found STRM chunk via proximity search: stream_id={}",
                            strm.stream_id
                        );
                        state.iq_stream_id = Some(strm.stream_id);
                        state.strm_chunks.insert(strm.stream_id, strm);
                    }
                }
            }
        }

        if state.strm_chunks.is_empty() {
            return Err(Error::FileFormat {
                offset: 0,
                reason: "Could not find STRM chunks via proximity search".to_string(),
            });
        }

        let scan_start = stream_area_offset;
        let scan_end = stream_area_offset + 100_000_000;
        Self::scan_for_samp_chunks(reader, scan_start, scan_end, &mut state.samp_chunk_offsets)?;
        Ok(())
    }

    /// Find the DSFT (Data Stream File Tail) chunk at the end of the file.
    fn find_dsft_tail(
        reader: &mut std::io::BufReader<std::fs::File>,
        file_size: u64,
    ) -> Result<DsftChunk> {
        let search_window = 1024.min(file_size);
        reader.seek(SeekFrom::End(-(search_window as i64)))?;
        let mut buffer = vec![0u8; search_window as usize];
        reader.read_exact(&mut buffer)?;

        if let Some(i) = buffer.windows(4).rposition(|w| w == b"DSFT") {
            let dsft_pos = file_size - search_window + i as u64;
            reader.seek(SeekFrom::Start(dsft_pos))?;
            let header = RtsaChunkHeader::read_from(reader)?;
            if &header.id == b"DSFT" {
                return DsftChunk::read_from(reader, header.size);
            }
        }
        Err(Error::FileFormat {
            offset: 0,
            reason: "Could not find DSFT chunk in RTSA file".to_string(),
        })
    }

    /// Find STRT chunk near the stream area.
    fn find_strt_near_stream(
        reader: &mut std::io::BufReader<std::fs::File>,
        stream_area_offset: u64,
    ) -> Result<StrtChunk> {
        let search_start = stream_area_offset.saturating_sub(1024);
        let search_size = 4096u64;
        reader.seek(SeekFrom::Start(search_start))?;
        let mut buffer = vec![0u8; search_size as usize];
        let bytes_read = reader.read(&mut buffer)?;

        if let Some(i) = buffer[..bytes_read].windows(4).position(|w| w == b"STRT") {
            let strt_offset = search_start + i as u64;
            reader.seek(SeekFrom::Start(strt_offset))?;
            let header = RtsaChunkHeader::read_from(reader)?;
            if &header.id == b"STRT" {
                return StrtChunk::read_from(reader, header.size);
            }
        }
        Err(Error::FileFormat {
            offset: 0,
            reason: "Could not find STRT chunk near stream area".to_string(),
        })
    }

    /// Find DSFH chunk near the stream area.
    fn find_dsfh_near_stream(
        reader: &mut std::io::BufReader<std::fs::File>,
        stream_area_offset: u64,
    ) -> Result<u64> {
        let search_start = stream_area_offset.saturating_sub(1024);
        let search_size = 4096u64;
        reader.seek(SeekFrom::Start(search_start))?;
        let mut buffer = vec![0u8; search_size as usize];
        let bytes_read = reader.read(&mut buffer)?;

        if let Some(i) = buffer[..bytes_read].windows(4).position(|w| w == b"DSFH") {
            let dsfh_offset = search_start + i as u64;
            if i + 8 <= bytes_read {
                let chunk_size = u32::from_le_bytes(buffer[i + 4..i + 8].try_into().unwrap());
                if (0..1000).contains(&chunk_size) {
                    return Ok(dsfh_offset);
                }
            }
        }
        Self::find_dsfh_chunk(reader, stream_area_offset * 2)
    }

    fn is_valid_rtsa_chunk_id(id: &[u8; 4]) -> bool {
        matches!(
            id,
            b"DSFH"
                | b"STRM"
                | b"SAMP"
                | b"SSTR"
                | b"SSCA"
                | b"ANTA"
                | b"MDTT"
                | b"SPRV"
                | b"STRT"
                | b"DSFT"
                | b"ANTS"
        )
    }

    /// Scan for SAMP chunks within a given range.
    fn scan_for_samp_chunks(
        reader: &mut std::io::BufReader<std::fs::File>,
        start_pos: u64,
        end_pos: u64,
        samp_chunk_offsets: &mut Vec<(u64, SampChunk)>,
    ) -> Result<()> {
        let file_len = reader.get_ref().metadata()?.len();
        let actual_end_pos = end_pos.min(file_len);
        reader.seek(SeekFrom::Start(start_pos))?;
        while reader.stream_position()? < actual_end_pos {
            let current_pos = reader.stream_position()?;
            match RtsaChunkHeader::read_from(reader) {
                Ok(header) => {
                    if header.size < 16 || !Self::is_valid_rtsa_chunk_id(&header.id) {
                        reader.seek(SeekFrom::Start(current_pos + 1))?;
                    } else {
                        let next_chunk_pos = current_pos + header.size as u64;
                        if &header.id == b"SAMP" {
                            let samp =
                                SampChunk::read_from(reader, header.size, header.header_size)?;
                            debug!(
                                "Found SAMP chunk at 0x{:08X}: stream_id={}, num_samples={}",
                                current_pos, samp.stream_id, samp.num_samples
                            );
                            samp_chunk_offsets.push((current_pos, samp));
                            reader.seek(SeekFrom::Start(next_chunk_pos))?;
                        } else {
                            reader.seek(SeekFrom::Start(next_chunk_pos))?;
                        }
                    }
                }
                Err(e) => {
                    debug!(
                        "scan_for_samp_chunks: failed to read header at offset {}: {:?}",
                        current_pos, e
                    );
                    reader.seek(SeekFrom::Start(current_pos + 1))?;
                }
            }
        }
        Ok(())
    }

    /// Read samples from raw IQ data section (reverse-order RTSA format)
    fn read_raw_iq_samples(&mut self, num_samples: usize) -> Result<Option<SampleData>> {
        // Calculate position in file for current sample index
        let sample_byte_offset = self.current_sample_index * 8; // 8 bytes per Complex32 (4 bytes I + 4 bytes Q)

        // Check if we've reached end of data
        if self.current_sample_index >= self.metadata.total_samples {
            return Ok(None);
        }

        // Calculate actual samples to read (don't exceed available data)
        let samples_to_read = std::cmp::min(
            num_samples as u64,
            self.metadata.total_samples - self.current_sample_index,
        ) as usize;

        if samples_to_read == 0 {
            return Ok(None);
        }

        // Seek to the position in the raw IQ data
        self.reader.seek(SeekFrom::Start(sample_byte_offset))?;

        // Read the raw float32 IQ data
        let mut samples = Vec::with_capacity(samples_to_read);
        for _ in 0..samples_to_read {
            let i = self.reader.read_f32::<LittleEndian>()?;
            let q = self.reader.read_f32::<LittleEndian>()?;
            samples.push(Complex32::new(i, q));
        }

        // Update current position
        self.current_sample_index += samples_to_read as u64;

        debug!(
            "Read {} raw IQ samples at position {}",
            samples_to_read,
            self.current_sample_index - samples_to_read as u64
        );

        Ok(Some(SampleData::Iq(samples)))
    }

    /// The comprehensive metadata parsed from this file's chunk headers.
    pub fn metadata(&self) -> &RtsaMetadata {
        &self.metadata
    }

    /// Get comprehensive antenna information from ANTA chunks
    pub fn antenna_info(&self) -> &[AntennaInfo] {
        &self.metadata.antennas
    }

    /// Get preview information from SPRV chunks
    pub fn preview_info(&self) -> &[PreviewInfo] {
        &self.metadata.previews
    }

    /// Get sub-stream information from SSTR chunks
    pub fn sub_stream_info(&self) -> &[SubStreamInfo] {
        &self.metadata.sub_streams
    }

    /// Get stream tail information from STRT chunk
    pub fn stream_tail_info(&self) -> Option<&StreamTailInfo> {
        self.metadata.stream_tail.as_ref()
    }

    /// Get metadata type definitions from MDTT chunks
    pub fn metadata_definitions(&self) -> &[(u64, MetaTypeDefinition)] {
        &self.metadata.metadata_definitions
    }

    /// Get comprehensive file structure information
    pub fn file_info(&self) -> (f64, u32, &str) {
        (
            self.metadata.creation_time,
            self.metadata.num_streams,
            &self.metadata.file_format_version,
        )
    }

    /// Get detailed stream information
    pub fn stream_info(&self) -> (u64, Option<&str>, Option<f32>, Option<f32>, f64) {
        (
            self.metadata.primary_stream_id,
            self.metadata.stream_type.as_deref(),
            self.metadata.stream_sample_rate,
            self.metadata.stream_center_frequency,
            self.metadata.stream_start_time,
        )
    }

    /// Get sample chunk statistics
    pub fn sample_chunk_stats(&self) -> (usize, u64) {
        (
            self.metadata.total_sample_chunks,
            self.metadata.sample_data_size,
        )
    }

    /// Check if this RTSA file contains antenna positioning data
    pub fn has_antenna_positioning(&self) -> bool {
        self.metadata
            .antennas
            .iter()
            .any(|ant| ant.latitude != 0.0 || ant.longitude != 0.0)
    }

    /// Check if this RTSA file contains preview/thumbnail data
    pub fn has_preview_data(&self) -> bool {
        !self.metadata.previews.is_empty()
    }

    /// Check if this RTSA file contains structured metadata definitions
    pub fn has_structured_metadata(&self) -> bool {
        !self.metadata.metadata_definitions.is_empty()
    }

    /// Get timing information in a human-readable format
    pub fn timing_info(&self) -> String {
        let start_seconds = self.metadata.start_time_ns as f64 / 1e9;

        if self.metadata.end_time_ns > 0 {
            let end_seconds = self.metadata.end_time_ns as f64 / 1e9;
            let duration = end_seconds - start_seconds;
            format!(
                "Start: {:.3}s, End: {:.3}s, Duration: {:.3}s",
                start_seconds, end_seconds, duration
            )
        } else if self.metadata.sample_rate > 0.0 && self.metadata.total_samples > 0 {
            let duration = self.metadata.total_samples as f64 / self.metadata.sample_rate;
            format!(
                "Start: {:.3}s, Duration: {:.3}s (calculated)",
                start_seconds, duration
            )
        } else {
            format!("Start: {:.3}s, Duration: unknown", start_seconds)
        }
    }

    /// Validate RTSA file structure and metadata consistency
    pub fn validate_structure(&self) -> Result<ValidationReport> {
        let mut warnings = Vec::new();
        let mut errors = Vec::new();

        // Validate core metadata consistency
        if self.metadata.sample_rate <= 0.0 && !self.metadata.sub_streams.is_empty() {
            warnings.push("Sample rate is unknown but sub-streams are present".to_string());
        }

        if self.metadata.center_frequency.is_none() && !self.metadata.sub_streams.is_empty() {
            warnings.push("Center frequency is unknown but sub-streams are present".to_string());
        }

        // Validate timing consistency
        if self.metadata.end_time_ns > 0 && self.metadata.end_time_ns <= self.metadata.start_time_ns
        {
            errors.push("End time is not after start time".to_string());
        }

        // Validate sample count consistency
        if self.metadata.total_samples > 0 && self.metadata.sample_data_size > 0 {
            let expected_bytes = self.metadata.total_samples * 8; // 8 bytes per Complex32
            let actual_bytes = self.metadata.sample_data_size;
            if (expected_bytes as i64 - actual_bytes as i64).abs() > (expected_bytes / 10) as i64 {
                warnings.push(format!(
                    "Sample count mismatch: expected {} bytes from {} samples, found {} bytes",
                    expected_bytes, self.metadata.total_samples, actual_bytes
                ));
            }
        }

        // Validate antenna data consistency
        for (i, antenna) in self.metadata.antennas.iter().enumerate() {
            if antenna.name.is_empty() {
                warnings.push(format!("Antenna {} has empty name", i));
            }
            if antenna.num_segments == 0 {
                warnings.push(format!("Antenna {} has zero segments", i));
            }
        }

        // Validate sub-stream data consistency
        for (i, sub_stream) in self.metadata.sub_streams.iter().enumerate() {
            if sub_stream.frequency_span <= 0.0 {
                errors.push(format!("Sub-stream {} has invalid frequency span", i));
            }
            if sub_stream.frequency_step <= 0.0 {
                warnings.push(format!("Sub-stream {} has invalid frequency step", i));
            }
            if sub_stream.name.is_empty() {
                warnings.push(format!("Sub-stream {} has empty name", i));
            }
        }

        // Validate preview data consistency
        for (i, preview) in self.metadata.previews.iter().enumerate() {
            if preview.preview_count as usize != preview.preview_offsets.len() {
                errors.push(format!("Preview {} count mismatch", i));
            }
            if preview.preview_offsets.len() != preview.preview_times.len() {
                errors.push(format!("Preview {} offset/time array size mismatch", i));
            }
        }

        // Validate stream tail consistency
        if let Some(tail) = &self.metadata.stream_tail {
            if tail.num_samples > 0 && tail.payload_size == 0 {
                warnings.push("Stream tail indicates samples but zero payload size".to_string());
            }
            if tail.end_time <= self.metadata.stream_start_time {
                errors.push("Stream tail end time is not after start time".to_string());
            }
        }

        Ok(ValidationReport {
            valid: errors.is_empty(),
            errors,
            warnings,
            metadata_completeness: self.calculate_metadata_completeness(),
        })
    }

    /// Calculate metadata completeness percentage
    fn calculate_metadata_completeness(&self) -> f32 {
        let mut score = 0;
        let mut total = 0;

        // Core timing and frequency (weight: 20)
        total += 20;
        if self.metadata.sample_rate > 0.0 {
            score += 5;
        }
        if self.metadata.center_frequency.is_some() {
            score += 5;
        }
        if self.metadata.bandwidth > 0.0 {
            score += 5;
        }
        if self.metadata.total_samples > 0 {
            score += 5;
        }

        // Timing information (weight: 15)
        total += 15;
        if self.metadata.start_time_ns > 0 {
            score += 8;
        }
        if self.metadata.end_time_ns > 0 {
            score += 7;
        }

        // Stream information (weight: 15)
        total += 15;
        if self.metadata.stream_type.is_some() {
            score += 5;
        }
        if self.metadata.stream_sample_rate.is_some() {
            score += 5;
        }
        if self.metadata.stream_center_frequency.is_some() {
            score += 5;
        }

        // Sub-streams (weight: 15)
        total += 15;
        if !self.metadata.sub_streams.is_empty() {
            score += 15;
        }

        // Antenna information (weight: 15)
        total += 15;
        if !self.metadata.antennas.is_empty() {
            score += 15;
        }

        // Preview data (weight: 10)
        total += 10;
        if !self.metadata.previews.is_empty() {
            score += 10;
        }

        // Stream tail (weight: 10)
        total += 10;
        if self.metadata.stream_tail.is_some() {
            score += 10;
        }

        (score as f32 / total as f32) * 100.0
    }

    /// Read up to `num_samples` samples from the stream, optionally
    /// restricted to a specific `sub_stream_id`. Returns `Ok(None)` at
    /// end of stream; a single call returns at most one chunk's worth of
    /// samples.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure or a malformed chunk.
    pub fn read_samples(
        &mut self,
        num_samples: usize,
        sub_stream_id: Option<u32>,
    ) -> Result<Option<SampleData>> {
        // Handle reverse-order RTSA format with raw IQ data
        if let Some(_raw_iq_end_offset) = self.raw_iq_data_end_offset {
            return self.read_raw_iq_samples(num_samples);
        }

        // Each iteration either returns data from the chunk containing the
        // cursor, or skips one unsupported-payload chunk and retries. A
        // single call returns at most one chunk's worth of samples.
        loop {
            // Scan for the chunk containing `current_sample_index`,
            // starting from the cursor left by the previous read. Chunks
            // are indexed in file order, so sequential reads resume where
            // they left off (the previous full rescan made sequential
            // playback O(chunks²) across a file). The wrap-around pass
            // covers backward seeks.
            let n_chunks = self.samp_chunk_offsets.len();
            let start_hint = self.chunk_scan_hint.min(n_chunks);
            let mut found_chunk = false;
            for idx in (start_hint..n_chunks).chain(0..start_hint) {
                let (offset, ref samp_chunk) = self.samp_chunk_offsets[idx];
                if samp_chunk.stream_id == self.iq_stream_id
                    && (sub_stream_id.is_none() || sub_stream_id == Some(samp_chunk.sub_stream_id))
                {
                    let chunk_start_sample =
                        *self.samp_chunk_start_samples.get(&offset).unwrap_or(&0);
                    if self.current_sample_index
                        < chunk_start_sample + samp_chunk.num_samples as u64
                        && self.current_sample_index >= chunk_start_sample
                    {
                        self.chunk_scan_hint = idx;
                        let samples_in_chunk_before =
                            (self.current_sample_index - chunk_start_sample) as u32;
                        let bytes_to_skip =
                            samples_in_chunk_before as u64 * samp_chunk.sample_size as u64;
                        let data_start_offset =
                            offset + samp_chunk.header.header_size as u64 + bytes_to_skip;
                        self.reader.seek(SeekFrom::Start(data_start_offset))?;

                        let remaining_in_chunk = samp_chunk.num_samples - samples_in_chunk_before;
                        let to_read = u32::try_from(num_samples)
                            .unwrap_or(u32::MAX)
                            .min(remaining_in_chunk);

                        let sample_data = match samp_chunk.payload_type {
                            DspStreamPayloadType::DsptIq => {
                                match samp_chunk.sample_type {
                                    DspStreamSampleType::DsStF32
                                    | DspStreamSampleType::DsStF32N => {
                                        if samp_chunk.compression > 0 {
                                            // In-band decode of compressed DSPT_IQ is
                                            // not possible: the format is proprietary
                                            // and the native decoder rejects it.
                                            // `RtsaSource::open` transparently reroutes
                                            // compressed files through RTSAFileTool, so
                                            // reaching this branch means the source was
                                            // constructed some other way.
                                            return Err(Error::FileFormat {
                                                offset: 0,
                                                reason: format!(
                                                    "compressed DSPT_IQ chunk at 0x{:08X}: in-band \
                                                 decompression is unsupported (proprietary \
                                                 format); open the file via RtsaSource::open, \
                                                 which decompresses through RTSAFileTool",
                                                    offset
                                                ),
                                            });
                                        }
                                        let mut samples = Vec::with_capacity(to_read as usize);
                                        for _ in 0..to_read {
                                            samples.push(Complex32::new(
                                                self.reader.read_f32::<LittleEndian>()?,
                                                self.reader.read_f32::<LittleEndian>()?,
                                            ));
                                        }
                                        Some(SampleData::Iq(samples))
                                    }
                                    DspStreamSampleType::DsStS16
                                    | DspStreamSampleType::DsStS16N => {
                                        // int16 IQ samples decode as
                                        // f32 = scale * raw_i16, where the scale factor
                                        // for a sub-stream is derived from its declared
                                        // value range (mValueMinimum / mValueMaximum) so
                                        // ±32768 maps to that range. When the SSTR
                                        // chunk reports a zero range, fall back to the
                                        // ADC-normalised default 1 / 32768.
                                        let scale = self
                                            .int16_scale_for_sub_stream(samp_chunk.sub_stream_id);
                                        let mut samples = Vec::with_capacity(to_read as usize);
                                        for _ in 0..to_read {
                                            let i_raw =
                                                self.reader.read_i16::<LittleEndian>()? as f32;
                                            let q_raw =
                                                self.reader.read_i16::<LittleEndian>()? as f32;
                                            samples
                                                .push(Complex32::new(i_raw * scale, q_raw * scale));
                                        }
                                        Some(SampleData::Iq(samples))
                                    }
                                    _ => None, // Unsupported sample type for IQ
                                }
                            }
                            DspStreamPayloadType::DsptSpectra => {
                                match samp_chunk.sample_type {
                                    DspStreamSampleType::DsStF32
                                    | DspStreamSampleType::DsStF32N => {
                                        let mut samples = Vec::with_capacity(to_read as usize);
                                        for _ in 0..to_read {
                                            samples.push(self.reader.read_f32::<LittleEndian>()?);
                                        }
                                        Some(SampleData::Spectra(samples))
                                    }
                                    _ => None, // Unsupported sample type for Spectra
                                }
                            }
                            _ => {
                                // Skip other payload types for now
                                self.reader.seek(SeekFrom::Start(
                                    offset + samp_chunk.header.size as u64,
                                ))?;
                                None
                            }
                        };

                        if sample_data.is_some() {
                            self.current_sample_index += to_read as u64;
                            return Ok(sample_data);
                        } else {
                            // Unsupported payload/sample type: skip the
                            // whole chunk and try the next one.
                            self.current_sample_index =
                                chunk_start_sample + samp_chunk.num_samples as u64;
                        }

                        found_chunk = true;
                        break;
                    }
                }
            }
            if !found_chunk {
                return Ok(None);
            }
        }
    }

    /// Per-stream int16 scale factor.
    ///
    /// The factor is derived from the sub-stream's declared value range
    /// (`mValueMinimum`, `mValueMaximum`) so that ±32768 maps to it. If the
    /// range is unset (both zero), fall back to 1 / 32768.0 — the standard
    /// ADC-normalised int16 mapping.
    fn int16_scale_for_sub_stream(&self, sub_stream_id: u32) -> f32 {
        const FALLBACK: f32 = 1.0 / 32768.0;
        let info = self
            .metadata
            .sub_streams
            .iter()
            .find(|s| s.sub_stream_id == sub_stream_id);
        match info {
            Some(s) => {
                let max_abs = s.value_minimum.abs().max(s.value_maximum.abs());
                if max_abs.is_finite() && max_abs > 0.0 {
                    (max_abs / 32768.0) as f32
                } else {
                    FALLBACK
                }
            }
            None => FALLBACK,
        }
    }

    /// The current read cursor, in samples from the start of the stream.
    pub fn current_position(&self) -> u64 {
        self.current_sample_index
    }

    /// Total number of samples in the stream, per the file's metadata.
    pub fn total_samples(&self) -> u64 {
        self.metadata.total_samples
    }

    /// Remaining samples.
    pub fn remaining_samples(&self) -> u64 {
        // Saturating: skipping unsupported payload chunks can advance the
        // cursor past `total_samples` on mixed-payload files; that must
        // read as "0 remaining", not a debug panic / release wraparound.
        self.metadata
            .total_samples
            .saturating_sub(self.current_sample_index)
    }

    /// Whether the read cursor has reached the end of the stream.
    pub fn is_eof(&self) -> bool {
        self.current_sample_index >= self.metadata.total_samples
    }

    /// Seek the read cursor to an absolute sample index.
    ///
    /// # Errors
    ///
    /// Returns an error if `sample_index` is out of bounds or does not
    /// fall within any known SAMP chunk.
    pub fn seek_to_sample(&mut self, sample_index: u64) -> Result<()> {
        if sample_index >= self.metadata.total_samples {
            return Err(Error::FileFormat {
                offset: 0,
                reason: "Seek position out of bounds.".to_string(),
            });
        }
        let mut current_total_samples = 0;
        for &(offset, ref samp_chunk) in &self.samp_chunk_offsets {
            if samp_chunk.stream_id == self.iq_stream_id {
                let chunk_end_sample = current_total_samples + samp_chunk.num_samples as u64;
                if (current_total_samples..chunk_end_sample).contains(&sample_index) {
                    let samples_into_chunk = sample_index - current_total_samples;
                    let byte_offset = samples_into_chunk * samp_chunk.sample_size as u64;
                    let data_start = offset + samp_chunk.header.header_size as u64;
                    self.reader
                        .seek(SeekFrom::Start(data_start + byte_offset))?;
                    self.current_sample_index = sample_index;
                    // The next read must rescan from the start: the seek
                    // may have moved backwards past the cursor hint.
                    self.chunk_scan_hint = 0;
                    return Ok(());
                }
                current_total_samples = chunk_end_sample;
            }
        }
        Err(Error::FileFormat {
            offset: 0,
            reason: "Could not find sample index in any SAMP chunk.".to_string(),
        })
    }

    /// Seek the read cursor back to the first sample of the stream.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure while seeking the underlying file.
    pub fn reset(&mut self) -> Result<()> {
        self.current_sample_index = 0;
        self.chunk_scan_hint = 0;
        if let Some((offset, samp_chunk)) = self
            .samp_chunk_offsets
            .iter()
            .find(|(_, s)| s.stream_id == self.iq_stream_id)
        {
            self.reader.seek(SeekFrom::Start(
                offset + samp_chunk.header.header_size as u64,
            ))?;
        } else {
            self.reader.seek(SeekFrom::Start(0))?;
        }
        Ok(())
    }

    /// Parse a backward-linked list of SSTR chunks.
    ///
    /// Real captures occasionally have a chain that runs past the
    /// end-of-file (typically because the recording was truncated mid-
    /// flight). We treat any IO-error from the chunk read as "end of
    /// chain" instead of bailing out of the whole parse — every
    /// upstream caller already tolerates a missing/short SSTR list.
    fn parse_sstr_chain<R: Read + Seek>(
        reader: &mut R,
        mut chain_offset: u64,
        sstr_chunks: &mut HashMap<u32, SstrChunk>,
    ) -> Result<()> {
        let mut visited = std::collections::HashSet::new();
        while chain_offset > 0 && visited.insert(chain_offset) {
            if reader.seek(SeekFrom::Start(chain_offset)).is_err() {
                break;
            }
            let header = match RtsaChunkHeader::read_from(reader) {
                Ok(h) => h,
                Err(e) => {
                    debug!(
                        "SSTR chain: header read failed at offset {}: {:?}; treating as end of chain",
                        chain_offset, e
                    );
                    break;
                }
            };
            if header.id != *b"SSTR" {
                debug!(
                    "SSTR chain: unexpected chunk id {:?} at offset {}; treating as end of chain",
                    std::str::from_utf8(&header.id),
                    chain_offset
                );
                break;
            }
            let sstr = match SstrChunk::read_from(reader, header.size) {
                Ok(s) => s,
                Err(e) => {
                    debug!(
                        "SSTR chain: chunk read failed at offset {}: {:?}; treating as end of chain",
                        chain_offset, e
                    );
                    break;
                }
            };
            chain_offset = if sstr.sub_stream_offset > 0 {
                sstr.sub_stream_offset as u64
            } else {
                0
            };
            sstr_chunks.insert(sstr.sub_stream_id, sstr);
        }
        Ok(())
    }

    /// Parse a backward-linked list of ANTA chunks. See
    /// [`Self::parse_sstr_chain`] for the chain-truncation rationale.
    fn parse_anta_chain<R: Read + Seek>(
        reader: &mut R,
        mut chain_offset: u64,
        anta_chunks: &mut HashMap<u64, AntaChunk>,
    ) -> Result<()> {
        let mut visited = std::collections::HashSet::new();
        while chain_offset > 0 && visited.insert(chain_offset) {
            if reader.seek(SeekFrom::Start(chain_offset)).is_err() {
                break;
            }
            let header = match RtsaChunkHeader::read_from(reader) {
                Ok(h) => h,
                Err(_) => break,
            };
            if header.id != *b"ANTA" {
                break;
            }
            let anta = match AntaChunk::read_from(reader, header.size) {
                Ok(a) => a,
                Err(_) => break,
            };
            chain_offset = if anta.antenna_offset > 0 {
                anta.antenna_offset as u64
            } else {
                0
            };
            anta_chunks.insert(anta.antenna_id, anta);
        }
        Ok(())
    }

    /// Parse a backward-linked list of MDTT chunks. See
    /// [`Self::parse_sstr_chain`] for the chain-truncation rationale.
    fn parse_mdtt_chain<R: Read + Seek>(
        reader: &mut R,
        mut chain_offset: u64,
        mdtt_chunks: &mut HashMap<u64, MdttChunk>,
    ) -> Result<()> {
        let mut visited = std::collections::HashSet::new();
        while chain_offset > 0 && visited.insert(chain_offset) {
            if reader.seek(SeekFrom::Start(chain_offset)).is_err() {
                break;
            }
            let header = match RtsaChunkHeader::read_from(reader) {
                Ok(h) => h,
                Err(_) => break,
            };
            if header.id != *b"MDTT" {
                break;
            }
            let mdtt = match MdttChunk::read_from(reader, header.size) {
                Ok(m) => m,
                Err(_) => break,
            };
            chain_offset = if mdtt.metadata_offset > 0 {
                mdtt.metadata_offset as u64
            } else {
                0
            };
            mdtt_chunks.insert(mdtt.metadata_id, mdtt);
        }
        Ok(())
    }

    /// Parse a tree of SPRV chunks. See [`Self::parse_sstr_chain`] for the
    /// chain-truncation rationale.
    fn parse_sprv_tree<R: Read + Seek>(
        reader: &mut R,
        mut tree_offset: u64,
        sprv_chunks: &mut Vec<SprvChunk>,
    ) -> Result<()> {
        let mut visited = std::collections::HashSet::new();
        while tree_offset > 0 && visited.insert(tree_offset) {
            if reader.seek(SeekFrom::Start(tree_offset)).is_err() {
                break;
            }
            let header = match RtsaChunkHeader::read_from(reader) {
                Ok(h) => h,
                Err(_) => break,
            };
            if header.id != *b"SPRV" {
                break;
            }
            let sprv = match SprvChunk::read_from(reader, header.size) {
                Ok(s) => s,
                Err(_) => break,
            };
            tree_offset = sprv
                .preview_offsets
                .iter()
                .find(|&&o| o > 0)
                .map_or(0, |&o| o as u64);
            sprv_chunks.push(sprv);
        }
        Ok(())
    }
}

/// S8: Maximum allowed chunk size to prevent malicious files from causing OOM
const MAX_RTSA_CHUNK_SIZE: u32 = 1_000_000_000; // 1 GB

// Helper traits for reading chunks from a reader.
impl RtsaChunkHeader {
    fn read_from<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        let mut id = [0u8; 4];
        reader.read_exact(&mut id)?;
        let size = reader.read_u32::<LittleEndian>()?;
        // S8: Validate chunk size to prevent malicious files from triggering huge allocations
        if size > MAX_RTSA_CHUNK_SIZE {
            return Err(Error::FileFormat {
                offset: 0,
                reason: format!(
                    "RTSA chunk '{}' size {} exceeds maximum ({})",
                    String::from_utf8_lossy(&id),
                    size,
                    MAX_RTSA_CHUNK_SIZE
                ),
            });
        }
        Ok(RtsaChunkHeader {
            id,
            size,
            flags: reader.read_u32::<LittleEndian>()?,
            version: reader.read_u16::<LittleEndian>()?,
            header_size: reader.read_u16::<LittleEndian>()?,
        })
    }
}

impl DsfhChunk {
    fn read_from<R: Read + Seek>(reader: &mut R, _size: u32) -> Result<Self> {
        Ok(DsfhChunk {
            header: RtsaChunkHeader {
                id: *b"DSFH",
                size: _size,
                flags: 0,
                version: 1,
                header_size: 8,
            },
            creation_time: reader.read_f64::<LittleEndian>()?,
        })
    }
}

impl StrmChunk {
    fn read_from<R: Read + Seek>(reader: &mut R, _size: u32) -> Result<Self> {
        if _size == 40 {
            let stream_id = reader.read_u32::<LittleEndian>()? as u64;
            let stream_type = reader.read_u32::<LittleEndian>()?;
            let _reserved1 = reader.read_u32::<LittleEndian>()?;
            let sample_rate = reader.read_f32::<LittleEndian>()?;
            let mut reserved2 = [0u8; 8];
            reader.read_exact(&mut reserved2)?;
            let center_frequency = reader.read_f32::<LittleEndian>()?;
            let mut device_name = [0u8; 8];
            reader.read_exact(&mut device_name)?;
            Ok(StrmChunk {
                header: RtsaChunkHeader {
                    id: *b"STRM",
                    size: _size,
                    flags: 0,
                    version: 1,
                    header_size: 40,
                },
                stream_id,
                start_time: 0.0,
                stream_offset: 0,
                stream_type: Some(stream_type),
                sample_rate: Some(sample_rate),
                center_frequency: Some(center_frequency),
                device_name: Some(device_name),
            })
        } else {
            Ok(StrmChunk {
                header: RtsaChunkHeader {
                    id: *b"STRM",
                    size: _size,
                    flags: 0,
                    version: 1,
                    header_size: 24,
                },
                stream_id: reader.read_u64::<LittleEndian>()?,
                start_time: reader.read_f64::<LittleEndian>()?,
                stream_offset: reader.read_i64::<LittleEndian>()?,
                stream_type: None,
                sample_rate: None,
                center_frequency: None,
                device_name: None,
            })
        }
    }
}

impl SampChunk {
    fn read_from<R: Read + Seek>(reader: &mut R, _size: u32, header_size: u16) -> Result<Self> {
        let stream_id = reader.read_u64::<LittleEndian>()?;
        let sub_stream_id = reader.read_u32::<LittleEndian>()?;
        let sample_type = DspStreamSampleType::try_from(reader.read_u8()?)?;
        let sample_unit = DspStreamSampleUnit::try_from(reader.read_u8()?)?;
        let payload_type = DspStreamPayloadType::try_from(reader.read_u8()?)?;
        let compression = reader.read_i8()?;
        let packet_start_time = reader.read_f64::<LittleEndian>()?;
        let packet_end_time = reader.read_f64::<LittleEndian>()?;
        let packet_flags = DspPacketFlags::from_bits_truncate(reader.read_u32::<LittleEndian>()?);
        let sample_size = reader.read_u32::<LittleEndian>()?;
        let sample_depth = reader.read_u32::<LittleEndian>()?;
        let num_samples = reader.read_u32::<LittleEndian>()?;

        let remaining = _size as i64 - 64;
        if remaining > 0 {
            reader.seek(SeekFrom::Current(remaining))?;
        }

        Ok(SampChunk {
            header: RtsaChunkHeader {
                id: *b"SAMP",
                size: _size,
                flags: packet_flags.bits(),
                version: 1,
                header_size,
            },
            stream_id,
            sub_stream_id,
            sample_type,
            sample_unit,
            payload_type,
            compression,
            packet_start_time,
            packet_end_time,
            packet_flags,
            sample_size,
            sample_depth,
            num_samples,
        })
    }
}

impl SstrChunk {
    fn read_from<R: Read + Seek>(reader: &mut R, _size: u32) -> Result<Self> {
        const HEADER_SIZE: u32 = 224;
        let stream_id = reader.read_u64::<LittleEndian>()?;
        let sub_stream_id = reader.read_u32::<LittleEndian>()?;
        reader.seek(SeekFrom::Current(4))?;
        let sub_stream_offset = reader.read_i64::<LittleEndian>()?;
        let frequency_start = reader.read_f64::<LittleEndian>()?;
        let frequency_step = reader.read_f64::<LittleEndian>()?;
        let frequency_span = reader.read_f64::<LittleEndian>()?;
        let value_minimum = reader.read_f64::<LittleEndian>()?;
        let value_maximum = reader.read_f64::<LittleEndian>()?;
        let direction = reader.read_f64::<LittleEndian>()?;
        let antenna_index = reader.read_u32::<LittleEndian>()?;
        let num_categories = reader.read_u32::<LittleEndian>()?;
        let mut name = [0u8; 128];
        reader.read_exact(&mut name)?;
        let antenna_id = reader.read_u64::<LittleEndian>()?;
        let metadata_id = reader.read_u64::<LittleEndian>()?;

        let remaining = _size as i64 - HEADER_SIZE as i64 - 16;
        if remaining > 0 {
            reader.seek(SeekFrom::Current(remaining))?;
        }

        Ok(SstrChunk {
            header: RtsaChunkHeader {
                id: *b"SSTR",
                size: _size,
                flags: 0,
                version: 1,
                header_size: HEADER_SIZE as u16,
            },
            stream_id,
            sub_stream_id,
            sub_stream_offset,
            frequency_start,
            frequency_step,
            frequency_span,
            value_minimum,
            value_maximum,
            direction,
            antenna_index,
            num_categories,
            name,
            antenna_id,
            metadata_id,
        })
    }
}

impl SscaChunk {
    /// Parse an SSCA (Sub-Stream Category, FILESPEC.md §6) chunk
    /// payload from `reader`. SSCA chunks describe named scalar
    /// measurements within a category sub-stream (e.g. channel
    /// power, peak hold) and embed an RGBA tint used by the RTSA
    /// GUI for plotting.
    ///
    /// Not yet wired into the SSTR chunk walker — keep this around as
    /// the canonical wire-format encoding for future callers. The
    /// dead-code lint is held off by the round-trip test
    /// `test_ssca_chunk_read_from_round_trip` in this module.
    pub fn read_from<R: Read + Seek>(reader: &mut R, size: u32) -> Result<Self> {
        let mut name = [0u8; 128];
        reader.read_exact(&mut name)?;
        Ok(SscaChunk {
            header: RtsaChunkHeader {
                id: *b"SSCA",
                size,
                flags: 0,
                version: 1,
                header_size: 152,
            },
            name,
            flags: DsscfFlags::from_bits_truncate(reader.read_u32::<LittleEndian>()?),
            red: reader.read_u8()?,
            green: reader.read_u8()?,
            blue: reader.read_u8()?,
            alpha: reader.read_u8()?,
            start_frequency: reader.read_f64::<LittleEndian>()?,
            end_frequency: reader.read_f64::<LittleEndian>()?,
        })
    }
}

impl AntaChunk {
    fn read_from<R: Read + Seek>(reader: &mut R, _size: u32) -> Result<Self> {
        const HEADER_SIZE: u32 = 244;
        let antenna_id = reader.read_u64::<LittleEndian>()?;
        let antenna_offset = reader.read_i64::<LittleEndian>()?;
        let mut name = [0u8; 128];
        reader.read_exact(&mut name)?;
        let latitude = reader.read_f64::<LittleEndian>()?;
        let longitude = reader.read_f64::<LittleEndian>()?;
        let flags = DspafFlags::from_bits_truncate(reader.read_u32::<LittleEndian>()?);
        let num_segments = reader.read_u32::<LittleEndian>()?;
        let mut transform = [[0f32; 4]; 4];
        for row in &mut transform {
            for cell in row.iter_mut() {
                *cell = reader.read_f32::<LittleEndian>()?;
            }
        }
        let mut antenna_uuid = [0u8; 16];
        reader.read_exact(&mut antenna_uuid)?;

        let remaining = _size as i64 - HEADER_SIZE as i64 - 16;
        if remaining > 0 {
            reader.seek(SeekFrom::Current(remaining))?;
        }

        Ok(AntaChunk {
            header: RtsaChunkHeader {
                id: *b"ANTA",
                size: _size,
                flags: 0,
                version: 1,
                header_size: HEADER_SIZE as u16,
            },
            antenna_id,
            antenna_offset,
            name,
            latitude,
            longitude,
            flags,
            num_segments,
            transform,
            antenna_uuid,
        })
    }
}

impl AntsChunk {
    /// Parse an ANTS (Antenna Segment, FILESPEC.md) chunk payload
    /// from `reader`. ANTS chunks describe sub-segments of a
    /// composite multi-element antenna — each segment carries a
    /// name, a 4-element orientation quaternion, and a numeric id.
    ///
    /// Not yet wired into the ANTA chunk walker — the parser stays
    /// here as the canonical wire-format encoding for future
    /// callers. Lint pressure held off by
    /// `test_ants_chunk_read_from_round_trip` in this module.
    pub fn read_from<R: Read + Seek>(reader: &mut R, size: u32) -> Result<Self> {
        let mut name = [0u8; 128];
        reader.read_exact(&mut name)?;
        let mut orientation = [0f32; 4];
        for cell in &mut orientation {
            *cell = reader.read_f32::<LittleEndian>()?;
        }
        Ok(AntsChunk {
            header: RtsaChunkHeader {
                id: *b"ANTS",
                size,
                flags: 0,
                version: 1,
                header_size: 148,
            },
            name,
            orientation,
            id: reader.read_u32::<LittleEndian>()?,
        })
    }
}

impl MdttChunk {
    /// Maximum size of an MDTT type-definition payload. The general chunk
    /// cap is 1 GB (sized for SAMP data); metadata definitions are tiny in
    /// practice, and the declared size is untrusted — without a tighter
    /// cap a crafted header forces a huge up-front allocation here.
    const MAX_MDTT_PAYLOAD: i64 = 16 * 1024 * 1024;

    fn read_from<R: Read + Seek>(reader: &mut R, size: u32) -> Result<Self> {
        const HEADER_SIZE: u32 = 16;
        let metadata_id = reader.read_u64::<LittleEndian>()?;
        let metadata_offset = reader.read_i64::<LittleEndian>()?;

        let payload_size = size as i64 - HEADER_SIZE as i64 - 16; // 16 bytes for metadata_id and metadata_offset
        if payload_size > Self::MAX_MDTT_PAYLOAD {
            return Err(Error::FileFormat {
                offset: 0,
                reason: format!(
                    "MDTT chunk declares a {} byte type-definition payload, exceeding the {} cap \
                 (corrupt or malicious file)",
                    payload_size,
                    Self::MAX_MDTT_PAYLOAD
                ),
            });
        }
        let definition = if payload_size > 0 {
            let mut payload_buffer = vec![0u8; payload_size as usize];
            reader.read_exact(&mut payload_buffer)?;
            let mut payload_reader = std::io::Cursor::new(payload_buffer);
            Some(Self::read_meta_type_definition(&mut payload_reader)?)
        } else {
            None
        };

        Ok(MdttChunk {
            header: RtsaChunkHeader {
                id: *b"MDTT",
                size,
                flags: 0,
                version: 1,
                header_size: HEADER_SIZE as u16,
            },
            metadata_id,
            metadata_offset,
            definition,
        })
    }
}

impl MdttChunk {
    /// Maximum nesting depth for MDTT type definitions. Real definitions
    /// are a handful of levels deep; the limit exists because the payload
    /// is untrusted — an unbounded recursion here lets a crafted file
    /// overflow the stack and abort the process (a chunk-size-capped 1 GB
    /// payload can encode millions of nesting levels).
    const MAX_META_TYPE_DEPTH: u32 = 32;

    /// Maximum number of fields in a single `MtObject` definition. The
    /// wire `count` is a raw `u32`; without a cap a crafted value forces
    /// up to 4 billion element-read iterations before the payload runs
    /// dry.
    const MAX_META_TYPE_ELEMENTS: u32 = 4096;

    fn read_meta_type_definition<R: Read + Seek>(reader: &mut R) -> Result<MetaTypeDefinition> {
        Self::read_meta_type_definition_at_depth(reader, 0)
    }

    fn read_meta_type_definition_at_depth<R: Read + Seek>(
        reader: &mut R,
        depth: u32,
    ) -> Result<MetaTypeDefinition> {
        if depth > Self::MAX_META_TYPE_DEPTH {
            return Err(Error::FileFormat {
                offset: 0,
                reason: format!(
                    "MDTT type definition exceeds maximum nesting depth {} (corrupt or malicious file)",
                    Self::MAX_META_TYPE_DEPTH
                ),
            });
        }

        let id = reader.read_u64::<LittleEndian>()?;
        let meta_type_byte = reader.read_u8()?;
        let meta_type = MetaType::try_from(meta_type_byte)?;
        let flags_raw = reader.read_u32::<LittleEndian>()?;
        let flags = MetaTypeFlags::from_bits_truncate(flags_raw);
        let count = reader.read_u32::<LittleEndian>()?;

        let mut elements = Vec::new();
        if meta_type == MetaType::MtObject
            || meta_type == MetaType::MtArray
            || meta_type == MetaType::MtVector
        {
            // For composite types, read elements recursively
            let num_elements_to_read = if meta_type == MetaType::MtObject {
                if count > Self::MAX_META_TYPE_ELEMENTS {
                    return Err(Error::FileFormat {
                        offset: 0,
                        reason: format!(
                            "MDTT object declares {} fields, exceeding the {} cap \
                         (corrupt or malicious file)",
                            count,
                            Self::MAX_META_TYPE_ELEMENTS
                        ),
                    });
                }
                count
            } else {
                1
            };
            for _ in 0..num_elements_to_read {
                // Read element name (128 bytes) and flags (u32)
                let mut name_bytes = [0u8; 128];
                reader.read_exact(&mut name_bytes)?;
                let name = String::from_utf8_lossy(&name_bytes)
                    .trim_end_matches('\0')
                    .to_string();
                let element_flags = reader.read_u32::<LittleEndian>()?;

                let element_definition =
                    Self::read_meta_type_definition_at_depth(reader, depth + 1)?;
                elements.push(MetaTypeElement {
                    name,
                    flags: element_flags,
                    definition: element_definition,
                });
            }
        }

        Ok(MetaTypeDefinition {
            id,
            meta_type,
            flags,
            count,
            elements,
        })
    }
}

impl SprvChunk {
    fn read_from<R: Read + Seek>(reader: &mut R, _size: u32) -> Result<Self> {
        const HEADER_SIZE: u32 = 390;
        let preview_level = reader.read_u8()?;
        let preview_count = reader.read_u8()?;
        reader.seek(SeekFrom::Current(6))?;
        let mut preview_offsets = [0i64; 16];
        for slot in &mut preview_offsets {
            *slot = reader.read_i64::<LittleEndian>()?;
        }
        let mut preview_times = [0f64; 16];
        for slot in &mut preview_times {
            *slot = reader.read_f64::<LittleEndian>()?;
        }
        let mut preview_samples = [0u64; 16];
        for slot in &mut preview_samples {
            *slot = reader.read_u64::<LittleEndian>()?;
        }

        let remaining = _size as i64 - HEADER_SIZE as i64 - 16;
        if remaining > 0 {
            reader.seek(SeekFrom::Current(remaining))?;
        }

        Ok(SprvChunk {
            header: RtsaChunkHeader {
                id: *b"SPRV",
                size: _size,
                flags: 0,
                version: 1,
                header_size: HEADER_SIZE as u16,
            },
            preview_level,
            preview_count,
            preview_offsets,
            preview_times,
            preview_samples,
        })
    }
}

impl StrtChunk {
    fn read_from<R: Read + Seek>(reader: &mut R, _size: u32) -> Result<Self> {
        Ok(StrtChunk {
            header: RtsaChunkHeader {
                id: *b"STRT",
                size: _size,
                flags: 0,
                version: 1,
                header_size: 80,
            },
            stream_offset: reader.read_i64::<LittleEndian>()?,
            sub_stream_offset: reader.read_i64::<LittleEndian>()?,
            preview_offset: reader.read_i64::<LittleEndian>()?,
            num_samples: reader.read_u64::<LittleEndian>()?,
            payload_size: reader.read_u64::<LittleEndian>()?,
            preview_levels: reader.read_u32::<LittleEndian>()?,
            num_previews: reader.read_u32::<LittleEndian>()?,
            num_preview_segments: reader.read_u32::<LittleEndian>()?,
            end_time: reader.read_f64::<LittleEndian>()?,
            antenna_offset: reader.read_i64::<LittleEndian>()?,
            metadata_offset: reader.read_i64::<LittleEndian>()?,
        })
    }
}

impl DsftChunk {
    fn read_from<R: Read + Seek>(reader: &mut R, _size: u32) -> Result<Self> {
        Ok(DsftChunk {
            header: RtsaChunkHeader {
                id: *b"DSFT",
                size: _size,
                flags: 0,
                version: 1,
                header_size: 20,
            },
            completion_time: reader.read_f64::<LittleEndian>()?,
            stream_offset: reader.read_u64::<LittleEndian>()?,
            num_streams: reader.read_u32::<LittleEndian>()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // Test RTSA chunk header parsing
    #[test]
    fn test_rtsa_chunk_header_parsing() {
        let data = vec![
            // DSFH chunk ID
            b'D', b'S', b'F', b'H', // Size (little-endian): 24 bytes
            24, 0, 0, 0, // Flags: 0x00000001
            1, 0, 0, 0, // Version: 1
            1, 0, // Header size: 16
            16, 0,
        ];

        let mut cursor = Cursor::new(data);
        let header = RtsaChunkHeader::read_from(&mut cursor).unwrap();

        assert_eq!(&header.id, b"DSFH");
        assert_eq!(header.size, 24);
        assert_eq!(header.flags, 1);
        assert_eq!(header.version, 1);
        assert_eq!(header.header_size, 16);
    }

    // Test DspStreamSampleType enum conversion
    #[test]
    fn test_dsp_stream_sample_type_conversion() {
        assert_eq!(
            DspStreamSampleType::try_from(0).unwrap(),
            DspStreamSampleType::DsStU8
        );
        assert_eq!(
            DspStreamSampleType::try_from(1).unwrap(),
            DspStreamSampleType::DsStU16
        );
        assert_eq!(
            DspStreamSampleType::try_from(2).unwrap(),
            DspStreamSampleType::DsStU32
        );
        assert_eq!(
            DspStreamSampleType::try_from(3).unwrap(),
            DspStreamSampleType::DsStS16
        );
        assert_eq!(
            DspStreamSampleType::try_from(4).unwrap(),
            DspStreamSampleType::DsStS32
        );
        assert_eq!(
            DspStreamSampleType::try_from(5).unwrap(),
            DspStreamSampleType::DsStF32
        );
        assert_eq!(
            DspStreamSampleType::try_from(6).unwrap(),
            DspStreamSampleType::DsStU8N
        );
        assert_eq!(
            DspStreamSampleType::try_from(7).unwrap(),
            DspStreamSampleType::DsStU16N
        );
        assert_eq!(
            DspStreamSampleType::try_from(8).unwrap(),
            DspStreamSampleType::DsStS16N
        );
        assert!(DspStreamSampleType::try_from(255).is_err());
    }

    // Test DspStreamSampleUnit enum conversion
    #[test]
    fn test_dsp_stream_sample_unit_conversion() {
        assert_eq!(
            DspStreamSampleUnit::try_from(0).unwrap(),
            DspStreamSampleUnit::DssuGeneric
        );
        assert_eq!(
            DspStreamSampleUnit::try_from(1).unwrap(),
            DspStreamSampleUnit::DssuDbm
        );
        assert_eq!(
            DspStreamSampleUnit::try_from(2).unwrap(),
            DspStreamSampleUnit::DssuDbmHz
        );
        assert_eq!(
            DspStreamSampleUnit::try_from(3).unwrap(),
            DspStreamSampleUnit::DssuPercentage
        );
        assert_eq!(
            DspStreamSampleUnit::try_from(4).unwrap(),
            DspStreamSampleUnit::DssuHz
        );
        assert_eq!(
            DspStreamSampleUnit::try_from(5).unwrap(),
            DspStreamSampleUnit::DssuWatt
        );
        assert_eq!(
            DspStreamSampleUnit::try_from(6).unwrap(),
            DspStreamSampleUnit::DssuVolt
        );
        assert_eq!(
            DspStreamSampleUnit::try_from(7).unwrap(),
            DspStreamSampleUnit::DssuTime
        );
        assert_eq!(
            DspStreamSampleUnit::try_from(8).unwrap(),
            DspStreamSampleUnit::DssuDateTime
        );
        assert!(DspStreamSampleUnit::try_from(255).is_err());
    }

    // Test DspStreamPayloadType enum conversion
    #[test]
    fn test_dsp_stream_payload_type_conversion() {
        assert_eq!(
            DspStreamPayloadType::try_from(0).unwrap(),
            DspStreamPayloadType::DsptGeneric
        );
        assert_eq!(
            DspStreamPayloadType::try_from(1).unwrap(),
            DspStreamPayloadType::DsptAudio
        );
        assert_eq!(
            DspStreamPayloadType::try_from(2).unwrap(),
            DspStreamPayloadType::DsptIq
        );
        assert_eq!(
            DspStreamPayloadType::try_from(3).unwrap(),
            DspStreamPayloadType::DsptSpectra
        );
        assert_eq!(
            DspStreamPayloadType::try_from(4).unwrap(),
            DspStreamPayloadType::DsptDetection
        );
        assert_eq!(
            DspStreamPayloadType::try_from(5).unwrap(),
            DspStreamPayloadType::DsptHistogram
        );
        assert_eq!(
            DspStreamPayloadType::try_from(6).unwrap(),
            DspStreamPayloadType::DsptStructured
        );
        assert_eq!(
            DspStreamPayloadType::try_from(7).unwrap(),
            DspStreamPayloadType::DsptImage
        );
        assert!(DspStreamPayloadType::try_from(255).is_err());
    }

    // Test MetaType enum conversion
    #[test]
    fn test_meta_type_conversion() {
        assert_eq!(MetaType::try_from(0).unwrap(), MetaType::MtNone);
        assert_eq!(MetaType::try_from(1).unwrap(), MetaType::MtBool);
        assert_eq!(MetaType::try_from(2).unwrap(), MetaType::MtInteger);
        assert_eq!(MetaType::try_from(3).unwrap(), MetaType::MtFloat);
        assert_eq!(MetaType::try_from(4).unwrap(), MetaType::MtString);
        assert_eq!(MetaType::try_from(5).unwrap(), MetaType::MtVector);
        assert_eq!(MetaType::try_from(6).unwrap(), MetaType::MtArray);
        assert_eq!(MetaType::try_from(7).unwrap(), MetaType::MtObject);
        assert!(MetaType::try_from(255).is_err()); // Invalid value should error
    }

    // Test DspPacketFlags bitflag operations
    #[test]
    fn test_dsp_packet_flags() {
        let flags = DspPacketFlags::STREAM_START | DspPacketFlags::PACKET_START;
        assert!(flags.contains(DspPacketFlags::STREAM_START));
        assert!(flags.contains(DspPacketFlags::PACKET_START));
        assert!(!flags.contains(DspPacketFlags::STREAM_END));

        let combined_bits =
            DspPacketFlags::STREAM_START.bits() | DspPacketFlags::PACKET_START.bits();
        assert_eq!(flags.bits(), combined_bits);
    }

    // Test DSFH chunk parsing
    #[test]
    fn test_dsfh_chunk_parsing() {
        let data = vec![
            // Creation time as f64 (little-endian): Unix timestamp 1609459200.0 (2021-01-01)
            0x00, 0x00, 0x00, 0x80, 0x99, 0xfb, 0xd7, 0x41,
        ];

        let mut cursor = Cursor::new(data);
        let dsfh = DsfhChunk::read_from(&mut cursor, 24).unwrap();

        assert_eq!(&dsfh.header.id, b"DSFH");
        assert_eq!(dsfh.header.size, 24);
        assert!((dsfh.creation_time - 1609459200.0).abs() < 1.0); // Allow small floating point error
    }

    // Test STRM chunk parsing (proximity-based format)
    #[test]
    fn test_strm_chunk_proximity_format() {
        let data = vec![
            // Stream ID as u32 (little-endian): 12345
            0x39, 0x30, 0x00, 0x00, // Stream type: 1
            0x01, 0x00, 0x00, 0x00, // Reserved1: 0
            0x00, 0x00, 0x00, 0x00, // Sample rate as f32: 2048000.0 Hz
            0x00, 0x00, 0xfa, 0x49, // Reserved2: 8 bytes of zeros
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Center frequency as f32: 915000000.0 Hz
            0x2b, 0x27, 0x5a, 0x4e, // Device name: "TESTDEV" + null byte
            b'T', b'E', b'S', b'T', b'D', b'E', b'V', 0x00,
        ];

        let mut cursor = Cursor::new(data);
        let strm = StrmChunk::read_from(&mut cursor, 40).unwrap();

        assert_eq!(&strm.header.id, b"STRM");
        assert_eq!(strm.stream_id, 12345);
        assert_eq!(strm.stream_type, Some(1));
        assert!((strm.sample_rate.unwrap() - 2048000.0).abs() < 1.0);
        assert!((strm.center_frequency.unwrap() - 915000000.0).abs() < 1000.0);
        assert_eq!(strm.device_name.unwrap()[0..7], b"TESTDEV"[..]);
    }

    // Test STRM chunk parsing (standard format)
    #[test]
    fn test_strm_chunk_standard_format() {
        let data = vec![
            // Stream ID as u64 (little-endian): 98765
            0xCD, 0x81, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Start time as f64: 1609459200.0
            0x00, 0x00, 0x00, 0x80, 0x99, 0xfb, 0xd7, 0x41, // Stream offset as i64: 4096
            0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        let mut cursor = Cursor::new(data);
        let strm = StrmChunk::read_from(&mut cursor, 32).unwrap();

        assert_eq!(&strm.header.id, b"STRM");
        assert_eq!(strm.stream_id, 98765);
        assert!((strm.start_time - 1609459200.0).abs() < 1.0);
        assert_eq!(strm.stream_offset, 4096);
        assert_eq!(strm.stream_type, None);
        assert_eq!(strm.sample_rate, None);
        assert_eq!(strm.center_frequency, None);
        assert_eq!(strm.device_name, None);
    }

    // Test SAMP chunk parsing
    #[test]
    fn test_samp_chunk_parsing() {
        let mut data = vec![
            // Stream ID as u64: 12345
            0x39, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Sub stream ID as u32: 1
            0x01, 0x00, 0x00, 0x00, // Sample type: F32 (5)
            0x05, // Sample unit: Generic (0)
            0x00, // Payload type: IQ (2)
            0x02, // Compression: 0 (uncompressed)
            0x00, // Packet start time as f64: 1609459200.0
            0x00, 0x00, 0x00, 0x80, 0x99, 0xfb, 0xd7, 0x41,
            // Packet end time as f64: 1609459201.0
            0x00, 0x00, 0x40, 0x80, 0x99, 0xfb, 0xd7, 0x41,
            // Packet flags as u32: STREAM_START
            0x01, 0x00, 0x00, 0x00, // Sample size as u32: 8 bytes (4 for I + 4 for Q)
            0x08, 0x00, 0x00, 0x00, // Sample depth as u32: 32 bits
            0x20, 0x00, 0x00, 0x00, // Num samples as u32: 1024
            0x00, 0x04, 0x00, 0x00,
        ];

        // Add padding to match header size requirements
        data.resize(64, 0);

        let mut cursor = Cursor::new(data);
        let samp = SampChunk::read_from(&mut cursor, 64, 64).unwrap();

        assert_eq!(&samp.header.id, b"SAMP");
        assert_eq!(samp.stream_id, 12345);
        assert_eq!(samp.sub_stream_id, 1);
        assert_eq!(samp.sample_type, DspStreamSampleType::DsStF32);
        assert_eq!(samp.sample_unit, DspStreamSampleUnit::DssuGeneric);
        assert_eq!(samp.payload_type, DspStreamPayloadType::DsptIq);
        assert_eq!(samp.compression, 0);
        assert_eq!(samp.sample_size, 8);
        assert_eq!(samp.sample_depth, 32);
        assert_eq!(samp.num_samples, 1024);
        assert!(samp.packet_flags.contains(DspPacketFlags::STREAM_START));
    }

    // Test SSTR chunk parsing
    #[test]
    fn test_sstr_chunk_parsing() {
        let mut data = vec![
            // Stream ID as u64: 12345
            0x39, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Sub stream ID as u32: 1
            0x01, 0x00, 0x00, 0x00, // 4 bytes of alignment padding
            0x00, 0x00, 0x00, 0x00, // Sub stream offset as i64: 0 (end of chain)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Frequency start as f64: 915000000.0 Hz
            0x00, 0x00, 0x00, 0x60, 0xe5, 0x44, 0xcb, 0x41,
            // Frequency step as f64: 2048000.0 Hz (sample rate)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x3f, 0x41,
            // Frequency span as f64: 2048000.0 Hz (bandwidth)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x3f, 0x41,
            // Value minimum as f64: -100.0
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x59, 0xc0, // Value maximum as f64: 0.0
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Direction as f64: 0.0
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Antenna index as u32: 0
            0x00, 0x00, 0x00, 0x00, // Num categories as u32: 0
            0x00, 0x00, 0x00, 0x00,
        ];

        // Add name field (128 bytes)
        let name = b"Test Sub Stream";
        data.extend_from_slice(name);
        data.resize(data.len() + 128 - name.len(), 0); // Pad to 128 bytes

        // Add remaining fields
        data.extend_from_slice(&[
            // Antenna ID as u64: 1
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Metadata ID as u64: 2
            0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);

        // Pad to expected size
        data.resize(254, 0);

        let mut cursor = Cursor::new(data);
        let sstr = SstrChunk::read_from(&mut cursor, 254).unwrap();

        assert_eq!(&sstr.header.id, b"SSTR");
        assert_eq!(sstr.stream_id, 12345);
        assert_eq!(sstr.sub_stream_id, 1);
        assert_eq!(sstr.sub_stream_offset, 0);
        assert!((sstr.frequency_start - 915000000.0).abs() < 1000.0);
        assert!((sstr.frequency_step - 2048000.0).abs() < 1.0);
        assert!((sstr.frequency_span - 2048000.0).abs() < 1.0);
        assert!((sstr.value_minimum + 100.0).abs() < 1.0);
        assert_eq!(sstr.antenna_index, 0);
        assert_eq!(sstr.num_categories, 0);
        assert_eq!(sstr.antenna_id, 1);
        assert_eq!(sstr.metadata_id, 2);

        // Check name parsing
        let parsed_name = std::str::from_utf8(&sstr.name)
            .unwrap()
            .trim_end_matches('\0');
        assert_eq!(parsed_name, "Test Sub Stream");
    }

    // Test STRT chunk parsing
    #[test]
    fn test_strt_chunk_parsing() {
        let data = vec![
            // Stream offset as i64: 8192
            0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Sub stream offset as i64: 16384
            0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Preview offset as i64: 0
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Num samples as u64: 1048576
            0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Payload size as u64: 8388608 (1M samples * 8 bytes)
            0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, // Preview levels as u32: 4
            0x04, 0x00, 0x00, 0x00, // Num previews as u32: 16
            0x10, 0x00, 0x00, 0x00, // Num preview segments as u32: 256
            0x00, 0x01, 0x00, 0x00, // End time as f64: 1609459201.0
            0x00, 0x00, 0x40, 0x80, 0x99, 0xfb, 0xd7, 0x41,
            // Antenna offset as i64: 32768
            0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Metadata offset as i64: 65536
            0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        let mut cursor = Cursor::new(data);
        let strt = StrtChunk::read_from(&mut cursor, 80).unwrap();

        assert_eq!(&strt.header.id, b"STRT");
        assert_eq!(strt.stream_offset, 8192);
        assert_eq!(strt.sub_stream_offset, 16384);
        assert_eq!(strt.preview_offset, 0);
        assert_eq!(strt.num_samples, 1048576);
        assert_eq!(strt.payload_size, 8388608);
        assert_eq!(strt.preview_levels, 4);
        assert_eq!(strt.num_previews, 16);
        assert_eq!(strt.num_preview_segments, 256);
        assert!((strt.end_time - 1609459201.0).abs() < 1.0);
        assert_eq!(strt.antenna_offset, 32768);
        assert_eq!(strt.metadata_offset, 65536);
    }

    // Test DSFT chunk parsing
    #[test]
    fn test_dsft_chunk_parsing() {
        let data = vec![
            // Completion time as f64: 1609459201.0
            0x00, 0x00, 0x40, 0x80, 0x99, 0xfb, 0xd7, 0x41, // Stream offset as u64: 8192
            0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Num streams as u32: 1
            0x01, 0x00, 0x00, 0x00,
        ];

        let mut cursor = Cursor::new(data);
        let dsft = DsftChunk::read_from(&mut cursor, 20).unwrap();

        assert_eq!(&dsft.header.id, b"DSFT");
        assert!((dsft.completion_time - 1609459201.0).abs() < 1.0);
        assert_eq!(dsft.stream_offset, 8192);
        assert_eq!(dsft.num_streams, 1);
    }

    // Test ANTA chunk parsing
    #[test]
    fn test_anta_chunk_parsing() {
        let mut data = vec![
            // Antenna ID as u64: 123
            0x7B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Antenna offset as i64: 0 (end of chain)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        // Add name field (128 bytes)
        let name = b"Test Antenna";
        data.extend_from_slice(name);
        data.resize(data.len() + 128 - name.len(), 0); // Pad to 128 bytes

        data.extend_from_slice(&[
            // Latitude as f64: 37.7749 (San Francisco)
            0xd0, 0xd5, 0x56, 0xec, 0x2f, 0xe3, 0x42, 0x40,
            // Longitude as f64: -122.4194
            0x50, 0xfc, 0x18, 0x73, 0xd7, 0x9a, 0x5e, 0xc0,
            // Flags as u32: LOCATION_VALID
            0x01, 0x00, 0x00, 0x00, // Num segments as u32: 1
            0x01, 0x00, 0x00, 0x00,
        ]);

        // Add 4x4 transform matrix (64 bytes of identity matrix)
        for i in 0..4 {
            for j in 0..4 {
                if i == j {
                    data.extend_from_slice(&1.0f32.to_le_bytes());
                } else {
                    data.extend_from_slice(&0.0f32.to_le_bytes());
                }
            }
        }

        // Add antenna UUID (16 bytes)
        data.extend_from_slice(&[
            0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88,
        ]);

        // Pad to expected size
        data.resize(270, 0);

        let mut cursor = Cursor::new(data);
        let anta = AntaChunk::read_from(&mut cursor, 270).unwrap();

        assert_eq!(&anta.header.id, b"ANTA");
        assert_eq!(anta.antenna_id, 123);
        assert_eq!(anta.antenna_offset, 0);
        assert!((anta.latitude - 37.7749).abs() < 0.0001);
        assert!((anta.longitude + 122.4194).abs() < 0.0001);
        assert!(anta.flags.contains(DspafFlags::LOCATION_VALID));
        assert_eq!(anta.num_segments, 1);

        // Check identity matrix
        for i in 0..4 {
            for j in 0..4 {
                if i == j {
                    assert!((anta.transform[i][j] - 1.0).abs() < 0.0001);
                } else {
                    assert!((anta.transform[i][j] - 0.0).abs() < 0.0001);
                }
            }
        }

        // Check name parsing
        let parsed_name = std::str::from_utf8(&anta.name)
            .unwrap()
            .trim_end_matches('\0');
        assert_eq!(parsed_name, "Test Antenna");
    }

    // Test MDTT meta type definition parsing (simple type)
    #[test]
    fn test_mdtt_simple_meta_type() {
        let data = vec![
            // Metadata ID as u64: 456
            0xC8, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Metadata offset as i64: 0
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Meta type definition payload:
            // ID as u64: 789
            0x15, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // Meta type as u8: MtFloat (3)
            0x03, // Flags as u32: DSSMTF_32BIT (immediately follows, no padding)
            0x04, 0x00, 0x00, 0x00, // Count as u32: 1
            0x01, 0x00, 0x00, 0x00,
        ];

        let data_len = data.len() as u32;
        let total_size = 16 + data_len; // 16 bytes for chunk header + data
        let mut cursor = Cursor::new(data);
        let mdtt = MdttChunk::read_from(&mut cursor, total_size).unwrap();

        assert_eq!(&mdtt.header.id, b"MDTT");
        assert_eq!(mdtt.metadata_id, 456);
        assert_eq!(mdtt.metadata_offset, 0);

        let definition = mdtt.definition.unwrap();
        assert_eq!(definition.id, 789);
        assert_eq!(definition.meta_type, MetaType::MtFloat);
        assert!(definition.flags.contains(MetaTypeFlags::DSSMTF_32BIT));
        assert_eq!(definition.count, 1);
        assert!(definition.elements.is_empty());
    }

    // Test SPRV chunk parsing
    #[test]
    fn test_sprv_chunk_parsing() {
        let mut data = vec![
            // Preview level as u8: 2
            0x02, // Preview count as u8: 4
            0x04, // 6 bytes of padding
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        // Add 16 preview offsets (i64 each)
        for i in 0..16 {
            let offset = (i * 1024) as i64;
            data.extend_from_slice(&offset.to_le_bytes());
        }

        // Add 16 preview times (f64 each)
        for i in 0..16 {
            let time = 1609459200.0 + (i as f64);
            data.extend_from_slice(&time.to_le_bytes());
        }

        // Add 16 preview samples (u64 each)
        for i in 0..16 {
            let samples = (i * 1024) as u64;
            data.extend_from_slice(&samples.to_le_bytes());
        }

        // Pad to expected size
        data.resize(410, 0);

        let mut cursor = Cursor::new(data);
        let sprv = SprvChunk::read_from(&mut cursor, 410).unwrap();

        assert_eq!(&sprv.header.id, b"SPRV");
        assert_eq!(sprv.preview_level, 2);
        assert_eq!(sprv.preview_count, 4);

        // Check a few preview offsets and times
        assert_eq!(sprv.preview_offsets[0], 0);
        assert_eq!(sprv.preview_offsets[1], 1024);
        assert_eq!(sprv.preview_offsets[2], 2048);

        assert!((sprv.preview_times[0] - 1609459200.0).abs() < 1.0);
        assert!((sprv.preview_times[1] - 1609459201.0).abs() < 1.0);

        assert_eq!(sprv.preview_samples[0], 0);
        assert_eq!(sprv.preview_samples[1], 1024);
    }

    // Test SampleData variants
    #[test]
    fn test_sample_data_variants() {
        let iq_data = vec![Complex32::new(1.0, 2.0), Complex32::new(3.0, 4.0)];
        let sample_data = SampleData::Iq(iq_data.clone());

        match sample_data {
            SampleData::Iq(samples) => {
                assert_eq!(samples.len(), 2);
                assert_eq!(samples[0], Complex32::new(1.0, 2.0));
                assert_eq!(samples[1], Complex32::new(3.0, 4.0));
            }
            _ => panic!("Expected IQ sample data"),
        }

        let spectra_data = vec![1.0, 2.0, 3.0, 4.0];
        let sample_data = SampleData::Spectra(spectra_data.clone());

        match sample_data {
            SampleData::Spectra(samples) => {
                assert_eq!(samples.len(), 4);
                assert_eq!(samples, spectra_data);
            }
            _ => panic!("Expected Spectra sample data"),
        }
    }

    // Test RtsaMetadata structure
    #[test]
    fn test_rtsa_metadata_structure() {
        let metadata = RtsaMetadata {
            sample_rate: 2048000.0,
            center_frequency: Some(915000000.0),
            bandwidth: 2048000.0,
            total_samples: 1048576,
            start_time_ns: 1609459200000000000,
            end_time_ns: 1609459201000000000,
            creation_time: 1609459200.0,
            num_streams: 1,
            file_format_version: "RTSA".to_string(),
            primary_stream_id: 1,
            stream_type: Some("IQ_SAMPLES".to_string()),
            stream_sample_rate: Some(2048000.0),
            stream_center_frequency: Some(915000000.0),
            stream_start_time: 1609459200.0,
            device_name: None,
            sub_streams: Vec::new(),
            antennas: Vec::new(),
            previews: Vec::new(),
            stream_tail: None,
            total_sample_chunks: 0,
            sample_data_size: 8388608,
            metadata_definitions: Vec::new(),
        };

        assert_eq!(metadata.sample_rate, 2048000.0);
        assert_eq!(metadata.center_frequency, Some(915000000.0));
        assert_eq!(metadata.bandwidth, 2048000.0);
        assert_eq!(metadata.total_samples, 1048576);
        assert_eq!(metadata.start_time_ns, 1609459200000000000);
        assert_eq!(metadata.end_time_ns, 1609459201000000000);
    }

    // Test MetaTypeFlags bitflag operations
    #[test]
    fn test_meta_type_flags() {
        let flags = MetaTypeFlags::DSSMTF_32BIT | MetaTypeFlags::DSSMTF_SIGNED;
        assert!(flags.contains(MetaTypeFlags::DSSMTF_32BIT));
        assert!(flags.contains(MetaTypeFlags::DSSMTF_SIGNED));
        assert!(!flags.contains(MetaTypeFlags::DSSMTF_64BIT));

        let integer_flags = MetaTypeFlags::DSSMTF_32BIT | MetaTypeFlags::DSSMTF_SIGNED;
        assert_eq!(integer_flags.bits(), 0x00000014); // 0x04 | 0x10
    }

    // Test error handling for invalid chunk data
    #[test]
    fn test_invalid_chunk_handling() {
        // Test truncated header
        let short_data = vec![b'D', b'S', b'F', b'H', 0x10]; // Too short for complete header
        let mut cursor = Cursor::new(short_data);
        assert!(RtsaChunkHeader::read_from(&mut cursor).is_err());

        // Test invalid enum conversion
        assert!(DspStreamSampleType::try_from(200).is_err());
        assert!(MetaType::try_from(200).is_err()); // Should error for truly invalid values
    }

    // Test chunk size validation logic
    #[test]
    fn test_chunk_size_validation() {
        // Test various chunk header sizes and validate they parse correctly
        let valid_header = vec![
            b'T', b'E', b'S', b'T', // Test chunk ID
            0x20, 0x00, 0x00, 0x00, // Size: 32 bytes
            0x00, 0x00, 0x00, 0x00, // Flags: 0
            0x01, 0x00, // Version: 1
            0x10, 0x00, // Header size: 16
        ];

        let mut cursor = Cursor::new(valid_header);
        let header = RtsaChunkHeader::read_from(&mut cursor).unwrap();

        assert_eq!(&header.id, b"TEST");
        assert_eq!(header.size, 32);
        assert_eq!(header.header_size, 16);

        // Validate that data size calculation works correctly
        let data_size = header.size.saturating_sub(header.header_size as u32);
        assert_eq!(data_size, 16); // 32 - 16 = 16 bytes of payload data
    }

    // Test comprehensive metadata parsing and validation
    #[test]
    fn test_comprehensive_metadata_validation() {
        // Create test chunks
        let dsfh_chunk = DsfhChunk {
            header: RtsaChunkHeader {
                id: *b"DSFH",
                size: 24,
                flags: 0,
                version: 1,
                header_size: 16,
            },
            creation_time: 1640995200.0, // 2022-01-01 00:00:00 UTC
        };

        let strm_chunk = StrmChunk {
            header: RtsaChunkHeader {
                id: *b"STRM",
                size: 48,
                flags: 0,
                version: 1,
                header_size: 16,
            },
            stream_id: 1,
            start_time: 1640995200.0,
            stream_offset: 0,
            stream_type: Some(1), // IQ signal type
            sample_rate: Some(2_000_000.0),
            center_frequency: Some(100_000_000.0),
            device_name: Some(*b"TESTDEV\0"),
        };

        let sstr_chunk = SstrChunk {
            header: RtsaChunkHeader {
                id: *b"SSTR",
                size: 64,
                flags: 0,
                version: 1,
                header_size: 16,
            },
            stream_id: 1,
            sub_stream_id: 1,
            sub_stream_offset: 0,
            frequency_start: 99_000_000.0,
            frequency_step: 1_000_000.0,
            frequency_span: 2_000_000.0,
            value_minimum: -100.0,
            value_maximum: 0.0,
            direction: 0.0,
            antenna_index: 0,
            num_categories: 0,
            name: {
                let mut name = [0u8; 128];
                let sstr_name = b"Test Sub-Stream";
                name[..sstr_name.len()].copy_from_slice(sstr_name);
                name
            },
            antenna_id: 1,
            metadata_id: 0,
        };

        let anta_chunk = AntaChunk {
            header: RtsaChunkHeader {
                id: *b"ANTA",
                size: 192,
                flags: 0,
                version: 1,
                header_size: 16,
            },
            antenna_id: 1,
            antenna_offset: 0,
            name: {
                let mut name = [0u8; 128];
                let antenna_name = b"Test Antenna";
                name[..antenna_name.len()].copy_from_slice(antenna_name);
                name
            },
            latitude: 37.7749,
            longitude: -122.4194,
            flags: DspafFlags::empty(),
            num_segments: 1,
            transform: [[1.0, 0.0, 0.0, 0.0]; 4],
            antenna_uuid: [0u8; 16],
        };

        let strt_chunk = StrtChunk {
            header: RtsaChunkHeader {
                id: *b"STRT",
                size: 80,
                flags: 0,
                version: 1,
                header_size: 16,
            },
            stream_offset: 0,
            sub_stream_offset: 0,
            preview_offset: 0,
            num_samples: 1_000_000,
            payload_size: 8_000_000,
            preview_levels: 0,
            num_previews: 0,
            num_preview_segments: 0,
            end_time: 1640995201.0,
            antenna_offset: 0,
            metadata_offset: 0,
        };

        // Create hash maps
        let mut strm_chunks = HashMap::new();
        strm_chunks.insert(1u64, strm_chunk.clone());

        let mut sstr_chunks = HashMap::new();
        sstr_chunks.insert(1u32, sstr_chunk);

        let mut anta_chunks = HashMap::new();
        anta_chunks.insert(1u64, anta_chunk);

        let mdtt_chunks = HashMap::new();
        let sprv_chunks = Vec::new();

        // Create a test SAMP chunk
        let samp_chunk = SampChunk {
            header: RtsaChunkHeader {
                id: *b"SAMP",
                size: 32,
                flags: 0,
                version: 1,
                header_size: 16,
            },
            stream_id: 1,
            sub_stream_id: 1,
            sample_type: DspStreamSampleType::DsStF32,
            sample_unit: DspStreamSampleUnit::DssuVolt,
            payload_type: DspStreamPayloadType::DsptGeneric,
            compression: 0,
            packet_start_time: 1640995200.0,
            packet_end_time: 1640995200.25,
            packet_flags: DspPacketFlags::empty(),
            sample_size: 4,
            sample_depth: 32,
            num_samples: 250_000, // 1/4 of total samples for this chunk
        };
        let samp_chunk_offsets = vec![(0u64, samp_chunk)];

        // Test metadata building
        let metadata = RtsaSource::build_comprehensive_metadata(RtsaMetadataBuilder {
            dsfh_chunk: &dsfh_chunk,
            primary_strm_chunk: &strm_chunk,
            strm_chunks: &strm_chunks,
            sstr_chunks: &sstr_chunks,
            strt_chunk: &Some(strt_chunk),
            anta_chunks: &anta_chunks,
            mdtt_chunks: &mdtt_chunks,
            sprv_chunks: &sprv_chunks,
            samp_chunk_offsets: &samp_chunk_offsets,
            iq_stream_id: 1,
            stream_offset: None,
        })
        .unwrap();

        // Validate core metadata
        assert_eq!(metadata.sample_rate, 2_000_000.0);
        assert_eq!(metadata.center_frequency, Some(100_000_000.0));
        assert_eq!(metadata.total_samples, 1_000_000);
        assert_eq!(metadata.creation_time, 1640995200.0);
        assert_eq!(metadata.num_streams, 1);

        // Validate antenna information
        assert_eq!(metadata.antennas.len(), 1);
        assert_eq!(metadata.antennas[0].antenna_id, 1);
        assert_eq!(metadata.antennas[0].name, "Test Antenna");
        assert_eq!(metadata.antennas[0].latitude, 37.7749);
        assert_eq!(metadata.antennas[0].longitude, -122.4194);

        // Validate sub-stream information
        assert_eq!(metadata.sub_streams.len(), 1);
        assert_eq!(metadata.sub_streams[0].sub_stream_id, 1);
        assert_eq!(metadata.sub_streams[0].frequency_start, 99_000_000.0);
        assert_eq!(metadata.sub_streams[0].frequency_span, 2_000_000.0);

        // Validate stream tail
        assert!(metadata.stream_tail.is_some());
        let tail = metadata.stream_tail.as_ref().unwrap();
        assert_eq!(tail.num_samples, 1_000_000);
        assert_eq!(tail.payload_size, 8_000_000);
        assert_eq!(tail.end_time, 1640995201.0);
    }

    #[test]
    fn test_metadata_validation_report() {
        // Create test metadata with some issues
        let metadata = RtsaMetadata {
            sample_rate: 0.0, // Invalid: should be > 0
            center_frequency: None,
            bandwidth: -1.0, // Invalid: should be >= 0
            total_samples: 1000,
            start_time_ns: 1_640_995_200_000_000_000,
            end_time_ns: 1_640_995_199_000_000_000, // Invalid: before start time
            creation_time: 1640995200.0,
            num_streams: 1,
            file_format_version: "RTSA".to_string(),
            primary_stream_id: 1,
            stream_type: Some("IQ_SAMPLES".to_string()),
            stream_sample_rate: Some(2_000_000.0),
            stream_center_frequency: Some(100_000_000.0),
            stream_start_time: 1640995200.0,
            device_name: None,
            sub_streams: vec![SubStreamInfo {
                stream_id: 1,
                sub_stream_id: 1,
                sub_stream_offset: 0,
                frequency_start: 100_000_000.0,
                frequency_step: 0.0,  // Warning: should be > 0
                frequency_span: -1.0, // Error: should be > 0
                value_minimum: -100.0,
                value_maximum: 0.0,
                direction: 0.0,
                antenna_index: 0,
                num_categories: 0,
                name: "".to_string(), // Warning: empty name
                antenna_id: 1,
                metadata_id: 0,
            }],
            antennas: vec![AntennaInfo {
                antenna_id: 1,
                name: "".to_string(), // Warning: empty name
                latitude: 0.0,
                longitude: 0.0,
                flags: DspafFlags::empty(),
                num_segments: 0, // Warning: zero segments
                transform: [[1.0, 0.0, 0.0, 0.0]; 4],
                antenna_uuid: [0u8; 16],
            }],
            previews: Vec::new(),
            stream_tail: None,
            total_sample_chunks: 0,
            sample_data_size: 8000,
            metadata_definitions: Vec::new(),
        };

        let source = RtsaSource {
            _temp_path: None,
            reader: std::io::BufReader::new(tempfile::tempfile().unwrap()),
            metadata,
            current_sample_index: 0,
            iq_stream_id: 1,
            samp_chunk_offsets: Vec::new(),
            samp_chunk_start_samples: HashMap::new(),
            raw_iq_data_end_offset: None,
            chunk_scan_hint: 0,
        };

        let report = source.validate_structure().unwrap();

        // Should have errors
        assert!(!report.valid);
        assert!(report.errors.len() >= 2); // End time, sub-stream frequency span
        assert!(report.warnings.len() >= 4); // Empty antenna name, zero segments, frequency step, empty sub-stream name

        // Should have low completeness score due to missing data
        assert!(report.metadata_completeness < 80.0);
    }

    fn rtsa_source_with_sub_streams(sub_streams: Vec<SubStreamInfo>) -> RtsaSource {
        let metadata = RtsaMetadata {
            sample_rate: 1.0,
            center_frequency: None,
            bandwidth: 0.0,
            total_samples: 0,
            start_time_ns: 0,
            end_time_ns: 0,
            creation_time: 0.0,
            num_streams: 1,
            file_format_version: "RTSA".to_string(),
            primary_stream_id: 1,
            stream_type: None,
            stream_sample_rate: None,
            stream_center_frequency: None,
            stream_start_time: 0.0,
            device_name: None,
            sub_streams,
            antennas: Vec::new(),
            previews: Vec::new(),
            stream_tail: None,
            total_sample_chunks: 0,
            sample_data_size: 0,
            metadata_definitions: Vec::new(),
        };
        RtsaSource {
            _temp_path: None,
            reader: std::io::BufReader::new(tempfile::tempfile().unwrap()),
            metadata,
            current_sample_index: 0,
            iq_stream_id: 1,
            samp_chunk_offsets: Vec::new(),
            samp_chunk_start_samples: HashMap::new(),
            raw_iq_data_end_offset: None,
            chunk_scan_hint: 0,
        }
    }

    fn sub_stream(id: u32, value_minimum: f64, value_maximum: f64) -> SubStreamInfo {
        SubStreamInfo {
            stream_id: 1,
            sub_stream_id: id,
            sub_stream_offset: 0,
            frequency_start: 0.0,
            frequency_step: 0.0,
            frequency_span: 0.0,
            value_minimum,
            value_maximum,
            direction: 0.0,
            antenna_index: 0,
            num_categories: 0,
            name: String::new(),
            antenna_id: 0,
            metadata_id: 0,
        }
    }

    #[test]
    fn test_int16_scale_uses_value_range() {
        let source = rtsa_source_with_sub_streams(vec![sub_stream(7, -1.0, 1.0)]);
        let scale = source.int16_scale_for_sub_stream(7);
        assert!((scale - (1.0 / 32768.0)).abs() < 1e-9);
    }

    #[test]
    fn test_int16_scale_uses_max_abs_when_asymmetric() {
        let source = rtsa_source_with_sub_streams(vec![sub_stream(3, -100.0, 50.0)]);
        let scale = source.int16_scale_for_sub_stream(3);
        // max(|min|, |max|) / 32768 = 100 / 32768
        assert!((scale - (100.0 / 32768.0)).abs() < 1e-9);
    }

    #[test]
    fn test_int16_scale_falls_back_when_range_zero() {
        let source = rtsa_source_with_sub_streams(vec![sub_stream(1, 0.0, 0.0)]);
        let scale = source.int16_scale_for_sub_stream(1);
        assert!((scale - (1.0 / 32768.0)).abs() < 1e-9);
    }

    #[test]
    fn test_int16_scale_falls_back_when_sub_stream_unknown() {
        let source = rtsa_source_with_sub_streams(Vec::new());
        let scale = source.int16_scale_for_sub_stream(42);
        assert!((scale - (1.0 / 32768.0)).abs() < 1e-9);
    }

    #[test]
    fn rtsa_epoch_seconds_passes_through_seconds() {
        // 2022-01-01 UTC as Unix seconds — well below the µs cutoff.
        let v = 1_640_995_200.0;
        assert!((rtsa_epoch_seconds(v) - v).abs() < 1e-9);
    }

    #[test]
    fn rtsa_epoch_seconds_normalises_microseconds() {
        // 2022-01-01 UTC as Unix microseconds — divide by 1e6.
        let usec = 1_640_995_200_000_000.0;
        let secs = rtsa_epoch_seconds(usec);
        assert!((secs - 1_640_995_200.0).abs() < 1e-3, "got {secs}");
    }

    #[test]
    fn rtsa_epoch_seconds_handles_zero() {
        // Zero stays zero so callers can treat it as "no timestamp".
        assert_eq!(rtsa_epoch_seconds(0.0), 0.0);
    }

    #[test]
    fn rtsa_epoch_seconds_passes_through_negative_and_nan() {
        // Non-finite or non-positive inputs are returned unchanged so
        // downstream code can decide how to handle them.
        assert_eq!(rtsa_epoch_seconds(-1.0), -1.0);
        assert!(rtsa_epoch_seconds(f64::NAN).is_nan());
    }

    #[test]
    fn rtsa_epoch_seconds_cutoff_is_year_2286_safe() {
        // The cutoff sits at 1e13. Confirm year-2286-as-seconds lands on
        // the seconds branch (no division), and year-2025-as-microseconds
        // lands on the microseconds branch.
        let yr2286_as_seconds = 9.999e12; // just under 1e13
        assert!((rtsa_epoch_seconds(yr2286_as_seconds) - yr2286_as_seconds).abs() < 1e-3);
        let yr2025_as_micros = 1.7e15;
        assert!((rtsa_epoch_seconds(yr2025_as_micros) - 1.7e9).abs() < 1e-3);
    }

    #[test]
    fn test_accessor_methods() {
        let antenna_info = AntennaInfo {
            antenna_id: 1,
            name: "Test Antenna".to_string(),
            latitude: 37.7749,
            longitude: -122.4194,
            flags: DspafFlags::empty(),
            num_segments: 1,
            transform: [[1.0, 0.0, 0.0, 0.0]; 4],
            antenna_uuid: [0u8; 16],
        };

        let metadata = RtsaMetadata {
            sample_rate: 2_000_000.0,
            center_frequency: Some(100_000_000.0),
            bandwidth: 2_000_000.0,
            total_samples: 1_000_000,
            start_time_ns: 1_640_995_200_000_000_000,
            end_time_ns: 1_640_995_201_000_000_000,
            creation_time: 1640995200.0,
            num_streams: 1,
            file_format_version: "RTSA".to_string(),
            primary_stream_id: 1,
            stream_type: Some("IQ_SAMPLES".to_string()),
            stream_sample_rate: Some(2_000_000.0),
            stream_center_frequency: Some(100_000_000.0),
            stream_start_time: 1640995200.0,
            device_name: None,
            sub_streams: Vec::new(),
            antennas: vec![antenna_info],
            previews: Vec::new(),
            stream_tail: None,
            total_sample_chunks: 5,
            sample_data_size: 8_000_000,
            metadata_definitions: Vec::new(),
        };

        let source = RtsaSource {
            _temp_path: None,
            reader: std::io::BufReader::new(tempfile::tempfile().unwrap()),
            metadata,
            current_sample_index: 0,
            iq_stream_id: 1,
            samp_chunk_offsets: Vec::new(),
            samp_chunk_start_samples: HashMap::new(),
            raw_iq_data_end_offset: None,
            chunk_scan_hint: 0,
        };

        // Test accessor methods
        assert_eq!(source.antenna_info().len(), 1);
        assert_eq!(source.antenna_info()[0].name, "Test Antenna");
        assert!(source.has_antenna_positioning());
        assert!(!source.has_preview_data());
        assert!(!source.has_structured_metadata());

        let (chunks, size) = source.sample_chunk_stats();
        assert_eq!(chunks, 5);
        assert_eq!(size, 8_000_000);

        let (creation_time, num_streams, format) = source.file_info();
        assert_eq!(creation_time, 1640995200.0);
        assert_eq!(num_streams, 1);
        assert_eq!(format, "RTSA");

        let timing = source.timing_info();
        assert!(timing.contains("Duration: 1.000s"));
    }

    #[cfg(feature = "futuresdr")]
    #[tokio::test]
    #[ignore] // Only run when explicitly requested with --ignored - requires RTSA Suite Pro at atc.local:54664
    async fn test_live_rtsa_suite_pro_connection() {
        use crate::http_source::HttpSourceBuilder;

        // Test connection to RTSA Suite Pro at atc.local:54664
        let base_url = "http://atc.local:54664";

        println!("🔄 Testing connection to RTSA Suite Pro at {}", base_url);

        // Test that we can create the HTTP source block successfully
        let http_block = HttpSourceBuilder::new(base_url)
            .frequency(100_000_000.0) // 100 MHz
            .sample_rate(2_000_000.0) // 2 MSPS
            .timeout_ms(10000) // 10 second timeout
            .buffer_size(1024 * 1024) // 1MB buffer
            .format(crate::http_streaming::StreamFormat::Float32)
            .build();

        match http_block {
            Ok(_block) => {
                println!("✅ Successfully created HTTP source block for RTSA Suite Pro");
                println!("   Base URL: {}", base_url);
                println!("   Frequency: 100 MHz");
                println!("   Sample Rate: 2 MSPS");
                println!("   Format: Float32");
                println!("   Buffer Size: 1MB");
                println!("   Timeout: 10s");

                // In a real application, this block would be connected to a FutureSDR graph
                // and started. For testing purposes, successful creation indicates
                // proper configuration and network connectivity capability.

                println!("🔗 RTSA Suite Pro HTTP streaming block ready for FutureSDR integration");
            }
            Err(e) => {
                eprintln!("❌ Failed to create HTTP source for RTSA Suite Pro: {}", e);
                eprintln!(
                    "   This is expected if RTSA Suite Pro is not running at {}",
                    base_url
                );
                eprintln!("   Error details: {}", e);

                // Don't panic - just log the error for manual testing
                // This allows the test to run in CI/automated environments
                println!("ℹ️  Note: This test requires a live RTSA Suite Pro device");
                println!("   To test manually: Start RTSA Suite Pro and run with --ignored flag");
            }
        }

        println!(
            "✅ Live RTSA Suite Pro connection test completed (check output above for results)"
        );
    }

    #[cfg(feature = "futuresdr")]
    #[tokio::test]
    #[ignore] // Only run when explicitly requested with --ignored - requires RTSA Suite Pro with Remote Config License
    async fn test_rtsa_suite_pro_configuration_validation() {
        use crate::http_endpoints::AuthMethod;
        use crate::http_source::HttpSourceBuilder;
        use crate::http_streaming::StreamFormat;

        // Test comprehensive configuration validation for RTSA Suite Pro
        // NOTE: This test validates HTTP source builder configuration construction,
        // but actual device parameter changes via /remoteconfig endpoint require
        // a separate "Remote Config" license from Aaronia.
        // See: https://aaronia.com/en/software-licence-remote-config
        let base_url = "http://atc.local:54664";

        println!("🔧 Testing RTSA Suite Pro Configuration Options:");
        println!("⚠️  NOTE: /remoteconfig endpoint requires separate licensing");
        println!("   This test validates HTTP builder configuration only");

        // Test various frequency configurations
        let frequency_tests = vec![
            (50_000_000.0, "50 MHz - VHF Low"),
            (146_000_000.0, "146 MHz - Amateur 2m"),
            (462_000_000.0, "462 MHz - UHF/DMR"),
            (915_000_000.0, "915 MHz - ISM Band"),
            (1_090_000_000.0, "1090 MHz - ADS-B"),
            (2_400_000_000.0, "2400 MHz - WiFi"),
        ];

        for (frequency, description) in frequency_tests {
            let result = HttpSourceBuilder::new(base_url)
                .frequency(frequency)
                .sample_rate(2_000_000.0)
                .timeout_ms(5000)
                .build();

            match result {
                Ok(_) => println!("✓ {} - Configuration valid", description),
                Err(e) => println!("✗ {} - Configuration failed: {}", description, e),
            }
        }

        // Test various sample rate configurations
        let sample_rate_tests = vec![
            (1_000_000.0, "1 MSPS"),
            (2_000_000.0, "2 MSPS"),
            (5_000_000.0, "5 MSPS"),
            (10_000_000.0, "10 MSPS"),
            (20_000_000.0, "20 MSPS"),
        ];

        println!("\n📊 Sample Rate Configuration Tests:");
        for (sample_rate, description) in sample_rate_tests {
            let result = HttpSourceBuilder::new(base_url)
                .frequency(915_000_000.0) // Fixed at 915 MHz
                .sample_rate(sample_rate)
                .timeout_ms(5000)
                .build();

            match result {
                Ok(_) => println!("✓ {} - Configuration valid", description),
                Err(e) => println!("✗ {} - Configuration failed: {}", description, e),
            }
        }

        // Test different stream formats
        let format_tests = vec![
            (StreamFormat::Float32, "Float32"),
            (StreamFormat::Int16, "Int16"),
        ];

        println!("\n🎯 Stream Format Configuration Tests:");
        for (format, description) in format_tests {
            let result = HttpSourceBuilder::new(base_url)
                .frequency(915_000_000.0)
                .sample_rate(2_000_000.0)
                .format(format)
                .timeout_ms(5000)
                .build();

            match result {
                Ok(_) => println!("✓ {} format - Configuration valid", description),
                Err(e) => println!("✗ {} format - Configuration failed: {}", description, e),
            }
        }

        // Test authentication methods
        let auth_tests = vec![
            (AuthMethod::None, "No Authentication"),
            // Add other auth methods if available
        ];

        println!("\n🔐 Authentication Configuration Tests:");
        for (auth_method, description) in auth_tests {
            let result = HttpSourceBuilder::new(base_url)
                .frequency(915_000_000.0)
                .sample_rate(2_000_000.0)
                .auth(auth_method)
                .timeout_ms(5000)
                .build();

            match result {
                Ok(_) => println!("✓ {} - Configuration valid", description),
                Err(e) => println!("✗ {} - Configuration failed: {}", description, e),
            }
        }

        println!("\n✅ RTSA Suite Pro configuration validation completed!");
    }

    /// Wire-format round trip for [`SscaChunk::read_from`]. Constructs
    /// the on-disk byte sequence by hand (name + flags + RGBA +
    /// start/end frequencies), parses it, and asserts every field
    /// matches what we wrote. Doubles as living documentation of the
    /// SSCA layout for the day a caller actually consumes these
    /// chunks.
    #[test]
    fn test_ssca_chunk_read_from_round_trip() {
        use byteorder::WriteBytesExt;
        use std::io::Cursor;

        let mut bytes = Vec::new();
        // 128-byte name, zero-padded.
        let mut name = [0u8; 128];
        name[..7].copy_from_slice(b"channel");
        bytes.extend_from_slice(&name);
        // u32 flags (DsscfFlags bitfield).
        bytes.write_u32::<LittleEndian>(0).unwrap();
        // RGBA tint.
        bytes.write_u8(0x10).unwrap();
        bytes.write_u8(0x20).unwrap();
        bytes.write_u8(0x30).unwrap();
        bytes.write_u8(0xFF).unwrap();
        // start / end frequency in Hz.
        bytes.write_f64::<LittleEndian>(2_400_000_000.0).unwrap();
        bytes.write_f64::<LittleEndian>(2_500_000_000.0).unwrap();

        let total_len = bytes.len() as u32;
        let mut cursor = Cursor::new(bytes);
        let parsed = SscaChunk::read_from(&mut cursor, total_len).expect("SscaChunk parse");
        assert_eq!(&parsed.name[..7], b"channel");
        assert_eq!(parsed.red, 0x10);
        assert_eq!(parsed.green, 0x20);
        assert_eq!(parsed.blue, 0x30);
        assert_eq!(parsed.alpha, 0xFF);
        assert!((parsed.start_frequency - 2_400_000_000.0).abs() < 1.0);
        assert!((parsed.end_frequency - 2_500_000_000.0).abs() < 1.0);
        assert_eq!(parsed.header.id, *b"SSCA");
    }

    /// Wire-format round trip for [`AntsChunk::read_from`]. Same
    /// pattern as the SSCA test above — encode an in-memory ANTS
    /// payload (name + orientation quaternion + id), parse it,
    /// assert field equality.
    #[test]
    fn test_ants_chunk_read_from_round_trip() {
        use byteorder::WriteBytesExt;
        use std::io::Cursor;

        let mut bytes = Vec::new();
        let mut name = [0u8; 128];
        name[..7].copy_from_slice(b"segment");
        bytes.extend_from_slice(&name);
        // Orientation quaternion (x, y, z, w).
        for v in [0.1f32, 0.2, 0.3, 1.0] {
            bytes.write_f32::<LittleEndian>(v).unwrap();
        }
        // Segment id.
        bytes.write_u32::<LittleEndian>(42).unwrap();

        let total_len = bytes.len() as u32;
        let mut cursor = Cursor::new(bytes);
        let parsed = AntsChunk::read_from(&mut cursor, total_len).expect("AntsChunk parse");
        assert_eq!(&parsed.name[..7], b"segment");
        for (got, want) in parsed.orientation.iter().zip(&[0.1f32, 0.2, 0.3, 1.0]) {
            assert!(
                (got - want).abs() < 1e-6,
                "orientation mismatch: got {got}, want {want}"
            );
        }
        assert_eq!(parsed.id, 42);
        assert_eq!(parsed.header.id, *b"ANTS");
    }

    /// Write `data` to a temp file that is deleted when the returned
    /// `TempPath` drops — including on test panic (the previous version
    /// used `.keep()` and leaked files whenever an assertion failed).
    fn create_temp_file(data: &[u8]) -> tempfile::TempPath {
        use std::io::Write;
        use tempfile::NamedTempFile;
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(data).unwrap();
        file.into_temp_path()
    }

    fn write_chunk<W: std::io::Write>(
        writer: &mut W,
        id: &[u8; 4],
        size: u32,
        header_size: u16,
        stream_id: u64,
        sub_stream_id: u32,
        num_samples: u32,
    ) {
        use byteorder::WriteBytesExt;
        // RtsaChunkHeader
        writer.write_all(id).unwrap();
        writer.write_u32::<LittleEndian>(size).unwrap();
        writer.write_u32::<LittleEndian>(0).unwrap(); // flags
        writer.write_u16::<LittleEndian>(1).unwrap(); // version
        writer.write_u16::<LittleEndian>(header_size).unwrap();

        if id == b"SAMP" {
            // SampChunk fields (exactly 48 bytes)
            writer.write_u64::<LittleEndian>(stream_id).unwrap();
            writer.write_u32::<LittleEndian>(sub_stream_id).unwrap();
            writer.write_u8(5).unwrap(); // sample_type: DsStF32
            writer.write_u8(0).unwrap(); // sample_unit: Generic
            writer.write_u8(2).unwrap(); // payload_type: DsptIq
            writer.write_i8(0).unwrap(); // compression: 0
            writer.write_f64::<LittleEndian>(0.0).unwrap(); // packet_start_time
            writer.write_f64::<LittleEndian>(0.0).unwrap(); // packet_end_time
            writer.write_u32::<LittleEndian>(0).unwrap(); // packet_flags
            writer.write_u32::<LittleEndian>(8).unwrap(); // sample_size: 8
            writer.write_u32::<LittleEndian>(32).unwrap(); // sample_depth: 32
            writer.write_u32::<LittleEndian>(num_samples).unwrap(); // num_samples

            // Extra header padding if header_size > 64
            if header_size > 64 {
                writer
                    .write_all(&vec![0u8; (header_size - 64) as usize])
                    .unwrap();
            }

            // Rest of size padding (remaining of size)
            let written_so_far = header_size.max(64) as u32;
            if size > written_so_far {
                writer
                    .write_all(&vec![0u8; (size - written_so_far) as usize])
                    .unwrap();
            }
        } else {
            // Extra header padding if header_size > 16
            if header_size > 16 {
                writer
                    .write_all(&vec![0u8; (header_size - 16) as usize])
                    .unwrap();
            }
            // Skip non-SAMP chunks
            let written_so_far = header_size as u32;
            if size > written_so_far {
                writer
                    .write_all(&vec![0u8; (size - written_so_far) as usize])
                    .unwrap();
            }
        }
    }

    #[test]
    fn test_scan_for_samp_chunks_header_size_drift() {
        let mut data = Vec::new();
        // Write a SAMP chunk with header_size = 72, total size = 120
        write_chunk(&mut data, b"SAMP", 120, 72, 123, 1, 10);
        // Write a second SAMP chunk with header_size = 64, total size = 80
        write_chunk(&mut data, b"SAMP", 80, 64, 123, 1, 20);

        let path = create_temp_file(&data);
        let file = std::fs::File::open(&path).unwrap();
        let len = file.metadata().unwrap().len();
        let mut reader = std::io::BufReader::new(file);

        let mut samp_chunk_offsets = Vec::new();
        RtsaSource::scan_for_samp_chunks(&mut reader, 0, len, &mut samp_chunk_offsets).unwrap();

        assert_eq!(samp_chunk_offsets.len(), 2);
        assert_eq!(samp_chunk_offsets[0].1.num_samples, 10);
        assert_eq!(samp_chunk_offsets[1].1.num_samples, 20);
    }

    #[test]
    fn test_read_samples_with_metadata_chunk_in_between() {
        use byteorder::WriteBytesExt;

        let mut data = Vec::new();
        // 1. Write DSFH chunk (size 24)
        data.extend_from_slice(b"DSFH");
        data.write_u32::<LittleEndian>(24).unwrap(); // size = 24
        data.write_u32::<LittleEndian>(0).unwrap(); // flags = 0
        data.write_u16::<LittleEndian>(1).unwrap(); // version = 1
        data.write_u16::<LittleEndian>(16).unwrap(); // header_size = 16
        data.write_f64::<LittleEndian>(1609459200.0).unwrap(); // creation_time

        // 2. Write STRM chunk (standard format, size 40)
        data.extend_from_slice(b"STRM");
        data.write_u32::<LittleEndian>(40).unwrap(); // size = 40
        data.write_u32::<LittleEndian>(0).unwrap(); // flags = 0
        data.write_u16::<LittleEndian>(1).unwrap(); // version = 1
        data.write_u16::<LittleEndian>(16).unwrap(); // header_size = 16
        data.write_u64::<LittleEndian>(123).unwrap(); // stream_id = 123
        data.write_f64::<LittleEndian>(1609459200.0).unwrap(); // start_time
        data.write_i64::<LittleEndian>(400).unwrap(); // stream_offset = 400 (where SAMP chunks start)

        // 3. Write SSTR chunk (size 240)
        data.extend_from_slice(b"SSTR");
        data.write_u32::<LittleEndian>(240).unwrap(); // size = 240
        data.write_u32::<LittleEndian>(0).unwrap(); // flags = 0
        data.write_u16::<LittleEndian>(1).unwrap(); // version = 1
        data.write_u16::<LittleEndian>(224).unwrap(); // header_size = 224
        // SSTR fields
        data.write_u64::<LittleEndian>(123).unwrap(); // stream_id = 123
        data.write_u32::<LittleEndian>(1).unwrap(); // sub_stream_id = 1
        data.write_u32::<LittleEndian>(0).unwrap(); // padding
        data.write_i64::<LittleEndian>(0).unwrap(); // sub_stream_offset = 0
        data.write_f64::<LittleEndian>(2400000000.0).unwrap(); // frequency_start
        data.write_f64::<LittleEndian>(2000000.0).unwrap(); // frequency_step
        data.write_f64::<LittleEndian>(2000000.0).unwrap(); // frequency_span
        data.write_f64::<LittleEndian>(-100.0).unwrap(); // value_minimum
        data.write_f64::<LittleEndian>(0.0).unwrap(); // value_maximum
        data.write_f64::<LittleEndian>(0.0).unwrap(); // direction
        data.write_u32::<LittleEndian>(0).unwrap(); // antenna_index
        data.write_u32::<LittleEndian>(0).unwrap(); // num_categories
        let mut name = [0u8; 128];
        name[0..15].copy_from_slice(b"Test Sub Stream");
        data.extend_from_slice(&name); // name
        data.write_u64::<LittleEndian>(1).unwrap(); // antenna_id = 1
        data.write_u64::<LittleEndian>(2).unwrap(); // metadata_id = 2

        // 4. Write STRT chunk (size 96)
        data.extend_from_slice(b"STRT");
        data.write_u32::<LittleEndian>(96).unwrap(); // size = 96
        data.write_u32::<LittleEndian>(0).unwrap(); // flags = 0
        data.write_u16::<LittleEndian>(1).unwrap(); // version = 1
        data.write_u16::<LittleEndian>(72).unwrap(); // header_size = 72
        // STRT fields
        data.write_i64::<LittleEndian>(24).unwrap(); // stream_offset = 24 (points to STRM)
        data.write_i64::<LittleEndian>(64).unwrap(); // sub_stream_offset = 64 (points to SSTR)
        data.write_i64::<LittleEndian>(0).unwrap(); // preview_offset = 0
        data.write_i64::<LittleEndian>(25).unwrap(); // num_samples = 25
        data.write_i64::<LittleEndian>(200).unwrap(); // payload_size = 200 (10 * 8 + 15 * 8)
        data.write_u32::<LittleEndian>(0).unwrap(); // preview_levels
        data.write_u32::<LittleEndian>(0).unwrap(); // num_previews
        data.write_u32::<LittleEndian>(0).unwrap(); // num_preview_segments
        data.write_f64::<LittleEndian>(1609459201.0).unwrap(); // end_time
        data.write_i64::<LittleEndian>(0).unwrap(); // antenna_offset
        data.write_i64::<LittleEndian>(0).unwrap(); // metadata_offset
        data.write_u32::<LittleEndian>(0).unwrap(); // padding to align to size 96

        // 5. Write first SAMP chunk: stream=123, sub=1, 10 samples
        // size = 144 (64 header + 80 data), header_size = 64
        write_chunk(&mut data, b"SAMP", 144, 64, 123, 1, 10);

        // 6. Write a metadata/other chunk (e.g. DUMY), size = 32, header_size = 16
        write_chunk(&mut data, b"DUMY", 32, 16, 0, 0, 0);

        // 7. Write second SAMP chunk: stream=123, sub=1, 15 samples
        // size = 184 (64 header + 120 data), header_size = 64
        write_chunk(&mut data, b"SAMP", 184, 64, 123, 1, 15);

        // 8. Write trailing DSFT chunk
        data.extend_from_slice(b"DSFT");
        data.write_u32::<LittleEndian>(36).unwrap(); // size = 36
        data.write_u32::<LittleEndian>(0).unwrap(); // flags = 0
        data.write_u16::<LittleEndian>(1).unwrap(); // version = 1
        data.write_u16::<LittleEndian>(16).unwrap(); // header_size = 16
        data.write_f64::<LittleEndian>(1609459201.0).unwrap(); // completion_time
        data.write_u64::<LittleEndian>(760).unwrap(); // stream_offset = 760
        data.write_u32::<LittleEndian>(1).unwrap(); // num_streams = 1

        let path = create_temp_file(&data);
        let mut source = RtsaSource::open(&path).unwrap();
        assert_eq!(source.total_samples(), 25);

        // Read 5 samples (from first chunk)
        let res1 = source.read_samples(5, None).unwrap().unwrap();
        if let SampleData::Iq(samples) = res1 {
            assert_eq!(samples.len(), 5);
        } else {
            panic!("Expected IQ samples");
        }
        assert_eq!(source.current_position(), 5);

        // Read 10 samples (should cross the DUMY chunk into the second SAMP chunk)
        let res2 = source.read_samples(10, None).unwrap().unwrap();
        if let SampleData::Iq(samples) = res2 {
            assert_eq!(samples.len(), 5);
        } else {
            panic!("Expected IQ samples");
        }
        assert_eq!(source.current_position(), 10);

        // Next read gets the second chunk
        let res3 = source.read_samples(10, None).unwrap().unwrap();
        if let SampleData::Iq(samples) = res3 {
            assert_eq!(samples.len(), 10);
        } else {
            panic!("Expected IQ samples");
        }
        assert_eq!(source.current_position(), 20);
    }

    #[test]
    fn test_malformed_and_edge_cases() {
        use byteorder::WriteBytesExt;

        // 1. size = 0 header (N1 no hang)
        let mut data1 = Vec::new();
        data1.extend_from_slice(b"SAMP");
        data1.write_u32::<LittleEndian>(0).unwrap(); // size = 0
        data1.write_u32::<LittleEndian>(0).unwrap();
        data1.write_u16::<LittleEndian>(1).unwrap();
        data1.write_u16::<LittleEndian>(16).unwrap();
        // Append another valid chunk to see if it recovered
        write_chunk(&mut data1, b"SAMP", 80, 64, 123, 1, 10);

        let path = create_temp_file(&data1);
        let file = std::fs::File::open(&path).unwrap();
        let len = file.metadata().unwrap().len();
        let mut reader = std::io::BufReader::new(file);

        let mut samp_chunk_offsets = Vec::new();
        RtsaSource::scan_for_samp_chunks(&mut reader, 0, len, &mut samp_chunk_offsets).unwrap();
        // Since first chunk was invalid (size=0 < 16), it skipped byte by byte and eventually found the second chunk
        assert_eq!(samp_chunk_offsets.len(), 1);
        assert_eq!(samp_chunk_offsets[0].1.num_samples, 10);
    }
}
