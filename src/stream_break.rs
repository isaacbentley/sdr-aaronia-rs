//! Where the sample stream broke, reported to whoever consumes the samples.
//!
//! # Why a stream needs this
//!
//! A source block hands its consumer a flat run of samples. That run *claims*
//! to be continuous, and a digital demodulator believes it: symbol timing,
//! carrier tracking and burst framing are all carried across the boundaries
//! between one delivery and the next. Splice two packets across a run the
//! server never sent, or across a retune, and every one of those is wrong with
//! nothing to say so — this crate's own `link_budget` documentation puts it
//! plainly: every dropped sample is an unsignalled phase discontinuity that
//! breaks digital symbol lock. Measuring that a link *will* drop samples and
//! then silently splicing when it does is only half the job.
//!
//! [`HttpSource`] knows about four such events and, until this existed,
//! reported all four as counters only:
//!
//! ```text
//!   event                     what it invalidates
//!   ───────────────────────   ──────────────────────────────────────────
//!   server gap                time continuity  (DropDetector, from the
//!                             packet timestamps)
//!   capacity trim             time continuity  (this process discarded
//!                             the oldest buffered samples)
//!   reconnect / restart       time continuity  (a new HTTP stream; the
//!                             parser and the drop detector both reset)
//!   centre or rate change     what the samples *mean* — the frequency
//!                             mapping itself is stale
//! ```
//!
//! A counter says how many times something happened. It cannot say *which*
//! buffered samples belong to the old epoch, which is the only thing a
//! consumer can act on.
//!
//! # The contract
//!
//! A break is keyed on an **absolute sample index in this source's own output
//! stream**: the position of the first sample that belongs to the epoch
//! *after* the break. Index 0 is the first sample the block ever produces.
//! Samples the source discards — trimmed, or cleared on a reconnect — never
//! occupy an index, so the coordinate is exactly "how many samples has this
//! block produced", which a consumer counting what it has consumed can match
//! without being told anything else.
//!
//! Two ordering guarantees make that usable:
//!
//! 1. A break is recorded **before** the samples it precedes are published,
//!    so by the time a consumer can see sample `i`, every break at or before
//!    `i` is already in the sink.
//! 2. Breaks arrive in index order, and an index is never revisited.
//!
//! Both are the caller's job, not the sink's; [`HttpSource`] holds breaks
//! against the samples still queued in its own buffer and only emits them as
//! those samples are produced, precisely so a later trim cannot move an index
//! that has already been reported.
//!
//! # Why a trait rather than a type
//!
//! The consumer owns the queue. This crate has no opinion on whether that is a
//! shared `VecDeque`, a channel, a log line or a test's `Vec`, and the
//! arguments are primitives so nothing here has to know the consumer's types.
//! `bigear` implements it over its own `StreamBreaks` side queue; a caller
//! with no interest in continuity passes nothing and pays for nothing.
//!
//! [`HttpSource`]: crate::http_source::HttpSource

/// What happened to the stream, and therefore what it invalidates.
///
/// The variants are deliberately different severities, because they invalidate
/// different things. A gap loses samples but leaves every frequency meaning
/// what it meant, so a carrier is still the same carrier and only state that
/// assumes *time continuity* is wrong. A retune changes what the samples mean,
/// so the frequency mapping itself is stale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamDiscontinuity {
    /// Samples that should have been here are not: the server skipped, the
    /// capacity trim discarded the oldest buffered samples, or the stream was
    /// reconnected.
    ///
    /// How many samples were lost is deliberately not reported. For a server
    /// gap it is a duration rather than a count, and for a reconnect it is
    /// unknowable — which is itself the reason to reset rather than to
    /// compensate: a resync that does not know the size of the hole cannot
    /// close it.
    Gap,
    /// The geometry the **first** packet of the run actually arrived under.
    ///
    /// Not a retune, and deliberately a separate variant. A caller builds its
    /// processing from the geometry it *requested*, and the RTSA server does
    /// not have to honour it — it serves the nearest rung of its own rate
    /// ladder and streams the span its mission is configured for. Until this
    /// existed a consumer could only compare packets against each other, so a
    /// run that was wrong from sample 0 was never wrong at all.
    ///
    /// Emitted at sample 0 of every run whether or not it differs, so the
    /// comparison happens in one place: this carries what arrived, and the
    /// consumer knows what it asked for.
    Initial {
        /// Centre frequency of the samples that follow, in Hz.
        center_hz: f64,
        /// Sample rate of the samples that follow, in Hz.
        rate_hz: f64,
    },
    /// The device retuned or changed rate mid-stream: the samples after this
    /// point describe a different band, a different bandwidth, or both.
    Retune {
        /// Centre frequency of the samples that follow, in Hz.
        center_hz: f64,
        /// Sample rate of the samples that follow, in Hz.
        rate_hz: f64,
    },
}

