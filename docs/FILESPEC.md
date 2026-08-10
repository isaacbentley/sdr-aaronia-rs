# Aaronia RTSA File Format Specification

## Overview

The Aaronia Real-Time Spectrum Analyzer (RTSA) file format is a chunk-based binary format designed for storing high-performance RF sample data, spectrum analysis results, and associated metadata. This specification covers both standard RTSA files and reverse-order variants, as well as HTTP streaming protocol variants.

This document outlines the complete structure of the Aaronia RTSA file format, based on the official PDF documentation, empirical analysis, and implementation experience.

> **Status & attribution.** This document is a *community-compiled* reference, **not** an official Aaronia specification. It is assembled from public posts on the Aaronia V6 forum, Aaronia's product documentation, and empirical analysis of capture files. Where these disagree, the vendor's own materials are authoritative. See [Sources and Attribution](#sources-and-attribution) for the upstream, vendor-published references.

## Table of Contents

- [File Structure](#file-structure)
- [Chunk Hierarchy](#chunk-hierarchy)
- [Chunk Definitions](#chunk-definitions)
- [Additional Elements](#additional-elements)
- [Implementation Details and Examples](#implementation-details-and-examples)
- [File Format Variants](#file-format-variants)
- [Implementation Guidelines](#implementation-guidelines)
- [Revision History](#revision-history)
- [Related Specifications](#related-specifications)
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
*   **Crate limitation:** this crate's reader currently parses a single segment and stream (the one reachable from the trailing `DSFT`) and does not follow the backward `STRM` chain to earlier streams in appended files. The `SSTR`/`ANTA`/`MDTT` backward chains *are* walked, with cycle guards.

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

> Note: this crate preserves the on-disk `mHeaderSize` only for `SAMP`
> chunks, where it is used to locate the payload; the other chunk
> readers consume their fields sequentially and synthesize a
> payload-size value instead.

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

> **Layout verified against captures** — this standard layout is the
> *only* STRM layout, at every chunk size. The official spec's worked
> example is a 40-byte STRM (16-byte chunk header + the 24 bytes above);
> real RTSA-Suite captures write 48-byte STRM chunks that carry the same
> three fields plus one undocumented trailing `double`: the
> **stream-relative capture start**. `mStartTime` is the stream's clock
> zero, not the first sample — in both LFS test captures the trailing
> double equals the first SAMP packet time and the first SPRV preview
> time exactly, and `mStartTime + offset` agrees with the DSFT
> completion time minus `STRT::mEndTime` to within 30 ms. The Rust
> reader (`StrmChunk::read_from` in `file_source.rs`) parses it into
> `capture_start_offset` when the chunk is exactly 48 bytes and the
> value is plausible (finite, `0 ≤ v < ~10 years`), and the metadata
> layer anchors `start_time_ns` with it so the reported time span
> matches the recorded data rather than the stream clock. An earlier revision dispatched
> 40-byte chunks into a fabricated alternate "proximity" layout
> (u32 id / stream type / f32 rate / f32 frequency / device name);
> no spec revision, capture, or writer produces that layout, and it
> misparsed exactly the minimal spec-conformant chunks — it has been
> removed.

---

### 4. Stream Tail (`STRT`)
Marks the end of a stream and contains offsets to its metadata.

> **Wire-format alignment (verified)** — like SSTR, the recorded STRT
> payload follows standard C-struct alignment: the three `quint32`
> preview counters leave the stream 4-byte aligned, so the compiler
> inserts **4 bytes of padding** before the 8-byte-aligned `mEndTime`
> double. The official spec's worked example shows the padding
> explicitly (`xx xx xx xx - padding` between `mNumPreviewSegments`
> and `mEndTime`), and in both LFS test captures the padding bytes are
> uninitialised garbage (`ff 7f 00 00`) while the padded decode yields
> an `mEndTime` exactly matching the last preview time. A reader that
> skips the padding gets the correct duration; one that doesn't gets a
> garbage end time *and* a garbage antenna offset assembled from the
> two halves of the double. `StrtChunk::read_from` in `file_source.rs`
> seeks past the 4 bytes and uses `HEADER_SIZE = 80` for the fixed
> fields.
>
> **Size-versioned tail (verified)** — the trailing offsets are only
> present when the chunk is large enough. The official example is an
> 88-byte STRT ending at `mAntennaOffset` (no `mMetaDataOffset`);
> RTSA-Suite captures write 104-byte STRTs carrying both offsets plus
> 8 trailing bytes (two undocumented `quint32` values) that readers
> should skip. The Rust reader defaults absent tail offsets to 0.

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
| *(padding)* | 4 bytes | Alignment padding before the 8-byte-aligned `mEndTime`. |
| `mEndTime` | `double` | End time of the stream, relative to the stream start time (i.e. the stream duration). |
| `mAntennaOffset` | `qint64` | Offset of the last antenna chunk (`ANTA`). Present when the chunk is ≥ 88 bytes. |
| `mMetaDataOffset` | `qint64` | Offset of the last metadata type chunk (`MDTT`). Present when the chunk is ≥ 96 bytes. |

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

> **Size note** — the fixed fields below total exactly **248 bytes**
> (8 + 8 + 128 + 8 + 8 + 4 + 4 + 64 + 16), which is the value
> `AntaChunk::read_from` uses when seeking past the segment payload to
> the end of the chunk.

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

> **Wire-format alignment / size note (verified)** — the two `quint8`
> fields are followed by **6 bytes of alignment padding** before the
> 8-byte-aligned `mPreviewOffsets` array, so the fixed fields total
> exactly **392 bytes** (2 + 6 + 3 × 128). Both LFS test captures
> confirm this: preview offsets/times/samples decode correctly at that
> layout, and the padding bytes are uninitialised garbage in some
> chunks. `SprvChunk::read_from` uses `HEADER_SIZE = 392` when seeking
> past the histogram/waterfall payload to the end of the chunk.

| Field | Data Type | Description |
| :--- | :--- | :--- |
| `mPreviewLevel` | `quint8` | Level of this chunk in the preview tree (0 for leaf nodes). |
| `mPreviewCount` | `quint8` | Number of preview elements in this chunk. |
| *(padding)* | 6 bytes | Alignment padding before the 8-byte-aligned `mPreviewOffsets`. |
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
| `mCompression` | `qint32:8` | Compression type (0 for uncompressed, 1-31 for lossy factor). |
| `mPacketStartTime` | `double` | Start time of this chunk relative to stream start. |
| `mPacketEndTime` | `double` | End time of this chunk relative to stream start. |
| `mPacketFlags` | `quint32` | Packet flags (`DSPPF_*`). |
| `mSampleSize` | `quint32` | Size of an individual sample. This crate treats it as the byte stride of one sample when seeking within IQ payloads (8 for float32 IQ); for spectra the vendor dump annotates it as the bin count — the two interpretations have not been reconciled against vendor documentation. |
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
| `DSPPF_WARN_RESAMPLED` | `0x00000800` | The data has been resampled. |
| `DSPPF_REPLAY` | `0x00001000` | The media sample is the start of a replay. |
| `DSPPF_IMMEDIATE` | `0x00002000` | The media sample is supposed to be processed immediately and displayed as a single update. |
| `DSPPF_TIME_OVERLAP` | `0x00004000` | Start time of this sample may be before end time of previous sample. |
| `DSPPF_PUSH` | `0x00008000` | Push the packet through the chain to the display; do not delay or combine. |
| `DSPPF_TIME_DISCONTINUITY` | `0x00010000` | There is a time discontinuity between this and the previous packet. |
| `DSPPF_WARN_DIRECTION` | `0x00020000` | The direction of the stream has changed (e.g. for direction-finding antennas). |
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
*   **Offsets:** All file offsets are 64-bit integers (declared `qint64` in the chunk tables above) representing an absolute position from the start of the file (byte 0). A zero or negative offset terminates a backward chain.
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
`file_source::rtsa_epoch_seconds` divides any input ≥ 10¹³ by 10⁶ before
returning it as seconds-since-epoch (non-finite or non-positive inputs
pass through unchanged). The cutoff sits well above any realistic
Unix-seconds timestamp this century (2025 ≈ 1.7×10⁹) and well below any
plausible Unix-microseconds timestamp from the same era (2025 ≈ 1.7×10¹⁵).
All *epoch-anchored* time fields the binding publishes pass through that
helper; `stream_start_time` (STRM's `mStartTime`) is published raw,
consistent with the report above that it stays in seconds.
`StreamTailInfo::end_time` is also exempt: `STRT::mEndTime` is a
stream-relative duration, not an epoch timestamp, and is published
as-is (the binding anchors it to the STRM start time when deriving the
absolute `end_time_ns`).

---

### Enum and Flag Definitions

Several chunks use enums or flags to specify data types or features. The most important ones are for the `SAMP` chunk.

> **Numbering (verified against the official spec)** — the numeric
> values below are transcribed from the enum declarations in Aaronia's
> official file-format document (rev. 4). Note the sample-type
> ordering: each width's *unsigned* variant is immediately followed by
> its signed sibling (`U8, U16, S16, U32, S32, F32`), rather than all
> unsigned then all signed — an earlier revision of the
> Rust reader had `S16`/`U32` swapped and omitted `DSST_U32N` (9)
> entirely, so a file using value 9 failed to open. The reader now
> maps any value outside the specified range of these three `SAMP`
> enums to an `Unknown` variant and skips chunks it cannot decode,
> instead of rejecting the whole file.

#### `SAMP` Chunk: `mSampleType`
Specifies the data type of individual data elements.

| Value | Enum (`DPSStreamSampleType`) | Description |
| :--- | :--- | :--- |
| 0 | `DSST_U8` | Unsigned 8-bit integer. |
| 1 | `DSST_U16` | Unsigned 16-bit integer. |
| 2 | `DSST_S16` | Signed 16-bit integer. |
| 3 | `DSST_U32` | Unsigned 32-bit integer. |
| 4 | `DSST_S32` | Signed 32-bit integer. |
| 5 | `DSST_F32` | 32-bit float. |
| 6–11 | `DSST_U8N` … `DSST_F32N` | "Packet storage" variants of 0–5 in the same order (`U8N`, `U16N`, `S16N`, `U32N`, `S32N`, `F32N`), where elements are not stored on 16-byte boundaries. |

#### `SAMP` Chunk: `mSampleUnit`
Specifies the physical unit for the sample data.

| Value | Enum (`DSPStreamSampleUnit`) | Description |
| :--- | :--- | :--- |
| 0 | `DSSU_GENERIC` | Generic floating-point value. |
| 1 | `DSSU_DBM` | Decibel-milliwatts. |
| 2 | `DSSU_PERCENTAGE` | Percentage (0 to 1). |
| 3 | `DSSU_DBM_HZ` | Decibel-milliwatts per Hertz. |
| 4 | `DSSU_DBM_M2` | Decibel-milliwatts per square meter. |
| 5 | `DSSU_INDEX` | Integer index. |
| 6 | `DSSU_PHASE` | Phase from -π to +π. |
| 7 | `DSSU_SIGNED_1` | Signed floating point in the range -1 to 1. |
| 8 | `DSSU_UNSIGNED_1` | Unsigned floating point in the range 0 to 1. |
| 9 | `DSSU_TIME` | Floating-point seconds. |
| 10 | `DSSU_DATE_TIME` | Floating-point seconds since the Unix epoch. |
| 11 | `DSSU_HZ` | Hertz. |
| 12 | `DSSU_HZ_LOG` | Logarithmic Hertz. |
| 13 | `DSSU_WATT` | Watts. |
| 14 | `DSSU_SECTOR` | Sector index. |
| 15 | `DSSU_SYMBOL` | Symbol vector. |
| 16 | `DSSU_DB` | Decibels. |
| 17 | `DSSU_NUMERIC` | No unit. |
| 18 | `DSSU_HZ_LOG_CENTER` | Logarithmic Hertz relative to the center frequency. |
| 19 | `DSSU_VOLT` | Volts. |
| 20 | `DSSU_LOG_PERCENTAGE` | Logarithmic percentage (0 to 1). |

The Rust reader maps all 21 values in table order; values outside 0–20
degrade to an `Unknown` variant rather than rejecting the file.

#### `SAMP` Chunk: `mPayloadType`
Specifies the high-level structure of the sample data.

| Value | Enum (`DSPStreamPayloadType`) | Description |
| :--- | :--- | :--- |
| 0 | `DSPT_GENERIC` | Generic numeric data. |
| 1 | `DSPT_AUDIO` | Audio samples. |
| 2 | `DSPT_IQ` | IQ samples (two values per sample). |
| 3 | `DSPT_SPECTRA` | Power spectra. |
| 4 | `DSPT_DETECTION` | Detection probability. |
| 5 | `DSPT_HISTOGRAM` | Histogram data. |
| 6 | `DSPT_ENERGY` | Energy. |
| 7 | `DSPT_VECTOR3` | 3D vectors. |
| 8 | `DSPT_STRUCTURED` | Structured data using meta-data types. |
| 9 | `DSPT_IQ_SLICE` | Slices of IQ samples. |
| 10 | `DSPT_IMAGE` | Grayscale image. |

---

### Structured Data and Meta Data Types (`MDTT`)

The `MDTT` chunk allows for defining complex, hierarchical data structures. This is a "type of types" system.

*   **Base Types:** A small set of base types are defined (`MT_BOOL`, `MT_INTEGER`, `MT_FLOAT`, `MT_STRING`).
*   **Type Constructors:** These base types can be combined using three constructors:
    *   `MT_VECTOR`: A fixed-size array of elements.
    *   `MT_ARRAY`: A variable-size array of elements.
    *   `MT_OBJECT`: A structure with named child elements (fields). (This crate caps nesting depth at 32 levels and accepts up to 4096 fields per object.)
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

This crate implements the spectra decoder in `decompression.rs`, but currently applies it only on the HTTP streaming path. The file reader reads spectra `SAMP` payloads as raw `f32` without checking `mCompression` — compressed spectra in a file are not yet decoded.

### Compression of IQ Data (`DSPT_IQ`)

**Note:** According to Aaronia representatives on their official forums, the specifics of the IQ compression algorithm are considered internal and proprietary, and have not been published in the official file format specification.

Empirical testing (specifically with CW signals) showed that compressed IQ datasets can sometimes be partially recovered by treating the payload as a flat Rice-coded bitstream, zero-padding the coefficient matrix, and applying a 2D inverse Haar wavelet transform. That fallback perfectly reconstructs sparse signals, but broadband captures decoded this way contain significant artifacts due to undocumented proprietary padding or block framing.

**This crate does not implement that fallback.** `Decompressor::decompress` rejects `DSPT_IQ`-shaped inputs with an error, and the in-band read path likewise refuses compressed IQ chunks. Instead, `RtsaSource::open` shells out to Aaronia's own `RTSAFileTool repair -compress=0` (located via the RTSA-Suite installation directory or `AARONIA_SDK_PATH`) to rewrite the capture uncompressed into a temporary file, which is then reopened. Opening a compressed-IQ capture without `RTSAFileTool` installed fails with an actionable error.

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

(This crate currently parses only the 16-slot offset/time/sample-index arrays of `SPRV` chunks; the histogram/waterfall payload these constants describe is not consumed.)

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

All multi-byte values are little-endian. There is no explicit element-count field: `MT_OBJECT` reads `mCount` elements, while `MT_ARRAY` and `MT_VECTOR` read exactly one element (the element type). Each element is a 128-byte zero-padded name, a `quint32` flags word, and a nested type definition.

```
// MetaType {id=1, type=MT_ARRAY, flags=0, count=0}
01 00 00 00 00 00 00 00  // mID = 1
06                       // mType = MT_ARRAY (6)
00 00 00 00              // mFlags = 0
00 00 00 00              // mCount = 0

// Element (MT_ARRAY has exactly one element: its element type)
00 00 ... (128 bytes)    // mName = "" (empty, zero-padded char[128])
00 00 00 00              // mFlags = 0

// Nested type {id=2, type=MT_INTEGER, flags=16bit|signed, count=0}
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

> **Divergence resolved:** an earlier revision of this crate dispatched
> 40-byte STRM chunks (`mChunkSize = 0x28`, as in this dump) into a
> fabricated "proximity" layout that would have misparsed exactly this
> official example. Verification against the vendor PDF and the LFS
> test captures confirmed the standard layout is the only one — see
> the layout note in the STRM chunk definition above. The reader now
> parses every STRM with the standard layout.

#### Sample Packet (SAMP)
```
53 41 4D 50      mChunkID
40 70 00 00      mChunkSize
...
07 00...         mStreamID
03 00 00 00      mSubStreamID
05               mSampleType (DSST_F32)
01               mSampleUnit (DSSU_DBM)
03               mPayloadType (DSPT_SPECTRA)
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
58 00 00 00      mNumPreviewSegments
xx xx xx xx      (alignment padding before mEndTime)
78 F8...         mEndTime
40 00...         mAntennaOffset
...
```

> **Size caveat resolved:** this dump's `mChunkSize` (0x58 = 88) is
> consistent with the padded layout once the tail is understood to be
> size-versioned: 72 payload bytes cover the fields through
> `mAntennaOffset` *including* the 4-byte alignment padding before the
> 8-byte-aligned `mEndTime`, with no `mMetaDataOffset` in this older
> variant. RTSA-Suite captures write 104-byte STRTs carrying both tail
> offsets plus 8 trailing bytes. The reader skips the padding and reads
> the tail offsets only when the chunk size says they are present —
> see the alignment note in the STRT chunk definition above.

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

1. **Format Detection** (pseudocode — in this crate the corresponding
   paths are `RtsaSource::scan_for_samp_chunks`, `read_raw_iq_samples`,
   and `parse_rtsa_with_tail`)
   ```rust
   if samp_chunks.is_empty() {
       // Reverse-order format: raw IQ from file start
       read_raw_iq_samples();
   } else {
       // Standard format: walk the chunk structure
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

Illustrative decoders (this crate's file path implements the equivalents
as `read_iq_f32` / `read_iq_i16` over a buffered reader; the int16
`scale` is derived from the SSTR `mValueMinimum`/`mValueMaximum` range,
falling back to `1.0 / 32768.0`):

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
- Implement streaming readers for large files (this crate uses buffered I/O via `BufReader`; memory-mapped access is a possible alternative for random-access-heavy workloads)

### Parser Limits (this crate)

Defensive caps enforced by this crate's parser — a conforming file should never hit them, but a reader implemented from this spec alone would:

| Limit | Value |
| :--- | :--- |
| Maximum chunk size | 1 GB |
| Maximum MDTT payload | 16 MiB |
| MDTT nesting depth / fields per object | 32 / 4096 |
| DSFT trailer search window | last 1024 bytes of the file |
| STRT/DSFH proximity search windows | 4096 bytes |
| Proximity SAMP scan span | 100 MB |
| Maximum decompressed samples per block | 2²⁴ |

### Compression Support

See [Compression of Spectrum Data](#compression-of-spectrum-data-dspt_spectra) and [Compression of IQ Data](#compression-of-iq-data-dspt_iq): `mCompression` 0 means uncompressed, 1–31 is the lossy wavelet factor, and IQ decompression is proprietary (handled by delegating to `RTSAFileTool`).

---

## Revision History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2025-01-11 | Initial specification from official PDF documentation |
| 2.0 | 2025-01-11 | Enhanced with comprehensive file format specification |
| 2.1 | 2025-01-11 | Separated HTTP streaming to HTTPSPEC.md |
| 2.2 | 2026-08-06 | Corrected against the Rust implementation: `RTSAFileTool` delegation for compressed IQ, enum numeric values, MDTT element layout, STRM/STRT size caveats, parser limits, and time-normalization scope |
| 2.3 | 2026-08-08 | Verified against the official file-format PDF (rev 4) and the LFS test captures: STRT alignment padding and size-versioned tail, SPRV/ANTA fixed-field sizes, single standard STRM layout (proximity layout removed), official `DSST`/`DSSU`/`DSPT` numbering with `Unknown` fallback, `mEndTime` documented as stream-relative duration; resolved the v2.2 STRM/STRT caveats |

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
undocumented and proprietary (see [Compression of IQ Data](#compression-of-iq-data-dspt_iq)).
"Aaronia", "RTSA", "Spectran", and the file formats they describe are the
property of Aaronia AG.

---

*This specification covers the complete Aaronia RTSA file format for offline RF data storage and analysis. For real-time HTTP streaming protocols, see HTTPSPEC.md.*
