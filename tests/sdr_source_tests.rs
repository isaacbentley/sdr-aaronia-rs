use crossbeam_channel::unbounded;
use num_complex::Complex32;
use sdr_aaronia_rs::sdr_source::{DwellController, PooledIqBuffer, freq_key_khz};
use std::time::{Duration, Instant};

#[test]
fn test_freq_key_khz() {
    assert_eq!(freq_key_khz(100_000_000.0), 100_000);
    assert_eq!(freq_key_khz(100_000_499.0), 100_000);
    assert_eq!(freq_key_khz(100_000_500.0), 100_001);
}

#[test]
fn test_dwell_controller_non_adaptive() {
    let ctrl = DwellController {
        min: Duration::from_millis(100),
        max: Duration::from_millis(200),
        extension: Duration::ZERO,
    };

    assert!(!ctrl.is_adaptive());

    let now = Instant::now();
    // Deadline should be exactly min (since extension is ZERO)
    let deadline = ctrl.deadline(now, Some(now));
    assert_eq!(deadline, now + Duration::from_millis(100));

    let deadline_no_signal = ctrl.deadline(now, None);
    assert_eq!(deadline_no_signal, now + Duration::from_millis(100));
}

#[test]
fn test_dwell_controller_adaptive() {
    let ctrl = DwellController {
        min: Duration::from_millis(100),
        max: Duration::from_millis(200),
        extension: Duration::from_millis(50),
    };

    assert!(ctrl.is_adaptive());

    let now = Instant::now();

    // Case 1: no signal. Deadline = min
    let deadline = ctrl.deadline(now, None);
    assert_eq!(deadline, now + Duration::from_millis(100));

    // Case 2: signal occurred 20ms ago (inside min).
    // Extended deadline = now - 20ms + 50ms = now + 30ms.
    // min is now + 100ms, so deadline should be base (min).
    let deadline = ctrl.deadline(now, Some(now - Duration::from_millis(20)));
    assert_eq!(deadline, now + Duration::from_millis(100));

    // Case 3: signal occurred exactly at now.
    // Extended deadline = now + 50ms. Still less than min (100ms), so deadline is min.
    let deadline = ctrl.deadline(now, Some(now));
    assert_eq!(deadline, now + Duration::from_millis(100));

    // Case 4: signal occurred at now + 80ms.
    // Extended = now + 80ms + 50ms = now + 130ms.
    // Greater than min (100ms) but less than max (200ms). Deadline should be now + 130ms.
    let deadline = ctrl.deadline(now, Some(now + Duration::from_millis(80)));
    assert_eq!(deadline, now + Duration::from_millis(130));

    // Case 5: signal occurred at now + 180ms.
    // Extended = now + 180ms + 50ms = now + 230ms.
    // Greater than max (200ms). Deadline should be capped to max (200ms).
    let deadline = ctrl.deadline(now, Some(now + Duration::from_millis(180)));
    assert_eq!(deadline, now + Duration::from_millis(200));
}

#[test]
fn test_pooled_iq_buffer_drop_and_recycle() {
    let (tx, rx) = unbounded();

    // Seed the pool with a buffer
    tx.send(vec![Complex32::new(0.0, 0.0); 10]).unwrap();

    // Retrieve buffer from pool
    let buf = rx.recv().unwrap();
    assert_eq!(buf.len(), 10);

    // Wrap it in PooledIqBuffer
    {
        let pooled = PooledIqBuffer::new_pooled(buf, tx.clone());
        assert_eq!(pooled.len(), 10);
        // Verify deref works
        assert_eq!(pooled[0].re, 0.0);

        // Channel should be empty now
        assert!(rx.is_empty());
    } // pooled goes out of scope here; should recycle the buffer

    // Buffer should be back in the channel
    assert!(!rx.is_empty());
    let recycled = rx.recv().unwrap();
    assert_eq!(recycled.len(), 0); // clear() was called during drop
    assert_eq!(recycled.capacity(), 10); // capacity is preserved!
}