impl StreamDiscontinuity {
    /// A short label for logs and counters.
    pub fn kind(&self) -> &'static str {
        match self {
            StreamDiscontinuity::Gap => "gap",
            StreamDiscontinuity::Initial { .. } => "initial",
            StreamDiscontinuity::Retune { .. } => "retune",
        }
    }
}

/// Where a source reports its stream breaks.
///
/// Implemented by the consumer and handed to the source before the flowgraph
/// takes ownership of it (see `HttpSourceBuilder::with_stream_breaks`).
///
/// `record` is called from the source block's `work()`, which is the thread
/// that must not stall, so an implementation has to be cheap and must not
/// block: a push onto a queue behind a `Mutex`, or an atomic increment. It is
/// called once per break — a handful of times over a run, not per sample.
pub trait StreamBreakSink: Send + Sync {
    /// Record that `cause` happened immediately before the sample at absolute
    /// index `at_sample` in the source's output stream.
    fn record(&self, at_sample: u64, cause: StreamDiscontinuity);
}

impl<T: StreamBreakSink + ?Sized> StreamBreakSink for std::sync::Arc<T> {
    fn record(&self, at_sample: u64, cause: StreamDiscontinuity) {
        (**self).record(at_sample, cause);
    }
}

/// A [`StreamBreakSink`] that keeps every break in a `Vec`, for tests and for
/// callers that only want to look at a finished run.
#[derive(Debug, Default)]
pub struct RecordingBreakSink {
    breaks: std::sync::Mutex<Vec<(u64, StreamDiscontinuity)>>,
}

impl RecordingBreakSink {
    /// An empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything recorded so far, in the order it was recorded.
    pub fn breaks(&self) -> Vec<(u64, StreamDiscontinuity)> {
        self.breaks.lock().map(|b| b.clone()).unwrap_or_default()
    }
}

impl StreamBreakSink for RecordingBreakSink {
    fn record(&self, at_sample: u64, cause: StreamDiscontinuity) {
        if let Ok(mut breaks) = self.breaks.lock() {
            breaks.push((at_sample, cause));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_recording_sink_keeps_order() {
        let sink = Arc::new(RecordingBreakSink::new());
        sink.record(
            0,
            StreamDiscontinuity::Initial {
                center_hz: 100e6,
                rate_hz: 1e6,
            },
        );
        sink.record(4096, StreamDiscontinuity::Gap);

        let breaks = sink.breaks();
        assert_eq!(breaks.len(), 2);
        assert_eq!(breaks[0].0, 0);
        assert_eq!(breaks[0].1.kind(), "initial");
        assert_eq!(breaks[1], (4096, StreamDiscontinuity::Gap));
    }

    #[test]
    fn an_arc_forwards_to_its_target() {
        // The source stores `Arc<dyn StreamBreakSink>`; a caller holding the
        // concrete sink must see what the source records through it.
        let sink = Arc::new(RecordingBreakSink::new());
        let erased: Arc<dyn StreamBreakSink> = sink.clone();
        erased.record(7, StreamDiscontinuity::Gap);
        assert_eq!(sink.breaks(), vec![(7, StreamDiscontinuity::Gap)]);
    }
}
