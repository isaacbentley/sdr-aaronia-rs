//! High-Performance RTSA File Access Module
//!
//! This module provides memory-mapped file access and intelligent caching
//! specifically optimized for Aaronia RTSA file format processing.
//!
//! **Standalone**: these readers are an optional, lower-level alternative
//! for callers that manage RTSA chunk offsets themselves (e.g. indexing
//! tools scanning very large captures). The main [`crate::RtsaSource`]
//! path uses buffered `std::io` and does **not** route through this
//! module.

use crate::{Error, Result};
use memmap2::{Mmap, MmapOptions};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

/// Memory-mapped RTSA file reader for large file performance optimization.
///
/// The chunk cache uses a tiered eviction policy (see the private
/// `evict_cache_entries` method below): Header and Metadata chunks
/// are favoured long-term because they're tiny and re-read
/// constantly, Preview and Sample chunks evict by age and hotness.
/// `MAX_CACHE_ENTRIES` is a *hard cap*: if the tiered pass can't
/// bring the cache size back under the cap (which used to be
/// possible because Header/Metadata chunks were never evicted), a
/// final LRU pass force-evicts the oldest entries — including
/// Headers — until the cap is satisfied, so the cache cannot grow
/// unboundedly under sustained load.
pub struct MmapRtsaReader {
    mmap: Mmap,
    file_size: usize,
    /// Keyed by `(offset, size)`: two reads at the same offset with
    /// different lengths are distinct cache entries. (Keying by offset
    /// alone returned a stale short buffer for the longer read.)
    chunk_cache: HashMap<(u64, usize), CachedChunk>,
    access_stats: AccessStats,
}

/// Soft eviction trigger — when the cache exceeds this many entries
/// the tiered policy runs.
const CACHE_EVICTION_THRESHOLD: usize = 2048;

/// Hard upper bound on cache size. After tiered eviction, any
/// remaining excess is force-evicted in LRU order. Cap is sized so
/// the cache cannot exceed ~2× the soft threshold, which on real
/// 64-bit hosts caps the worst-case chunk-cache footprint at a few
/// hundred MB even with full-block SAMP chunks.
const MAX_CACHE_ENTRIES: usize = 4096;

/// Cached chunk data with LRU eviction
#[derive(Clone)]
struct CachedChunk {
    data: Vec<u8>,
    last_access: Instant,
    access_count: u64,
    chunk_type: ChunkType,
}

/// Statistics for memory access patterns
#[derive(Debug, Default)]
pub struct AccessStats {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub total_bytes_read: u64,
    pub sequential_reads: u64,
    pub random_reads: u64,
    pub last_offset: Option<u64>,
}

/// RTSA chunk types for intelligent caching
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChunkType {
    Sample,   // SAMP chunks - frequently accessed
    Metadata, // SSTR, ANTA, MDTT - cached indefinitely
    Preview,  // SPRV chunks - medium priority
    Header,   // DSFH, DSFT, STRM, STRT - small, cache permanently
}

