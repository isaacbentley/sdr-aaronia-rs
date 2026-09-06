//! Decompression logic for RTSA files.
//!
//! # Status: spectra decode validated; native IQ decode UNSUPPORTED
//!
//! `Decompressor::unpack_coefficients` decodes the variable-length
//! Rice symbols exactly as documented in `docs/FILESPEC.md`
//! ("Compression Bit-Packing Codes") — see
//! `tests::unpack_matches_filespec_code_table`. The spectra compression
//! (`DSPT_SPECTRA`) is documented by Aaronia and decoded reliably.
//!
//! The `DSPT_IQ` compression, however, is a **proprietary, undocumented**
//! Aaronia format. A clean-room reverse-engineering effort got as far as
//! the 48-byte independent Rice block structure, but the coefficient
//! transform remains unknown, so [`Decompressor::decompress`] **rejects**
//! `DSPT_IQ`-shaped inputs (`num_rows == 1 && num_cols >= 128`) with an
//! error rather than emitting wrong samples. Compressed-IQ files are
//! handled at a higher level: `RtsaSource::open` transparently
//! decompresses them through the official `RTSAFileTool` when the
//! RTSA-Suite is installed.

use crate::{Error, Result};

struct BitReader<'a> {
    bytes: &'a [u8],
    byte_pos: usize,
    reservoir: u64,
    bits_in_reservoir: usize,
}

impl<'a> BitReader<'a> {
    /// Creates a new BitReader.
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            byte_pos: 0,
            reservoir: 0,
            bits_in_reservoir: 0,
        }
    }

    /// Fills the 64-bit reservoir from the byte slice.
    fn fill(&mut self) {
        while self.bits_in_reservoir <= 56 && self.byte_pos < self.bytes.len() {
            self.reservoir = (self.reservoir << 8) | (self.bytes[self.byte_pos] as u64);
            self.bits_in_reservoir += 8;
            self.byte_pos += 1;
        }
    }

    /// Current bit position in the stream.
    fn pos(&self) -> usize {
        (self.byte_pos * 8) - self.bits_in_reservoir
    }

    /// Reads a single bit.
    fn read_bit(&mut self) -> Result<u8> {
        if self.bits_in_reservoir == 0 {
            self.fill();
            if self.bits_in_reservoir == 0 {
                return Err(Error::FileFormat {
                    offset: 0,
                    reason: "Not enough bits".to_string(),
                });
            }
        }
        self.bits_in_reservoir -= 1;
        Ok(((self.reservoir >> self.bits_in_reservoir) & 1) as u8)
    }

    /// Reads a specified number of bits (max 32).
    fn read_bits(&mut self, n: usize) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        if self.bits_in_reservoir < n {
            self.fill();
            if self.bits_in_reservoir < n {
                return Err(Error::FileFormat {
                    offset: 0,
                    reason: "Not enough bits".to_string(),
                });
            }
        }
        self.bits_in_reservoir -= n;
        Ok(((self.reservoir >> self.bits_in_reservoir) & ((1 << n) - 1)) as u32)
    }
}

