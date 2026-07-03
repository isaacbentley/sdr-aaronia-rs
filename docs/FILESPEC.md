# Aaronia RTSA File Format Specification

## Overview

The Aaronia Real-Time Spectrum Analyzer (RTSA) file format is a chunk-based binary format designed for storing high-performance RF sample data, spectrum analysis results, and associated metadata. This specification covers both standard RTSA files and reverse-order variants, as well as HTTP streaming protocol variants.

This document outlines the complete structure of the Aaronia RTSA file format, based on the official PDF documentation, empirical analysis, and implementation experience.

> **Status & attribution.** This document is a *community-compiled* reference, **not** an official Aaronia specification. It is assembled from public posts on the Aaronia V6 forum, Aaronia's product documentation, and empirical analysis of capture files. Where these disagree, the vendor's own materials are authoritative. See [Sources and Attribution](#sources-and-attribution) for the upstream, vendor-published references.

## Table of Contents

- [File Structure](#file-structure)
- [Chunk Hierarchy](#chunk-hierarchy)
- [File Format Variants](#file-format-variants)
- [Chunk Definitions](#chunk-definitions)
- [Data Types and Enums](#data-types-and-enums)
- [Implementation Guidelines](#implementation-guidelines)
- [Compression and Decompression](#compression-and-decompression)
- [Sources and Attribution](#sources-and-attribution)

## File Structure

### Standard RTSA Format

```
┌─────────────────────┐
│ DSFH (File Header)  │  File metadata and creation time
├─────────────────────┤
│ STRM (Stream Head)  │  Stream configuration and timing
├─────────────────────┤
│ SSTR (Sub Stream)   │  Frequency and parameter definitions
├─────────────────────┤
│ ANTA (Antenna)      │  Antenna configuration and location
├─────────────────────┤
│ SAMP (Sample Data)  │  IQ/Spectrum sample packets
│       ...           │  (Multiple SAMP chunks)
├─────────────────────┤
│ STRT (Stream Tail)  │  Stream summary and pointers
├─────────────────────┤
│ DSFT (File Trailer) │  File completion metadata
└─────────────────────┘
```

### Reverse-Order RTSA Format

```
┌─────────────────────┐
│ Raw IQ Sample Data  │  Float32 IQ pairs (I,Q,I,Q,...)
│       ...           │  (Continuous binary data)
├─────────────────────┤
│ DSFH (File Header)  │  File metadata
├─────────────────────┤
│ STRM (Stream Head)  │  Stream configuration
├─────────────────────┤
│ STRT (Stream Tail)  │  Stream summary
├─────────────────────┤
│ DSFT (File Trailer) │  Points to metadata start
└─────────────────────┘
```

## Chunk Hierarchy

The basic layout is a series of self-contained **Segments**, where each segment contains one or more **Streams**.

```
RTSA File
└── Segment
    ├── DSFH (File Head)
    │   (Marks the beginning of a data segment)
    │
    ├── Stream
    │   ├── STRM (Stream Head)
    │   │   (Marks the beginning of a logical stream of data)
    │   │
    │   ├── ANTA (Antenna)
    │   │   │   (Defines an antenna used in the stream. Multiple ANTAs can exist,
    │   │   │    forming a backward-linked list via offsets.)
    │   │   │
    │   │   └── Payload Contains:
    │   │       └── ANTS (Antenna Segment) [...]
    │   │           (A series of chunks defining parts of a multi-segment antenna)
    │   │
    │   ├── SSTR (Sub Stream)
    │   │   │   (Defines a specific type of data, e.g., a frequency band. Multiple
    │   │   │    SSTRs can exist, forming a backward-linked list.)
    │   │   │
    │   │   └── Payload Contains:
    │   │       └── SSCA (Sub Stream Category) [...]
    │   │           (Named scalar values, if the sub-stream is category-based)
    │   │
    │   ├── SAMP (Sample)
    │   │   │   (Contains the actual measurement data. Belongs to a specific
    │   │   │    Stream and Sub Stream, identified by their IDs.)
    │   │   │
    │   │   └── Payload Contains:
    │   │       └── Raw sample data (e.g., IQ values, spectra)
    │   │
    │   ├── SPRV (Preview)
    │   │   │   (Contains downsampled data for visualization. These chunks form
    │   │   │    a tree structure to enable fast seeking.)
    │   │   │
    │   │   └── Payload Contains:
    │   │       └── Histogram and Waterfall data
    │   │
    │   ├── MDTT (Meta Data Type)
    │   │   │   (Optional chunk that defines a custom data structure used elsewhere.)
    │   │   │
    │   │   └── Payload Contains:
    │   │       └── Binary definition of the data type
    │   │
    │   └── STRT (Stream Tail)
    │       (Marks the end of the logical stream. Contains backward-pointing
    │        offsets to the STRM, last SSTR, last ANTA, and root SPRV.)
    │
    └── DSFT (File Tail)
        (Marks the end of the data segment. Contains the absolute offset
         to the start of this segment's data area.)

```

### Key Relationships:

*   **Segment:** A `DSFH` / `DSFT` pair encloses all the data for a recording session. If a file is appended to, it will contain multiple segments.
*   **Stream:** A `STRM` / `STRT` pair encloses a logical stream of data. A segment can contain multiple streams.
*   **Linked Lists:** Metadata chunks like `ANTA` and `SSTR` are not necessarily stored sequentially. They form backward-linked lists, where each chunk contains an offset pointing to the previous one. You typically find the last one via the `STRT` and follow the chain backward.
*   **Data Association:** A `SAMP` (Sample) chunk is logically associated with a parent `STRM` (Stream) and `SSTR` (Sub Stream) via the `mStreamID` and `mSubStreamID` fields within it.

---

## Chunk Definitions

### Base Chunk Header (`DSPStreamFileChunk`)

All chunks start with this common header.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mChunkID` | `quint32` | Four ASCII characters identifying the chunk type (e.g., 'DSFH'). |
| `mChunkSize` | `quint32` | The total size of the chunk in bytes, including this header. |
| `mChunkFlags` | `quint32` | Miscellaneous flags for the chunk. |
| `mVersion` | `quint16` | Version number for the chunk's data structure. |
| `mHeaderSize` | `quint16` | The size of this specific chunk's header (can be larger than the base). |

---

### 1. File Head (`DSFH`)
Starts a new, independent segment in an RTSA file.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mCreationTime` | `double` | The creation time of the file, relative to the Unix epoch. |

---

### 2. File Tail (`DSFT`)
Terminates a file segment. Found at the end of the file.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mCompletionTime` | `double` | The completion time of the file, relative to the Unix epoch. |
| `mStreamOffset` | `qint64` | Absolute file position of the stream's data area. (Note: The PDF states this is the "Offset of the tail of the last stream in the file", which our analysis indicates is a discrepancy.) |
| `mNumStreams` | `quint32` | Total number of streams in the file. |

---

### 3. Stream Head (`STRM`)
Indicates the start of a new stream.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mStreamID` | `quint64` | Unique 64-bit ID for this stream. |
| `mStartTime` | `double` | Start time of the stream, relative to the Unix epoch. |
| `mStreamOffset` | `qint64` | Offset of the tail chunk (`STRT`) of the *previous* stream in the file. |

---

### 4. Stream Tail (`STRT`)
Marks the end of a stream and contains offsets to its metadata.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mStreamOffset` | `qint64` | Offset of this stream's head chunk (`STRM`). |
| `mSubStreamOffset` | `qint64` | Offset of the last sub-stream chunk (`SSTR`). |
| `mPreviewOffset` | `qint64` | Offset of the last (root) preview chunk (`SPRV`). |
| `mNumSamples` | `quint64` | Total number of samples in this stream. |
| `mPayloadSize` | `quint64` | Total payload size in bytes for this stream. |
| `mPreviewLevels` | `quint32` | Number of levels in the preview hierarchy tree. |
| `mNumPreviews` | `quint32` | Total number of preview elements. |
| `mNumPreviewSegments` | `quint32` | Total number of preview segments. |
| `mEndTime` | `double` | End time of the stream (stream duration). |
| `mAntennaOffset` | `qint64` | Offset of the last antenna chunk (`ANTA`). |
| `mMetaDataOffset` | `qint64` | Offset of the last metadata type chunk (`MDTT`). |

---

### 5. Sub Stream (`SSTR`)
Contains common metadata for a series of samples (e.g., frequency range, sample rate).

> **Wire-format alignment** — the recorded SSTR payload follows standard
> C-struct alignment: the `qint64 mSubStreamOffset` field is 8-byte
> aligned, so the compiler inserts **4 bytes of zero padding** after the
> `quint32 mSubStreamID`. A reader that doesn't skip those 4 bytes will
> consume them as the low half of `mSubStreamOffset` (and every
> downstream offset / frequency derived from this chunk will be wrong).
> The Rust reader (`SstrChunk::read_from` in `file_source.rs`) issues a
> `reader.seek(SeekFrom::Current(4))` between the two fields and uses
> `HEADER_SIZE = 224` (not 220) for this reason.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mStreamID` | `quint64` | ID of the parent stream. |
| `mSubStreamID` | `quint32` | Unique ID for this sub-stream. |
| *(padding)* | 4 bytes | Alignment padding before the 8-byte-aligned `mSubStreamOffset`. |
| `mSubStreamOffset` | `qint64` | Offset of the previous sub-stream chunk for this stream. |
| `mFrequencyStart` | `double` | Start of the frequency range. |
| `mFrequencyStep` | `double` | Sample rate or bin step. |
| `mFrequencySpan` | `double` | Size of the frequency range. |
| `mValueMinimum` | `double` | Lowest value. |
| `mValueMaximum` | `double` | Highest value. |
| `mDirection` | `double` | Simple directional indicator. |
| `mAntennaIndex` | `quint32` | Index of a multi-segment antenna. |
| `mNumCategories` | `quint32` | Number of categories if samples are name-indexed. |
| `mName` | `char[128]` | Name of this sub-stream. |
| `mAntennaID` | `quint64` | ID of the antenna used for this sub-stream. |
| `mMetaDataID` | `quint64` | ID of the metadata type for a structured data sub-stream. |
| **Payload** | `SSCA[]` | The payload contains a series of Sub Stream Category (`SSCA`) chunks. |

---

### 6. Sub Stream Category (`SSCA`)
A named scalar measurement within a category sub-stream (e.g., channel power).

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mName` | `char[128]` | Name of the category. |
| `mFlags` | `quint32` | Category flags (`DSSCF_*`). |
| `mRed`, `mGreen`, `mBlue`, `mAlpha` | `quint8` | Color values for visualization. |
| `mStartFrequency` | `double` | Start frequency. |
| `mEndFrequency` | `double` | End frequency. |

---

### 7. Antenna (`ANTA`)
Combines physical, logical, and geographical properties of an antenna.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mAntennaID` | `quint64` | Unique ID of the antenna. |
| `mAntennaOffset` | `qint64` | Offset of the previous antenna chunk in the stream. |
| `mName` | `char[128]` | Name of the antenna. |
| `mLatitude`, `mLongitude` | `double` | Location of the base antenna. |
| `mFlags` | `quint32` | Antenna flags (`DSPAF_*`). |
| `mNumSegments` | `quint32` | Number of antenna segments. |
| `mTransform` | `float[4][4]` | Antenna transformation matrix (e.g., rotation). |
| `mAntennaUUID` | `char[16]` | Global unique ID of the physical antenna. |
| **Payload** | `ANTS[]` | The payload contains a series of Antenna Segment (`ANTS`) chunks. |

---

### 8. Antenna Segment (`ANTS`)
A single segment of a multi-segment antenna.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mName` | `char[128]` | Name of the segment. |
| `mOrientation` | `float[4]` | Orientation of the segment in the antenna coordinate system. |
| `mID` | `quint32` | ID of the segment. |

---

### 9. Meta Data Type (`MDTT`)
Defines the structure for structured data.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mMetaDataID` | `quint64` | Unique ID of this metadata type. |
| `mMetaDataOffset` | `qint64` | Offset of the previous metadata chunk. |
| **Payload** | `binary` | The payload contains the binary compressed definition of the type. |

---

### 10. Preview (`SPRV`)
Contains a histogram and preview spectra for fast seeking and visualization.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mPreviewLevel` | `quint8` | Level of this chunk in the preview tree (0 for leaf nodes). |
| `mPreviewCount` | `quint8` | Number of preview elements in this chunk. |
| `mPreviewOffsets` | `qint64[16]` | Offsets of child preview chunks or sample chunks. |
| `mPreviewTimes` | `double[16]` | Start times of the child preview chunks. |
| `mPreviewSamples` | `quint64[16]` | Start sample index numbers of the child preview chunks. |
| **Payload** | `struct` | Contains `mHistogram` and `mWaterfall` arrays for visualization. |

---

### 11. Samples (`SAMP`)
Contains the actual measurement data.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mStreamID` | `quint64` | ID of the parent stream. |
| `mSubStreamID` | `quint32` | ID of the sub-stream for this data. |
| `mSampleType` | `enum:8` | Data type of individual data elements (`DSST_*`). |
| `mSampleUnit` | `enum:8` | Unit used for the samples (`DSSU_*`). |
| `mPayloadType` | `enum:8` | High-level sample data structure (`DSPT_*`). |
| `mCompression` | `qint32:8` | Compression type (0 for lossless, 1-31 for lossy factor). |
| `mPacketStartTime` | `double` | Start time of this chunk relative to stream start. |
| `mPacketEndTime` | `double` | End time of this chunk relative to stream start. |
| `mPacketFlags` | `quint32` | Packet flags (`DSPPF_*`). |
| `mSampleSize` | `quint32` | Size of an individual sample. |
| `mSampleDepth` | `quint32` | Depth of a sample. |
| `mNumSamples` | `quint32` | Number of samples in this packet. |
| **Payload** | `binary` | The payload contains the actual sample data, formatted as specified by the header fields. |

#### SAMP Chunk Packet Flags (`mPacketFlags`)

These flags are used in the `mPacketFlags` field of the `SAMP` chunk to provide additional information about the packet.

| Flag Name | Value (Hex) | Description |
| :--- | :--- | :--- |
| `DSPPF_STREAM_START` | `0x00000001` | A new stream starts with this packet. |
| `DSPPF_STREAM_END` | `0x00000002` | The current stream ends after this packet. |
| `DSPPF_SEGMENT_START` | `0x00000004` | A new segment starts with this packet. |
| `DSPPF_SEGMENT_END` | `0x00000008` | The current segment ends with this packet. |
| `DSPPF_BREAK` | `0x00000010` | The content of the stream is broken before this packet. |
| `DSPPF_FLUSH` | `0x00000020` | Flush the processing pipe down stream. |
| `DSPPF_STREAM_FLAGS` | `(Composite)` | `DSPPF_STREAM_START | DSPPF_STREAM_END | DSPPF_SEGMENT_START | DSPPF_SEGMENT_END` |
| `DSPPF_PACKET_START` | `0x00000040` | This is the first sample of a packet. |
| `DSPPF_PACKET_END` | `0x00000080` | This is the last sample of a packet. |
| `DSPPF_WARN_OVERFLOW` | `0x00000100` | Data overflow, and most likely clipped. |
| `DSPPF_WARN_DROPPED` | `0x00000200` | Data missing due to packet drop. |
| `DSPPF_WARN_INACCURATE` | `0x00000400` | Data is inaccurate (e.g., due to missing calibration or unstable clock). |
| `DSPPF_WARN_RESAMPLED` | `0x00000800` | (No description provided in image for this specific flag, but it's a warning). |
| `DSPPF_REPLAY` | `0x00001000` | The media sample is the start of a replay. |
| `DSPPF_IMMEDIATE` | `0x00002000` | The media sample is supposed to be processed immediately and displayed as a single update. |
| `DSPPF_TIME_OVERLAP` | `0x00004000` | Start time of this sample may be before end time of previous sample. |
| `DSPPF_PUSH` | `0x00008000` | Push the packet through the chain to the display; do not delay or combine. |
| `DSPPF_TIME_DISCONTINUITY` | `0x00010000` | There is a time discontinuity between this and the previous packet. |
| `DSPPF_WARN_DIRECTION` | `0x00020000` | There is a time discontinuity between this and the previous packet. |
| `DSPPF_REJECTED` | `0x00100000` | Eliminated by filter 0. |
| `DSPPF_USER_0` | `0x01000000` | User-defined flag 0. |
| `DSPPF_USER_1` | `0x02000000` | User-defined flag 1. |
| `DSPPF_USER_2` | `0x04000000` | User-defined flag 2. |
| `DSPPF_USER_3` | `0x08000000` | User-defined flag 3. |
| `DSPPF_CONDITION_0` | `0x10000000` | Condition flag 0. |
| `DSPPF_CONDITION_1` | `0x20000000` | Condition flag 1. |
| `DSPPF_CONDITION_2` | `0x40000000` | Condition flag 2. |
| `DSPPF_CONDITION_3` | `0x80000000` | Condition flag 3. |

---

## Additional Elements

### General Data Type Conventions

Across all chunks, the file format adheres to these conventions:

*   **Endianness:** All data is stored in **little-endian** format.
*   **Time:** Timestamps are stored as 64-bit floating-point `double` values, representing seconds relative to the Unix epoch (January 1st, 1970, 12:00 AM) or the start of the stream.
*   **Offsets:** All file offsets are 64-bit unsigned integers (`quint64`) representing an absolute position from the start of the file (byte 0).
*   **Strings:** Strings are stored as standard UTF-8 and are padded with trailing zeros to fill their fixed-size character arrays.

#### Time-unit caveat (community-reported)

The official spec text above says "seconds relative to the Unix epoch", but
a community member reading v4 captures reported (Aaronia forum, 2025) that
in practice **`DSFH::mCreationTime` and `DSFT::mCompletionTime` decode as
microseconds since the Unix epoch, not seconds**. The `STRM::mStartTime`
field in the same captures stays in seconds. `SAMP::mPacketStartTime` and
`SAMP::mPacketEndTime` "appear to be in microseconds since the stream
start" but the original poster could not fully verify that.

The Rust binding accommodates this with a value-range heuristic:
[`file_source::rtsa_epoch_seconds`] divides any input ≥ 10¹³ by 10⁶ before
returning it as seconds-since-epoch. The cutoff sits well above any
realistic Unix-seconds timestamp this century (2025 ≈ 1.7×10⁹) and well
below any plausible Unix-microseconds timestamp from the same era
(2025 ≈ 1.7×10¹⁵). All `RtsaMetadata` time-related fields the binding
publishes pass through that helper, so callers always see seconds.

---

### Enum and Flag Definitions

Several chunks use enums or flags to specify data types or features. The most important ones are for the `SAMP` chunk.

#### `SAMP` Chunk: `mSampleType`
Specifies the data type of individual data elements.

| Enum Value (`DPSStreamSampleType`) | Description |
| :--- | :--- |
| `DSST_U8`, `DSST_U16`, `DSST_U32` | Unsigned integer of 8, 16, or 32 bits. |
| `DSST_S16`, `DSST_S32` | Signed integer of 16 or 32 bits. |
| `DSST_F32` | 32-bit float. |
| `DSST_U8N`, `U16N`, `S16N`, etc. | "Packet storage" format, where elements are not stored on 16-byte boundaries. |

#### `SAMP` Chunk: `mSampleUnit`
Specifies the physical unit for the sample data.

| Enum Value (`DSPStreamSampleUnit`) | Description |
| :--- | :--- |
| `DSSU_GENERIC` | Generic floating-point value. |
| `DSSU_DBM` | Decibel-milliwatts. |
| `DSSU_DBM_HZ` | Decibel-milliwatts per Hertz. |
| `DSSU_PERCENTAGE` | Percentage (0 to 1). |
| `DSSU_HZ` | Hertz. |
| `DSSU_WATT` | Watts. |
| `DSSU_VOLT` | Volts. |
| `DSSU_TIME` | Floating-point seconds. |
| `DSSU_DATE_TIME` | Floating-point seconds since the Unix epoch. |

#### `SAMP` Chunk: `mPayloadType`
Specifies the high-level structure of the sample data.

| Enum Value (`DSPStreamPayloadType`) | Description |
| :--- | :--- |
| `DSPT_GENERIC` | Generic numeric data. |
| `DSPT_AUDIO` | Audio samples. |
| `DSPT_IQ` | IQ samples (two values per sample). |
| `DSPT_SPECTRA` | Power spectra. |
| `DSPT_DETECTION` | Detection probability. |
| `DSPT_HISTOGRAM` | Histogram data. |
| `DSPT_STRUCTURED` | Structured data using meta-data types. |
| `DSPT_IMAGE` | Grayscale image. |

---

### Structured Data and Meta Data Types (`MDTT`)

The `MDTT` chunk allows for defining complex, hierarchical data structures. This is a "type of types" system.

*   **Base Types:** A small set of base types are defined (`MT_BOOL`, `MT_INTEGER`, `MT_FLOAT`, `MT_STRING`).
*   **Type Constructors:** These base types can be combined using three constructors:
    *   `MT_VECTOR`: A fixed-size array of elements.
    *   `MT_ARRAY`: A variable-size array of elements.
    *   `MT_OBJECT`: A structure with up to 32 named child elements (fields).
*   **Storage:**
    *   **Objects** are stored with a 32-bit mask indicating which fields are present, followed by the data for the non-zero elements.
    *   **Arrays** are stored with a 32-bit size, followed by the sequence of elements.
    *   **Vectors** are stored as a packed sequence of elements.

---

### Compression of Spectrum Data (`DSPT_SPECTRA`)

When `SAMP` chunks contain spectra and `mCompression` is non-zero, a three-step algorithm is used:

1.  **Wavelet Conversion:** A trivial wavelet transform is performed on blocks of spectra. It replaces even/odd indexed numbers with their sum/difference, which separates low-pass and high-pass coefficients. This is done recursively.
2.  **Quantization:** The resulting coefficients are uniformly quantized (converted to integers) using a factor derived from the `mCompression` field (`1` to `31`).
3.  **Bit Packing:** The quantized integers are stored using a variant of the Rice Code. The number of leading zero bits in the code indicates the size of the residual value that follows, allowing for efficient storage of numbers with a skewed probability distribution (many small values).

Decompression is performed in the inverse order: Unpacking -> Dequantization -> Inverse Wavelet Transform.

### Compression of IQ Data (`DSPT_IQ`)

**Note:** According to Aaronia representatives on their official forums, the specifics of the IQ compression algorithm are considered internal and proprietary, and have not been published in the official file format specification.

However, based on empirical testing (specifically with CW signals), compressed IQ datasets can be partially recovered using a fallback strategy:

1.  **Flat Bitstream Decoding:** The payload is treated as a continuous flat bitstream using the standard Rice decoding variant (the exact same decoder used for `DSPT_SPECTRA`). 
2.  **Zero Padding:** Because the proprietary encoder truncates the file once all remaining wavelet coefficients are mathematically zero, the decoder will run out of bits before filling the entire expected `num_rows × num_cols` matrix. The remaining coefficients in the matrix are simply padded with zeroes.
3.  **2D Wavelet Transform:** A 2D Inverse Haar Wavelet Transform is then applied to the zero-padded matrix. 

*Warning: This fallback strategy perfectly reconstructs sparse signals (like a clean CW tone), but due to undocumented proprietary padding or block framing present in more complex signals (like broadband LTE), broadband data decoded this way may contain significant artifacts.*

---

## Implementation Details and Examples

This section provides specific constants, tables, and examples from the PDF that are crucial for implementation.

### Implementation Constants

#### SSCA Chunk Flags (`mFlags`)
| Flag | Value | Description |
| :--- | :--- | :--- |
| `DSSCF_FREQUENCY_VALID` | `0x00000001` | Indicates that the `mStartFrequency` and `mEndFrequency` fields are valid. |
| `DSSCF_COLOR_VALID` | `0x00000002` | Indicates that the color value fields (`mRed`, `mGreen`, etc.) are valid. |

#### ANTA Chunk Flags (`mFlags`)
| Flag | Value | Description |
| :--- | :--- | :--- |
| `DSPAF_LOCATION_VALID` | `0x00000001` | The `mLatitude` and `mLongitude` fields are valid. |
| `DSPAF_TRANSFORM_VALID` | `0x00000002` | The `mTransform` matrix is valid. |
| `DSPAF_DIRECTION_VALID` | `0x00000004` | The direction is valid. |
| `DSPAF_ROTATION` | `0x00000008` | The rotation is valid. |

#### SPRV Chunk Constants
| Constant | Value | Description |
| :--- | :--- | :--- |
| `HistogramWidth` | 48 | Width of the preview histogram. |
| `HistogramHeight` | 32 | Height of the preview histogram. |
| `WaterfallWidth` | 128 | Width of the preview waterfall image. |
| `SegmentsShift` | 4 | Bit shift to get the number of segments (2^4 = 16). |
| `Segments` | 16 | Number of segments in a preview chunk. |
| `Samples` | 4096 | Number of samples referenced by a leaf preview chunk. |

### Compression Bit-Packing Codes

This table maps the variable-length codes to integer values in the bit-packing stage of compression.

| Code (Binary) | Value | Code (Binary) | Value |
| :--- | :--- | :--- | :--- |
| `1000` | +0 | `1001` | -0 |
| `1010` | +1 | `1011` | -1 |
| `1100` | +2 | `1101` | -2 |
| `1110` | +3 | `1111` | -3 |
| `0100 0000` | +4 | `0100 0001` | -4 |
| `0100 0010` | +5 | `0100 0011` | -5 |
| ... | ... | ... | ... |

*(Note: The pattern continues where the number of leading zeros determines the size of the encoded integer.)*

### Concrete MDTT Binary Examples

#### Example 1: Array of 16-bit Signed Integers
This example shows the binary layout for a metadata type that defines an array of `int16`.

```
// MetaType {id=1, type=MT_ARRAY, flags=0, count=0, elements=[...]}
01 00 00 00 00 00 00 00  // mID = 1
06                       // mType = MT_ARRAY (6)
00 00 00 00              // mFlags = 0
00 00 00 00              // mCount = 0
00 00 00 01              // mElements size = 1

// Element {name="", flags=0, type={...}}
00 00 00 00              // mName = "" (empty string)
00 00 00 00              // mFlags = 0

// Type {id=2, type=MT_INTEGER, flags=16bit|signed, count=0}
02 00 00 00 00 00 00 00  // mID = 2
02                       // mType = MT_INTEGER (2)
12 00 00 00              // mFlags = DSSMTF_16BIT | DSSMTF_SIGNED
00 00 00 00              // mCount = 0
```

### Decompression Sample Code (C++)

The following C++ code illustrates the inverse wavelet transform, which is the core of the decompression algorithm.

```cpp
void WaveTransformStep(quint32 sx, quint32 sy, quint32 dxy)
{
    for (quint32 y = 0; y < NumRows; y += sy)
    {
        for (quint32 x = 0; x < NumColumns; x += sx)
        {
            float s = WaveBuffer[x + y * NumColumns];
            float t = WaveBuffer[x + y * NumColumns + dxy];

            WaveBuffer[x + y * NumColumns] = SQRTHALF * (s + t);
            WaveBuffer[x + y * NumColumns + dxy] = SQRTHALF * (s - t);
        }
    }
}

void WaveDecompress(void)
{
    quint32 step = 1;
    while ((NumRows & (2 * step - 1)) == 0) step *= 2;
    while ((NumColumns & (2 * step - 1)) == 0) step *= 2;

    while (step > 1)
    {
        step >>= 1;
        if ((NumColumns & (2 * step - 1)) == 0)
        {
            WaveTransformStep(2 * step, step, step);
        }
        if ((NumRows & (2 * step - 1)) == 0)
        {
            WaveTransformStep(step, 2 * step, step * NumColumns);
        }
    }
}
```

### Annotated Hex Dump Example

This section breaks down the annotated hex dump of a sample file, illustrating how raw bytes map to the chunk structures.

#### File Header (DSFH)
```
44 53 46 48      mChunkID
18 00 00 00      mChunkSize
00 00 00 00      mChunkFlags
01 00            mVersion
18 00            mHeaderSize
E0 96 2A ED 3A 1C 15 43 mCreationTime
```

#### Stream Header (STRM)
```
53 54 52 4D      mChunkID
28 00 00 00      mChunkSize
...
07 00 00 00 00 00 00 00 mStreamID
29 5C FF EC BE 22 D6 41 mStartTime
00 00 00 00 00 00 00 00 mStreamOffset (terminating, no prior stream)
```

#### Sample Packet (SAMP)
```
53 41 4D 50      mChunkID
40 70 00 00      mChunkSize
...
07 00...         mStreamID
03 00 00 00      mSubStreamID
05               mSampleType (DSST_F32)
01               mSampleUnit (DSSU_DBU)
03               mPayloadType (DSPT_SPECRTA)
00               mCompression (uncompressed)
D9 39...         mPacketStartTime
1D E7...         mPacketEndTime
...
80 03 00 00      mSampleSize (896 bins)
01 00 00 00      mSampleDepth
08 00 00 00      mNumSamples (8 spectra)
```

#### Stream Tail (STRT)
```
53 54 52 54      mChunkID
58 00 00 00      mChunkSize
...
18 00...         mStreamOffset (offset to STRM chunk)
38 01...         mSubStreamOffset (offset to last SSTR chunk)
30 D8...         mPreviewOffset
10 13...         mNumSamples
...
01 00 00 00      mPreviewLevels (1 level)
06 00 00 00      mNumPreviews (6 chunks)
...
```

---

## File Format Variants

### Standard RTSA Files

- **Structure**: Header chunks + SAMP data chunks + trailer
- **Detection**: Presence of SAMP chunks with structured headers
- **Sample Access**: Sequential reading through SAMP chunks
- **Metadata**: Complete metadata in SSTR, ANTA, MDTT chunks

### Reverse-Order RTSA Files

- **Structure**: Raw IQ data + metadata at end
- **Detection**: No SAMP chunks found, stream_offset > 0
- **Sample Access**: Direct float32 pairs from file start
- **Metadata**: Minimal metadata in STRM/STRT chunks
- **Calculation**: `total_samples = stream_offset / 8` (bytes per Complex32)

---

## Related Specifications

For HTTP streaming protocol and real-time data access, see [HTTPSPEC.md](HTTPSPEC.md).

## Implementation Guidelines

### Parser Architecture

1. **Format Detection**
   ```rust
   if file.find_samp_chunks().is_empty() {
       // Reverse-order format
       parse_raw_iq_data();
   } else {
       // Standard format
       parse_structured_chunks();
   }
   ```

2. **Chunk Discovery**
   - Start from DSFT trailer at file end
   - Follow stream_offset to find metadata area
   - Use pointer chains for linked chunks (SSTR, ANTA, MDTT)

3. **Error Handling**
   - Graceful degradation for missing optional chunks
   - Validate chunk sizes and types
   - Handle compression and different sample formats

4. **Performance Optimization**
   - Pre-allocate sample buffers
   - Use bulk operations for sample conversion
   - Implement chunk caching for random access

### Sample Processing

```rust
// Float32 IQ samples (most common)
fn parse_iq_float32(data: &[u8]) -> Vec<Complex32> {
    data.chunks_exact(8)
        .map(|chunk| {
            let i = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let q = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            Complex32::new(i, q)
        })
        .collect()
}

// Int16 with scaling
fn parse_iq_int16(data: &[u8], scale: f32) -> Vec<Complex32> {
    data.chunks_exact(4)
        .map(|chunk| {
            let i = i16::from_le_bytes([chunk[0], chunk[1]]) as f32 * scale;
            let q = i16::from_le_bytes([chunk[2], chunk[3]]) as f32 * scale;
            Complex32::new(i, q)
        })
        .collect()
}
```


### Memory Management

- Use buffer pools for high-throughput scenarios
- Implement streaming readers for large files
- Consider memory-mapped file access for random access patterns

### Compression Support

RTSA files support various compression algorithms:
- Compression level -1: No compression
- Compression level 0-9: Standard compression levels
- Custom decompression may be required for specific devices

---

## Revision History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2025-01-11 | Initial specification from official PDF documentation |
| 2.0 | 2025-01-11 | Enhanced with comprehensive file format specification |
| 2.1 | 2025-01-11 | Separated HTTP streaming to HTTPSPEC.md |

---

## Sources and Attribution

This specification is a compiled, community-maintained document. It is **not**
published or endorsed by Aaronia AG, and it may lag or diverge from the
vendor's own materials. For authoritative, vendor-published references,
consult:

- **RTSA-Suite PRO file format** (Aaronia V6 forum) — [v6-forum.aaronia.de/forum/topic/rtsa-suite-pro-file-format](https://v6-forum.aaronia.de/forum/topic/rtsa-suite-pro-file-format/)
- Aaronia RTSA-Suite PRO product documentation / file-format PDF.

The content here is derived from the above plus empirical analysis of capture
files produced by RTSA-Suite PRO. The IQ compression format in particular is
undocumented and proprietary (see [Compression and Decompression](#compression-and-decompression)).
"Aaronia", "RTSA", "Spectran", and the file formats they describe are the
property of Aaronia AG.

---

*This specification covers the complete Aaronia RTSA file format for offline RF data storage and analysis. For real-time HTTP streaming protocols, see HTTPSPEC.md.*