impl MmapRtsaReader {
    /// Create new memory-mapped RTSA reader with intelligent caching
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path.as_ref())?;
        let mmap = unsafe { MmapOptions::new().map(&file)? };
        let file_size = mmap.len();

        Ok(Self {
            mmap,
            file_size,
            chunk_cache: HashMap::with_capacity(1024),
            access_stats: AccessStats::default(),
        })
    }

    /// Read chunk with intelligent caching based on access patterns
    pub fn read_chunk(&mut self, offset: u64, size: usize) -> Result<Vec<u8>> {
        // Update access statistics
        self.update_access_stats(offset);

        // Check cache first
        if let Some(cached) = self.chunk_cache.get_mut(&(offset, size)) {
            cached.last_access = Instant::now();
            cached.access_count += 1;
            self.access_stats.cache_hits += 1;
            return Ok(cached.data.clone());
        }

        // Cache miss - read from memory-mapped file
        self.access_stats.cache_misses += 1;
        self.access_stats.total_bytes_read += size as u64;

        if offset as usize + size > self.file_size {
            return Err(Error::FileFormat {
                offset,
                reason: format!(
                    "Read beyond file bounds: {} + {} > {}",
                    offset, size, self.file_size
                ),
            });
        }

        let data = self.mmap[offset as usize..offset as usize + size].to_vec();
        let chunk_type = self.classify_chunk(&data);

        // Cache the chunk with appropriate eviction policy
        let cached_chunk = CachedChunk {
            data: data.clone(),
            last_access: Instant::now(),
            access_count: 1,
            chunk_type,
        };

        self.chunk_cache.insert((offset, size), cached_chunk);

        // Evict old entries if cache is getting large. The tiered
        // pass favours keeping small Header/Metadata chunks; if it
        // can't bring us back under the hard cap (which is what
        // happened under sustained load with only growing
        // Header/Metadata reads — A19) the LRU fallback runs.
        if self.chunk_cache.len() > CACHE_EVICTION_THRESHOLD {
            self.evict_cache_entries();
        }

        Ok(data)
    }

    /// Classify chunk type for intelligent caching policies
    fn classify_chunk(&self, data: &[u8]) -> ChunkType {
        if data.len() < 4 {
            return ChunkType::Header;
        }

        match &data[0..4] {
            b"SAMP" => ChunkType::Sample,
            b"SSTR" | b"ANTA" | b"MDTT" => ChunkType::Metadata,
            b"SPRV" => ChunkType::Preview,
            b"DSFH" | b"DSFT" | b"STRM" | b"STRT" => ChunkType::Header,
            _ => ChunkType::Header,
        }
    }

    /// Update access pattern statistics for optimization
    fn update_access_stats(&mut self, offset: u64) {
        if let Some(last_offset) = self.access_stats.last_offset {
            if offset > last_offset && offset - last_offset < 4096 {
                self.access_stats.sequential_reads += 1;
            } else {
                self.access_stats.random_reads += 1;
            }
        }
        self.access_stats.last_offset = Some(offset);
    }

    /// Intelligent cache eviction based on access patterns and chunk
    /// types, followed by a hard-cap LRU fallback (A19).
    ///
    /// Tiered pass:
    /// - Header / Metadata: keep (they're tiny and re-read constantly).
    /// - Preview: evict if older than 5 minutes.
    /// - Sample: evict if older than 60 s AND access_count < 3.
    ///
    /// Then, if the cache is *still* over [`MAX_CACHE_ENTRIES`], force-
    /// evict the least-recently-used entries (regardless of chunk
    /// type) until the cap is satisfied. This is what gives the
    /// hard-cap guarantee — under a workload that only ever reads
    /// new Header/Metadata chunks, the tiered pass alone would let
    /// the cache grow without bound.
    fn evict_cache_entries(&mut self) {
        let now = Instant::now();
        let mut entries_to_remove = Vec::new();

        for (key, chunk) in &self.chunk_cache {
            let age = now.duration_since(chunk.last_access);
            let should_evict = match chunk.chunk_type {
                ChunkType::Header | ChunkType::Metadata => false, // Tiered pass favours these.
                ChunkType::Preview => age > Duration::from_secs(300), // 5 minutes
                ChunkType::Sample => {
                    // Evict sample chunks based on access frequency and age
                    age > Duration::from_secs(60) && chunk.access_count < 3
                }
            };

            if should_evict {
                entries_to_remove.push(*key);
            }
        }

        // Remove selected entries
        for key in entries_to_remove {
            self.chunk_cache.remove(&key);
        }

        // Hard-cap fallback: if the tiered pass didn't bring us
        // under the cap (e.g. the cache is mostly Header / Metadata
        // chunks, which the tiered pass never evicts), force-evict
        // the oldest entries in LRU order until we're at the cap.
        if self.chunk_cache.len() > MAX_CACHE_ENTRIES {
            let mut by_age: Vec<((u64, usize), Instant)> = self
                .chunk_cache
                .iter()
                .map(|(key, c)| (*key, c.last_access))
                .collect();
            // Oldest first.
            by_age.sort_by_key(|(_, ts)| *ts);
            let overflow = self.chunk_cache.len() - MAX_CACHE_ENTRIES;
            for (key, _) in by_age.into_iter().take(overflow) {
                self.chunk_cache.remove(&key);
            }
        }
    }

    /// Prefetch chunks based on detected access patterns
    pub fn prefetch_sequential(
        &mut self,
        start_offset: u64,
        chunk_size: usize,
        count: usize,
    ) -> Result<()> {
        for i in 0..count {
            let offset = start_offset + (i * chunk_size) as u64;
            if !self.chunk_cache.contains_key(&(offset, chunk_size)) {
                let _ = self.read_chunk(offset, chunk_size);
            }
        }
        Ok(())
    }

    /// Get cache efficiency statistics
    pub fn get_cache_stats(&self) -> CacheStats {
        let total_requests = self.access_stats.cache_hits + self.access_stats.cache_misses;
        let hit_rate = if total_requests > 0 {
            self.access_stats.cache_hits as f64 / total_requests as f64 * 100.0
        } else {
            0.0
        };

        CacheStats {
            hit_rate,
            total_entries: self.chunk_cache.len(),
            total_memory_mb: self.estimate_cache_memory() / 1024 / 1024,
            sequential_ratio: if self.access_stats.sequential_reads + self.access_stats.random_reads
                > 0
            {
                self.access_stats.sequential_reads as f64
                    / (self.access_stats.sequential_reads + self.access_stats.random_reads) as f64
                    * 100.0
            } else {
                0.0
            },
        }
    }

    /// Estimate cache memory usage
    fn estimate_cache_memory(&self) -> usize {
        self.chunk_cache
            .values()
            .map(|chunk| chunk.data.len())
            .sum::<usize>()
    }
}