/// Decompresses spectrum data from RTSA files.
pub struct Decompressor;

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Decompressor {
    /// Creates a new decompressor.
    pub fn new() -> Self {
        Self
    }

    /// Unpacks coefficients from a byte slice using a variant of Rice coding.
    fn unpack_coefficients(&self, data: &[u8]) -> Result<Vec<i32>> {
        const MAX_DECOMPRESSED_SAMPLES: usize = 1 << 24;
        let mut reader = BitReader::new(data);
        let mut coefficients = Vec::new();

        while reader.pos() < data.len() * 8 {
            if coefficients.len() >= MAX_DECOMPRESSED_SAMPLES {
                return Err(Error::FileFormat {
                    offset: 0,
                    reason: "Decompression output exceeded limit".to_string(),
                });
            }
            let mut leading_zeros = 0usize;
            while let Ok(bit) = reader.read_bit() {
                if bit == 0 {
                    leading_zeros += 1;
                } else {
                    break;
                }
            }
            // A run of leading zeros longer than any real code is
            // trailing zero padding. This frequently occurs in DSPT_IQ
            // 48-byte blocks that do not fully saturate their space.
            // `LEADING_ZEROS` tiers cover magnitudes up to 2^(3·MAX+2);
            // past `MAX_TIER` the value exceeds any plausible quantised
            // coefficient and the offset arithmetic below would overflow,
            // so stop.
            const MAX_TIER: usize = 9;
            if leading_zeros > MAX_TIER {
                break;
            }

            // Per FILESPEC.md "Compression Bit-Packing Codes": the code
            // is `leading_zeros` zero bits, a `1` stop bit, then a
            // `3·(L+1)`-bit residual. The low residual bit is the sign;
            // the remaining `3L+2` bits are the magnitude *within* tier
            // L, which begins where the previous tier ended. So tier 0
            // (`1xxx`) covers magnitudes 0–3, tier 1 (`01xxxxxx`) covers
            // 4–35, etc. The earlier decoder omitted this per-tier
            // offset and decoded `0100 0000` as 0 instead of +4.
            let residual_bits = 3 * (leading_zeros + 1);
            if reader.pos() + residual_bits > data.len() * 8 {
                // Not enough bits left for the residual — trailing padding.
                break;
            }

            let residual = reader.read_bits(residual_bits)?;
            let sign = residual & 1;
            let mut magnitude = (residual >> 1) as i64;
            // Cumulative count of all magnitudes in lower tiers:
            // Σ_{k=0}^{L-1} 2^(3k+2).
            for k in 0..leading_zeros {
                magnitude += 1i64 << (3 * k + 2);
            }

            let value = if sign == 1 {
                -(magnitude as i32)
            } else {
                magnitude as i32
            };
            coefficients.push(value);
        }

        Ok(coefficients)
    }

    /// Dequantizes the coefficients. Caller must pass `compression_factor >= 1`;
    /// `0` indicates an uncompressed payload and `decompress` will short-circuit
    /// before reaching this function.
    fn dequantize(&self, coefficients: &[i32], compression_factor: u32) -> Result<Vec<f32>> {
        debug_assert!(
            compression_factor >= 1,
            "dequantize precondition violated; decompress() should have rejected this"
        );
        let quant = 0.1 * (1 << (compression_factor - 1)) as f32;
        Ok(coefficients.iter().map(|&c| c as f32 * quant).collect())
    }

    /// Performs an inverse wavelet transform on the data.
    fn inverse_wavelet_transform(&self, data: &mut [f32], num_rows: usize, num_cols: usize) {
        let mut step = 1;
        while (num_rows & (2 * step - 1)) == 0 {
            step *= 2;
        }
        while (num_cols & (2 * step - 1)) == 0 {
            step *= 2;
        }

        while step > 1 {
            step >>= 1;
            if (num_cols & (2 * step - 1)) == 0 {
                self.wave_transform_step(data, 2 * step, step, step, num_cols);
            }
            if (num_rows & (2 * step - 1)) == 0 {
                self.wave_transform_step(data, step, 2 * step, step * num_cols, num_cols);
            }
        }
    }

    /// A single step of the inverse wavelet transform.
    fn wave_transform_step(
        &self,
        data: &mut [f32],
        sx: usize,
        sy: usize,
        dxy: usize,
        num_cols: usize,
    ) {
        let sqrt_half = (0.5f32).sqrt();
        let num_rows = data.len() / num_cols;

        // Iterate over rows properly (y represents row indices, not flat array indices)
        for row in (0..num_rows).step_by(sy) {
            for col in (0..num_cols).step_by(sx) {
                let idx1 = row * num_cols + col;
                let idx2 = idx1 + dxy;

                // Bounds check to prevent index out of bounds
                if idx2 < data.len() {
                    let s = data[idx1];
                    let t = data[idx2];
                    data[idx1] = sqrt_half * (s + t);
                    data[idx2] = sqrt_half * (s - t);
                }
            }
        }
    }

    /// Decompresses a block of data.
    ///
    /// `compression_factor == 0` indicates an uncompressed payload — there is
    /// no Rice/wavelet codestream to decode. Callers must guard with
    /// `if compression_factor > 0` and read the data directly in that case;
    /// passing 0 here is a precondition violation.
    pub fn decompress(
        &self,
        data: &[u8],
        compression_factor: u32,
        num_rows: usize,
        num_cols: usize,
    ) -> Result<Vec<f32>> {
        if compression_factor == 0 {
            return Err(Error::Protocol(
                "Decompressor::decompress called with compression_factor = 0; \
                 the payload is uncompressed — read it directly"
                    .to_string(),
            ));
        }
        // Per docs/FILESPEC.md, `mCompression` is documented as "1 to 31
        // for lossy factor" — this value can originate from a parsed
        // network packet (HTTP streaming metadata) or file header, so it
        // must be validated here rather than trusted by `dequantize`,
        // which shifts `1i32 << (compression_factor - 1)`: at 32 that
        // overflows the sign bit (producing a negative, corrupt
        // quantizer) and at 33+ it's an arithmetic-overflow panic in
        // debug builds.
        if compression_factor > 31 {
            return Err(Error::Protocol(format!(
                "Decompressor::decompress: compression_factor {compression_factor} is out of \
                 the documented range (1-31)"
            )));
        }

        // Zero dimensions have no valid transform and would wedge the
        // inverse wavelet step: `num_rows == 0` spins the `step`-doubling
        // loop until it overflows, and `num_cols == 0` divides by zero in
        // `wave_transform_step`. Reject up front (internal callers always
        // pass >= 1, but this is a `pub` entry point).
        if num_rows == 0 || num_cols == 0 {
            return Err(Error::Protocol(format!(
                "Decompressor::decompress requires non-zero dimensions \
                 (got num_rows={num_rows}, num_cols={num_cols})"
            )));
        }

        if num_rows == 1 && num_cols >= 128 {
            // IQ sample data (DSPT_IQ) uses a proprietary Aaronia compression format.
            // Our cleanroom reverse-engineering attempt determined that native Rust
            // decompression is currently infeasible without the official specifications.
            // The fallback must be to use the Aaronia SDK/DLL for DSPT_IQ payloads.
            return Err(Error::Protocol(
                "DSPT_IQ decompression requires the Aaronia SDK/DLL. Native decompression is proprietary and currently unsupported.".to_string()
            ));
        }

        let coefficients = self.unpack_coefficients(data)?;
        let mut dequantized = self.dequantize(&coefficients, compression_factor)?;
        // Dimensions come from packet metadata; cap them like the decoded
        // coefficient count, or a corrupt header sizes a huge allocation.
        const MAX_OUTPUT_VALUES: usize = 1 << 24;
        let expected_len = num_rows
            .checked_mul(num_cols)
            .filter(|n| *n <= MAX_OUTPUT_VALUES)
            .ok_or_else(|| {
                Error::Protocol(format!(
                    "decompress: {num_rows} x {num_cols} values exceeds the {MAX_OUTPUT_VALUES} limit"
                ))
            })?;
        if dequantized.len() < expected_len {
            tracing::warn!(
                "decompress: coefficient stream produced {} of {} expected values \
                 (truncated or corrupt payload); zero-padding the remainder",
                dequantized.len(),
                expected_len
            );
            dequantized.resize(expected_len, 0.0);
        } else if dequantized.len() > expected_len {
            // `wave_transform_step` derives its own row count from
            // `data.len() / num_cols` rather than trusting `num_rows`, so
            // an over-long buffer would make it operate on more rows
            // than the caller asked for instead of erroring or ignoring
            // the excess. Truncate to the caller's declared dimensions.
            tracing::warn!(
                "decompress: coefficient stream produced {} of {} expected values \
                 (extra trailing data); truncating",
                dequantized.len(),
                expected_len
            );
            dequantized.truncate(expected_len);
        }
        self.inverse_wavelet_transform(&mut dequantized, num_rows, num_cols);
        Ok(dequantized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code in FILESPEC.md's "Compression Bit-Packing Codes" table
    /// must decode to its documented value. This pins the Rice *symbol*
    /// decoder against the authoritative spec examples — in particular
    /// the per-tier value offset (`0100 0000` = +4, not 0) that the
    /// earlier decoder omitted.
    #[test]
    fn unpack_matches_filespec_code_table() {
        // (code bits MSB-first, padded into bytes) -> expected value.
        // Each case is decoded in isolation; trailing zero bits act as
        // padding and terminate the stream cleanly.
        let cases: &[(&[u8], i32)] = &[
            // Tier 0: `1xxx`, magnitudes 0..=3.
            (&[0b1000_0000], 0),  // +0
            (&[0b1001_0000], 0),  // -0  (== 0)
            (&[0b1010_0000], 1),  // +1
            (&[0b1011_0000], -1), // -1
            (&[0b1100_0000], 2),  // +2
            (&[0b1101_0000], -2), // -2
            (&[0b1110_0000], 3),  // +3
            (&[0b1111_0000], -3), // -3
            // Tier 1: `01xxxxxx`, magnitudes 4..=35 (offset +4).
            (&[0b0100_0000], 4),  // +4
            (&[0b0100_0001], -4), // -4
            (&[0b0100_0010], 5),  // +5
            (&[0b0100_0011], -5), // -5
        ];
        let d = Decompressor::new();
        for (bytes, expected) in cases {
            let coeffs = d.unpack_coefficients(bytes).unwrap();
            assert!(!coeffs.is_empty(), "no coefficient decoded from {bytes:?}");
            assert_eq!(
                coeffs[0], *expected,
                "code {bytes:?} decoded to {} (expected {expected})",
                coeffs[0]
            );
        }
    }

    #[test]
    fn test_bit_reader_single_bit() {
        let data = [0b10110100]; // Binary: 10110100
        let mut reader = BitReader::new(&data);

        assert_eq!(reader.read_bit().unwrap(), 1);
        assert_eq!(reader.read_bit().unwrap(), 0);
        assert_eq!(reader.read_bit().unwrap(), 1);
        assert_eq!(reader.read_bit().unwrap(), 1);
        assert_eq!(reader.read_bit().unwrap(), 0);
        assert_eq!(reader.read_bit().unwrap(), 1);
        assert_eq!(reader.read_bit().unwrap(), 0);
        assert_eq!(reader.read_bit().unwrap(), 0);
    }

    #[test]
    fn test_bit_reader_multiple_bits() {
        let data = [0b11010010, 0b01001110]; // Two bytes
        let mut reader = BitReader::new(&data);

        assert_eq!(reader.read_bits(4).unwrap(), 0b1101); // First 4 bits
        assert_eq!(reader.read_bits(3).unwrap(), 0b001); // Next 3 bits
        assert_eq!(reader.read_bits(5).unwrap(), 0b00100); // Next 5 bits spanning byte boundary
    }

    #[test]
    fn test_bit_reader_bounds_checking() {
        let data = [0b10110100]; // Only 8 bits
        let mut reader = BitReader::new(&data);

        // Read all 8 bits
        for _ in 0..8 {
            reader.read_bit().unwrap();
        }

        // Should fail when trying to read beyond bounds
        assert!(reader.read_bit().is_err());
    }

    #[test]
    fn test_decompressor_creation() {
        let decompressor = Decompressor::new();
        // Just asserting we can call methods on it to make sure it's valid
        let empty_data: &[u8] = &[];
        assert!(decompressor.unpack_coefficients(empty_data).is_ok());
    }

    #[test]
    fn test_rice_coding_unpack_simple_values() {
        let decompressor = Decompressor::new();

        // 0b10100000:
        // bit 0: 1 (stop bit) => 0 leading zeros => residual is 3 bits
        // bits 1-3: 010 (residual) => sign=0, value=1 -> coefficient is 1
        // bits 4-15: all 0 (trailing padding)
        let data = [0b10100000, 0x00];

        let result = decompressor.unpack_coefficients(&data);
        assert!(result.is_ok());
        let coeffs = result.unwrap();
        assert!(!coeffs.is_empty());
        assert_eq!(coeffs[0], 1);
    }

    #[test]
    fn test_dequantization_basic() {
        let decompressor = Decompressor::new();
        let coefficients = vec![1, -2, 3, 0];
        let compression_factor = 2;

        let result = decompressor
            .dequantize(&coefficients, compression_factor)
            .unwrap();

        assert_eq!(result.len(), 4);
        let expected_quant = 0.1 * (1 << (compression_factor - 1)) as f32; // 0.1 * 2 = 0.2
        assert_eq!(result[0], 1.0 * expected_quant);
        assert_eq!(result[1], -2.0 * expected_quant);
        assert_eq!(result[2], 3.0 * expected_quant);
        assert_eq!(result[3], 0.0 * expected_quant);
    }

    #[test]
    fn test_dequantization_compression_factors() {
        let decompressor = Decompressor::new();
        let coefficients = vec![5];

        // Test different compression factors
        let result1 = decompressor.dequantize(&coefficients, 1).unwrap();
        let result2 = decompressor.dequantize(&coefficients, 3).unwrap();

        let quant1 = 0.1 * (1 << (1 - 1)) as f32; // 0.1 * 1 = 0.1
        let quant2 = 0.1 * (1 << (3 - 1)) as f32; // 0.1 * 4 = 0.4

        assert_eq!(result1[0], 5.0 * quant1);
        assert_eq!(result2[0], 5.0 * quant2);
        assert!(result2[0] > result1[0]); // Higher compression factor = larger quantization
    }

    #[test]
    fn test_wavelet_transform_2x2_matrix() {
        let decompressor = Decompressor::new();
        let mut data = vec![1.0, 2.0, 3.0, 4.0]; // 2x2 matrix

        // Test 2x2 wavelet transform
        decompressor.inverse_wavelet_transform(&mut data, 2, 2);

        assert_eq!(data.len(), 4);
        // Wavelet transform should modify the data
        // The exact values depend on the transform but data should be transformed
    }

    #[test]
    fn test_wavelet_transform_4x4_matrix() {
        let decompressor = Decompressor::new();
        let mut data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
        ];

        // Test 4x4 wavelet transform
        decompressor.inverse_wavelet_transform(&mut data, 4, 4);

        assert_eq!(data.len(), 16);
        // Wavelet transform should complete without errors
    }

    #[test]
    fn test_wavelet_transform_step_basic() {
        let decompressor = Decompressor::new();
        let mut data = vec![1.0, 0.0, 2.0, 0.0]; // 4 elements for safe indexing

        // Use parameters that don't cause out-of-bounds access
        decompressor.wave_transform_step(&mut data, 2, 2, 1, 2);

        // Test that algorithm runs without panicking and data is modified
        assert_eq!(data.len(), 4);
    }

    #[test]
    fn test_full_decompression_workflow() {
        let decompressor = Decompressor::new();

        // Test the full workflow with minimal data
        let data = [0b10100000]; // Minimal Rice-coded data
        let compression_factor = 2;
        let num_rows = 1;
        let num_cols = 1;

        let result = decompressor.decompress(&data, compression_factor, num_rows, num_cols);

        // Algorithm may fail with minimal test data, so we verify that the error propagates properly,
        // or that it succeeds if the test data happens to be sufficiently padded.
        match result {
            Ok(decompressed) => assert!(!decompressed.is_empty()),
            Err(e) => assert!(
                e.to_string().contains("Not enough bits")
                    || e.to_string().contains("index out of bounds")
            ),
        }
    }

    #[test]
    fn test_decompression_exact_oracle() {
        let decompressor = Decompressor::new();

        // Craft a known-good bitstream for a 1x2 decompression.
        // We want coefficients: [1, -2].
        // Coeff 0: value 1, positive. sign=0, value=1 -> residual = (1 << 1) | 0 = 2 = 0b010.
        // Needs 3 bits, so leading_zeros = 0.
        // Bits: 1 (stop bit), 0, 1, 0 (residual=2) => 1010
        //
        // Coeff 1: value 2, negative. sign=1, value=2 -> residual = (2 << 1) | 1 = 5 = 0b101.
        // Needs 3 bits, so leading_zeros = 0.
        // Bits: 1 (stop bit), 1, 0, 1 (residual=5) => 1101
        //
        // Concat: 1010 1101 => 0xAD
        let data = [0xAD];

        let compression_factor = 2;
        let num_rows = 1;
        let num_cols = 2;

        let result = decompressor
            .decompress(&data, compression_factor, num_rows, num_cols)
            .expect("Decompression failed");

        // Let's manually verify the math we expect.
        // coeffs = [1, -2]
        // dequantized = [0.2, -0.4] (since quant = 0.1 * 2^1 = 0.2)
        // inverse wavelet transform on [0.2, -0.4]:
        // data[0] = sqrt(0.5) * (0.2 + -0.4) = -0.14142136
        // data[1] = sqrt(0.5) * (0.2 - -0.4) = 0.42426407
        assert_eq!(result.len(), 2);

        let sqrt_half = (0.5f32).sqrt();
        let expected_0 = sqrt_half * -0.2;
        let expected_1 = sqrt_half * 0.6;

        assert!(
            (result[0] - expected_0).abs() < 1e-6,
            "got {}, expected {}",
            result[0],
            expected_0
        );
        assert!(
            (result[1] - expected_1).abs() < 1e-6,
            "got {}, expected {}",
            result[1],
            expected_1
        );
    }

    #[test]
    fn decompress_rejects_compression_factor_above_documented_range() {
        // docs/FILESPEC.md documents mCompression as "1 to 31 for lossy
        // factor". A malformed/malicious header claiming 32+ must be
        // rejected here rather than reaching `dequantize`'s
        // `1i32 << (compression_factor - 1)`, which overflows the sign
        // bit at 32 and panics on overflow (debug builds) at 33+.
        let decompressor = Decompressor::new();
        let data = [0xAD];
        for bad_factor in [32, 33, 255, u32::MAX] {
            let result = decompressor.decompress(&data, bad_factor, 1, 2);
            assert!(
                result.is_err(),
                "compression_factor={bad_factor} should be rejected"
            );
        }
    }

    #[test]
    fn decompress_accepts_compression_factor_at_documented_ceiling() {
        let decompressor = Decompressor::new();
        let data = [0xAD];
        // 31 is the top of the documented 1-31 range and must still work.
        assert!(decompressor.decompress(&data, 31, 1, 2).is_ok());
    }

    #[test]
    fn test_empty_data_handling() {
        let decompressor = Decompressor::new();
        let empty_data = [];

        let result = decompressor.unpack_coefficients(&empty_data);
        assert!(result.is_ok());
        let coefficients = result.unwrap();
        assert!(coefficients.is_empty());
    }

    #[test]
    fn test_large_compression_factor() {
        let decompressor = Decompressor::new();
        let coefficients = vec![1, 2, 3];
        let compression_factor = 10;

        let result = decompressor
            .dequantize(&coefficients, compression_factor)
            .unwrap();

        let expected_quant = 0.1 * (1 << (compression_factor - 1)) as f32;
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], 1.0 * expected_quant);
        assert!(expected_quant > 50.0); // Should be a large quantization factor
    }

    #[test]
    fn test_single_coefficient_rice_decoding() {
        let decompressor = Decompressor::new();

        // 0b10000000:
        // bit 0: 1 (stop bit) => 0 leading zeros => residual is 1 bit
        // bit 1: 0 (residual bit) => sign=0, value=0 -> coeff is 0
        let data = [0b10000000, 0b00000000];

        let result = decompressor.unpack_coefficients(&data);
        assert!(result.is_ok());
        let coeffs = result.unwrap();
        assert!(!coeffs.is_empty());
        assert_eq!(coeffs[0], 0);
    }

    #[test]
    fn test_negative_coefficient_handling() {
        let decompressor = Decompressor::new();
        let coefficients = vec![-5, -10, -1];
        let compression_factor = 3;

        let result = decompressor
            .dequantize(&coefficients, compression_factor)
            .unwrap();

        assert_eq!(result.len(), 3);
        assert!(result[0] < 0.0); // Negative coefficient should remain negative
        assert!(result[1] < 0.0);
        assert!(result[2] < 0.0);
    }

    #[test]
    fn test_decompress_rejects_factor_zero() {
        // compression_factor 0 means the payload is uncompressed and
        // should be read directly. The decompress entry
        // point must reject 0 with a helpful error rather than panic from
        // the underflow in (1 << (factor - 1)).
        let decompressor = Decompressor::new();
        let err = decompressor
            .decompress(&[0u8; 4], 0, 1, 1)
            .expect_err("expected error for compression_factor = 0");
        assert!(err.to_string().contains("uncompressed"));
    }

    #[test]
    fn test_decompress_rejects_zero_dimensions() {
        // Zero rows/cols must be rejected before reaching the inverse
        // wavelet transform, which would otherwise spin forever
        // (num_rows == 0) or divide by zero (num_cols == 0).
        let decompressor = Decompressor::new();
        for (rows, cols) in [(0usize, 4usize), (4, 0), (0, 0)] {
            let err = decompressor
                .decompress(&[0u8; 4], 1, rows, cols)
                .expect_err("expected error for zero dimension");
            assert!(
                err.to_string().contains("non-zero dimensions"),
                "unexpected error for ({rows}, {cols}): {err}"
            );
        }
    }

    #[test]
    fn decompress_truncates_over_produced_coefficients() {
        // `test_decompression_exact_oracle` decodes the byte 0xAD into
        // exactly 2 coefficients for a 1x2 request. Requesting 1x1
        // instead (fewer expected values than the stream actually
        // decodes) exercises the over-long branch: the result must be
        // truncated to the requested length, not left at the raw
        // decoded length.
        let decompressor = Decompressor::new();
        let data = [0xAD];
        let result = decompressor
            .decompress(&data, 2, 1, 1)
            .expect("decompression should succeed with truncation, not error");
        assert_eq!(result.len(), 1);
    }
}