/// Cache performance statistics
#[derive(Debug)]
pub struct CacheStats {
    pub hit_rate: f64,
    pub total_entries: usize,
    pub total_memory_mb: usize,
    pub sequential_ratio: f64,
}

/// Adaptive chunk reader that optimizes for detected access patterns
pub struct AdaptiveChunkReader {
    mmap_reader: MmapRtsaReader,
    read_ahead_size: usize,
}

impl AdaptiveChunkReader {
    /// Open `path` as a memory-mapped RTSA file with a 64 KB starting
    /// read-ahead window.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or memory-mapped.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        Ok(Self {
            mmap_reader: MmapRtsaReader::new(path)?,
            read_ahead_size: 64 * 1024, // Start with 64KB read-ahead
        })
    }

    /// Adaptive read that adjusts strategy based on access patterns
    pub fn read_adaptive(&mut self, offset: u64, size: usize) -> Result<Vec<u8>> {
        let stats = &self.mmap_reader.access_stats;
        let sequential_ratio = if stats.sequential_reads + stats.random_reads > 0 {
            stats.sequential_reads as f64 / (stats.sequential_reads + stats.random_reads) as f64
        } else {
            0.0
        };

        // Adjust read-ahead based on access patterns
        if sequential_ratio > 0.8 {
            // Highly sequential access - increase read-ahead
            self.read_ahead_size = std::cmp::min(self.read_ahead_size * 2, 1024 * 1024);

            // Prefetch next chunks
            let chunks_to_prefetch = self.read_ahead_size / size;
            let _ = self.mmap_reader.prefetch_sequential(
                offset + size as u64,
                size,
                chunks_to_prefetch,
            );
        } else if sequential_ratio < 0.3 {
            // Random access - reduce read-ahead
            self.read_ahead_size = std::cmp::max(self.read_ahead_size / 2, 4096);
        }

        self.mmap_reader.read_chunk(offset, size)
    }

    /// Returns the current cache statistics and read-ahead window size
    /// (in bytes).
    pub fn get_performance_stats(&self) -> (CacheStats, usize) {
        (self.mmap_reader.get_cache_stats(), self.read_ahead_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_mmap_reader_caching() -> Result<()> {
        let dir = tempdir()?;
        let file_path = dir.path().join("test.rtsa");

        // Create a test file
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&file_path)?;
        file.write_all(b"SAMP")?;
        file.write_all(&[0u8; 1020])?; // Pad to 1024 bytes
        file.flush()?;
        drop(file);

        let mut reader = MmapRtsaReader::new(&file_path)?;

        // First read - cache miss
        let _data1 = reader.read_chunk(0, 1024)?;
        assert_eq!(reader.access_stats.cache_misses, 1);
        assert_eq!(reader.access_stats.cache_hits, 0);

        // Second read - cache hit
        let _data2 = reader.read_chunk(0, 1024)?;
        assert_eq!(reader.access_stats.cache_misses, 1);
        assert_eq!(reader.access_stats.cache_hits, 1);

        let stats = reader.get_cache_stats();
        assert_eq!(stats.hit_rate, 50.0);
        assert_eq!(stats.total_entries, 1);

        Ok(())
    }

    /// Regression: reads at the same offset but different sizes must not
    /// alias in the cache. The offset-only key served the stale 512-byte
    /// buffer for a subsequent 1024-byte read.
    #[test]
    fn test_cache_distinguishes_sizes_at_same_offset() -> Result<()> {
        let dir = tempdir()?;
        let file_path = dir.path().join("sizes.rtsa");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&file_path)?;
        file.write_all(b"SAMP")?;
        file.write_all(&[0xABu8; 2044])?; // Pad to 2048 bytes
        file.flush()?;
        drop(file);

        let mut reader = MmapRtsaReader::new(&file_path)?;
        let short = reader.read_chunk(0, 512)?;
        let long = reader.read_chunk(0, 1024)?;
        assert_eq!(short.len(), 512);
        assert_eq!(
            long.len(),
            1024,
            "longer read at a cached offset must not return the shorter cached buffer"
        );
        Ok(())
    }

    /// Verify the cache enforces `MAX_CACHE_ENTRIES` as a hard upper
    /// bound even on workloads that the tiered eviction can't shrink
    /// (e.g. all Header chunks, which the tiered pass treats as
    /// permanent).
    ///
    /// Populates the cache directly with `MAX_CACHE_ENTRIES + 64`
    /// Header chunks (whose timestamps form a strict ordering), then
    /// calls the eviction path and asserts the cache shrank to
    /// exactly `MAX_CACHE_ENTRIES`. Skips the public `read_chunk`
    /// API because driving it would mean creating a several-MB file
    /// just to exercise an in-memory invariant.
    #[test]
    fn test_cache_hard_cap_lru_fallback() -> Result<()> {
        let dir = tempdir()?;
        let file_path = dir.path().join("hardcap.rtsa");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&file_path)?;
        file.write_all(b"DSFH")?;
        file.write_all(&[0u8; 4]).unwrap();
        file.flush()?;
        drop(file);

        let mut reader = MmapRtsaReader::new(&file_path)?;
        let base = Instant::now();
        let overflow = 64usize;
        // Insert MAX_CACHE_ENTRIES + overflow Header chunks with
        // strictly-increasing last_access timestamps so LRU order is
        // unambiguous.
        for i in 0..(MAX_CACHE_ENTRIES + overflow) {
            reader.chunk_cache.insert(
                (i as u64, 4),
                CachedChunk {
                    data: vec![0u8; 4],
                    // i=0 is the oldest, i=last is the newest.
                    last_access: base + Duration::from_nanos(i as u64),
                    access_count: 1,
                    chunk_type: ChunkType::Header,
                },
            );
        }
        assert_eq!(reader.chunk_cache.len(), MAX_CACHE_ENTRIES + overflow);

        // Tiered pass would normally keep all Header chunks; the
        // hard-cap fallback should drop the oldest `overflow`.
        reader.evict_cache_entries();
        assert_eq!(
            reader.chunk_cache.len(),
            MAX_CACHE_ENTRIES,
            "hard cap must be enforced even when the tiered pass keeps everything"
        );
        // Oldest entries (i in 0..overflow) should be gone.
        for i in 0..overflow {
            assert!(
                !reader.chunk_cache.contains_key(&(i as u64, 4)),
                "expected the {} oldest LRU entries to be evicted",
                overflow
            );
        }
        // Newest entries should survive.
        for i in (overflow + MAX_CACHE_ENTRIES - 8)..(MAX_CACHE_ENTRIES + overflow) {
            assert!(
                reader.chunk_cache.contains_key(&(i as u64, 4)),
                "expected the newest LRU entries to survive"
            );
        }
        Ok(())
    }
}
