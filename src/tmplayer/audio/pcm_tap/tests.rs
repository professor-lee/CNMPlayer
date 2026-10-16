use super::*;
use std::sync::Arc;

fn ramp(from: usize, count: usize) -> Vec<f32> {
    (from..from + count).map(|i| i as f32).collect()
}

#[test]
fn independent_readers_do_not_consume_samples_or_repeat_reads() {
    let ring = PcmRing::new();
    let mut first = PcmReader::default();
    let mut second = PcmReader::default();
    let mut batch = PcmBatch::default();
    let left_ptr = batch.left.as_ptr();
    let right_ptr = batch.right.as_ptr();
    let left_capacity = batch.left.capacity();
    let right_capacity = batch.right.capacity();
    ring.push(&[1.0, 2.0], &[11.0, 12.0], 48_000);
    ring.read_since(&mut first, &mut batch);
    assert!(batch.reset);
    assert!(!batch.discontinuity);
    assert_eq!(batch.len, 2);
    assert_eq!(&batch.left[..batch.len], &[1.0, 2.0]);
    assert_eq!(&batch.right[..batch.len], &[11.0, 12.0]);
    ring.push(&[3.0], &[13.0], 48_000);
    ring.read_since(&mut first, &mut batch);
    assert!(!batch.reset);
    assert_eq!(&batch.left[..batch.len], &[3.0]);
    ring.read_since(&mut second, &mut batch);
    assert_eq!(&batch.left[..batch.len], &[1.0, 2.0, 3.0]);
    assert_eq!(&batch.right[..batch.len], &[11.0, 12.0, 13.0]);
    for reader in [&mut first, &mut second] {
        for _ in 0..3 {
            ring.read_since(reader, &mut batch);
            assert_eq!(batch.len, 0);
            assert!(!batch.reset);
            assert!(!batch.discontinuity);
        }
    }
    assert_eq!(batch.left.as_ptr(), left_ptr);
    assert_eq!(batch.right.as_ptr(), right_ptr);
    assert_eq!(batch.left.capacity(), left_capacity);
    assert_eq!(batch.right.capacity(), right_capacity);
    let mut snapshot = PcmSnapshot::default();
    ring.snapshot(&mut snapshot);
    assert_eq!(&snapshot.left[..snapshot.len], &[1.0, 2.0, 3.0]);
}

#[test]
fn whole_ring_wrap_is_exactly_one_new_batch_not_zero_samples() {
    let ring = PcmRing::new();
    let mut reader = PcmReader::default();
    let mut batch = PcmBatch::default();
    ring.push(&[-1.0], &[-2.0], 48_000);
    ring.read_since(&mut reader, &mut batch);
    let data = ramp(0, CAPACITY);
    for chunk in data.chunks(FLUSH_FRAMES) {
        ring.push(chunk, chunk, 48_000);
    }
    ring.read_since(&mut reader, &mut batch);
    assert_eq!(batch.len, CAPACITY);
    assert_eq!(&batch.left[..batch.len], data.as_slice());
    assert_eq!(&batch.right[..batch.len], data.as_slice());
    assert!(!batch.reset);
    assert!(!batch.discontinuity);
    ring.read_since(&mut reader, &mut batch);
    assert_eq!(batch.len, 0);
}

#[test]
fn unread_overrun_reports_a_gap_and_retains_the_available_tail() {
    let ring = PcmRing::new();
    let mut reader = PcmReader::default();
    let mut batch = PcmBatch::default();
    ring.read_since(&mut reader, &mut batch);
    let data = ramp(0, CAPACITY + 777);
    for chunk in data.chunks(FLUSH_FRAMES) {
        ring.push(chunk, chunk, 48_000);
    }
    ring.read_since(&mut reader, &mut batch);
    assert_eq!(batch.len, CAPACITY);
    assert_eq!(&batch.left[..batch.len], &data[777..]);
    assert!(batch.discontinuity);
    assert!(!batch.reset);
    ring.read_since(&mut reader, &mut batch);
    assert_eq!(batch.len, 0);
    assert!(!batch.discontinuity);
    let mut snapshot = PcmSnapshot::default();
    ring.snapshot(&mut snapshot);
    assert_eq!(&snapshot.left[..snapshot.len], &data[777..]);
}

#[test]
fn oversized_single_push_counts_frames_that_were_overwritten() {
    let ring = PcmRing::new();
    let mut reader = PcmReader::default();
    let mut batch = PcmBatch::default();
    ring.read_since(&mut reader, &mut batch);
    let data = ramp(0, CAPACITY + 7);
    ring.push(&data, &data, 48_000);
    ring.read_since(&mut reader, &mut batch);
    assert!(batch.discontinuity);
    assert_eq!(&batch.left[..batch.len], &data[7..]);
    ring.push(&[90_000.0], &[91_000.0], 48_000);
    ring.read_since(&mut reader, &mut batch);
    assert!(!batch.discontinuity);
    assert_eq!(&batch.left[..batch.len], &[90_000.0]);
    assert_eq!(&batch.right[..batch.len], &[91_000.0]);
}

#[test]
fn reset_and_rate_changes_start_distinct_sample_generations() {
    let ring = PcmRing::new();
    let mut first = PcmReader::default();
    let mut second = PcmReader::default();
    let mut batch = PcmBatch::default();
    ring.push(&[1.0, 2.0], &[1.0, 2.0], 48_000);
    ring.read_since(&mut first, &mut batch);
    ring.read_since(&mut second, &mut batch);
    ring.reset();
    ring.read_since(&mut first, &mut batch);
    assert!(batch.reset);
    assert!(!batch.discontinuity);
    assert_eq!(batch.len, 0);
    assert_eq!(batch.sample_rate, 0);
    ring.read_since(&mut first, &mut batch);
    assert!(!batch.reset);
    ring.push(&[3.0], &[4.0], 96_000);
    ring.read_since(&mut first, &mut batch);
    assert!(!batch.reset);
    assert_eq!(batch.sample_rate, 96_000);
    assert_eq!(&batch.left[..batch.len], &[3.0]);
    ring.read_since(&mut second, &mut batch);
    assert!(batch.reset);
    assert_eq!(batch.sample_rate, 96_000);
    assert_eq!(&batch.right[..batch.len], &[4.0]);
    // 即使调用方未显式 reset，采样率变更也不能混入旧流。
    ring.push(&[5.0], &[6.0], 44_100);
    ring.read_since(&mut first, &mut batch);
    assert!(batch.reset);
    assert!(!batch.discontinuity);
    assert_eq!(batch.sample_rate, 44_100);
    assert_eq!(&batch.left[..batch.len], &[5.0]);
    let mut snapshot = PcmSnapshot::default();
    ring.snapshot(&mut snapshot);
    assert_eq!(snapshot.sample_rate, 44_100);
    assert_eq!(&snapshot.right[..snapshot.len], &[6.0]);
}

#[test]
fn failed_try_lock_is_visible_even_without_a_successful_following_write() {
    let ring = PcmRing::new();
    let mut reader = PcmReader::default();
    let mut batch = PcmBatch::default();
    ring.push(&[1.0], &[2.0], 48_000);
    ring.read_since(&mut reader, &mut batch);
    {
        // 确定性占锁，不依赖调度或 sleep，也验证 push 不阻塞。
        let _guard = ring.inner.lock();
        ring.push(&[3.0], &[4.0], 96_000);
    }
    ring.read_since(&mut reader, &mut batch);
    assert_eq!(batch.len, 0);
    assert_eq!(batch.sample_rate, 48_000);
    assert!(batch.discontinuity);
    assert!(!batch.reset);
    ring.read_since(&mut reader, &mut batch);
    assert!(!batch.discontinuity);
    let mut snapshot = PcmSnapshot::default();
    ring.snapshot(&mut snapshot);
    assert_eq!(snapshot.sample_rate, 48_000);
    assert_eq!(&snapshot.left[..snapshot.len], &[1.0]);
    ring.push(&[5.0], &[6.0], 48_000);
    {
        let _guard = ring.inner.lock();
        ring.push(&[7.0], &[8.0], 48_000);
    }
    ring.push(&[9.0], &[10.0], 48_000);
    ring.read_since(&mut reader, &mut batch);
    assert!(batch.discontinuity);
    assert!(!batch.reset);
    assert_eq!(&batch.left[..batch.len], &[9.0]);
    assert_eq!(&batch.right[..batch.len], &[10.0]);
    ring.read_since(&mut reader, &mut batch);
    assert_eq!(batch.len, 0);
    assert!(!batch.discontinuity);
    ring.snapshot(&mut snapshot);
    assert_eq!(&snapshot.left[..snapshot.len], &[1.0, 5.0, 9.0]);
    ring.push(&[11.0], &[12.0], 48_000);
    ring.read_since(&mut reader, &mut batch);
    assert!(!batch.discontinuity);
    assert_eq!(&batch.left[..batch.len], &[11.0]);
    assert_eq!(&batch.right[..batch.len], &[12.0]);
}

#[test]
fn drop_discards_pending_incremental_history_for_each_reader_only() {
    let ring = PcmRing::new();
    let mut first = PcmReader::default();
    let mut second = PcmReader::default();
    let mut batch = PcmBatch::default();
    ring.read_since(&mut first, &mut batch);
    ring.read_since(&mut second, &mut batch);
    ring.push(&[1.0], &[2.0], 48_000);
    {
        let _guard = ring.inner.lock();
        ring.push(&[3.0], &[4.0], 48_000);
    }
    ring.read_since(&mut first, &mut batch);
    assert_eq!(batch.len, 0);
    assert!(batch.discontinuity);
    assert!(!batch.reset);
    ring.read_since(&mut first, &mut batch);
    assert_eq!(batch.len, 0);
    assert!(!batch.discontinuity);
    ring.push(&[5.0, 6.0], &[7.0, 8.0], 48_000);
    ring.read_since(&mut first, &mut batch);
    assert!(!batch.discontinuity);
    assert_eq!(&batch.left[..batch.len], &[5.0, 6.0]);
    ring.read_since(&mut second, &mut batch);
    assert!(batch.discontinuity);
    assert!(!batch.reset);
    assert_eq!(&batch.left[..batch.len], &[5.0, 6.0]);
    assert_eq!(&batch.right[..batch.len], &[7.0, 8.0]);
    ring.read_since(&mut second, &mut batch);
    assert_eq!(batch.len, 0);
    assert!(!batch.discontinuity);
    let mut snapshot = PcmSnapshot::default();
    ring.snapshot(&mut snapshot);
    assert_eq!(&snapshot.left[..snapshot.len], &[1.0, 5.0, 6.0]);
    assert_eq!(&snapshot.right[..snapshot.len], &[2.0, 7.0, 8.0]);
}

#[test]
fn bounded_snapshot_uses_current_rate_and_does_not_change_full_snapshot() {
    let ring = PcmRing::new();
    let data = ramp(0, CAPACITY + 31);
    ring.push(&data, &data, 48_000);
    let mut snapshot = PcmSnapshot::default();
    let left_ptr = snapshot.left.as_ptr();
    let right_ptr = snapshot.right.as_ptr();
    ring.snapshot_tail_ms(&mut snapshot, 65);
    assert_eq!(snapshot.len, 3120);
    assert_eq!(&snapshot.left[..snapshot.len], &data[data.len() - 3120..]);
    ring.snapshot(&mut snapshot);
    assert_eq!(snapshot.len, CAPACITY);
    assert_eq!(&snapshot.left[..snapshot.len], &data[31..]);
    ring.reset();
    ring.snapshot_tail_ms(&mut snapshot, 65);
    assert_eq!(snapshot.len, 0);
    assert_eq!(snapshot.sample_rate, 0);
    let data = ramp(100, 7000);
    ring.push(&data, &data, 96_000);
    ring.snapshot_tail_ms(&mut snapshot, 65);
    assert_eq!(snapshot.len, 6240);
    assert_eq!(snapshot.sample_rate, 96_000);
    assert_eq!(&snapshot.left[..snapshot.len], &data[760..]);
    assert_eq!(snapshot.left.as_ptr(), left_ptr);
    assert_eq!(snapshot.right.as_ptr(), right_ptr);
}

#[test]
fn snapshot_returns_newest_samples_in_order_across_wrap() {
    let ring = PcmRing::new();
    let total = CAPACITY + 777;
    let data = ramp(0, total);
    for chunk in data.chunks(FLUSH_FRAMES) {
        ring.push(chunk, chunk, 48_000);
    }

    let mut snap = PcmSnapshot::default();
    ring.snapshot(&mut snap);

    assert_eq!(snap.len, CAPACITY);
    assert_eq!(snap.sample_rate, 48_000);
    assert_eq!(snap.left[0], (total - CAPACITY) as f32);
    assert_eq!(snap.left[CAPACITY - 1], (total - 1) as f32);
    assert!(snap.left[..snap.len].windows(2).all(|w| w[1] - w[0] == 1.0));
}

#[test]
fn reset_empties_the_ring_without_a_further_push() {
    let ring = PcmRing::new();
    ring.push(&[0.5; 64], &[0.5; 64], 48_000);
    ring.reset();

    let mut snap = PcmSnapshot::default();
    ring.snapshot(&mut snap);
    assert_eq!(snap.len, 0);
}

#[test]
fn identical_channels_are_reported_as_mono() {
    let ring = PcmRing::new();
    let left = ramp(0, 128);
    ring.push(&left, &left, 48_000);

    let mut snap = PcmSnapshot::default();
    ring.snapshot(&mut snap);
    assert!(!snap.stereo);

    let mut right = left.clone();
    right[7] = -1.0;
    ring.push(&left, &right, 48_000);
    ring.snapshot(&mut snap);
    assert!(snap.stereo);
}

#[test]
fn tap_splits_stereo_and_mirrors_mono() {
    let stereo = Arc::new(PcmRing::new());
    let mut tap = PcmTap::new(Arc::clone(&stereo), 2, 48_000, None);
    for (channel, sample) in [(0, 1.0), (1, 2.0), (0, 3.0), (1, 4.0)] {
        tap.push_sample(channel, sample);
    }
    tap.flush();

    let mut snap = PcmSnapshot::default();
    stereo.snapshot(&mut snap);
    assert_eq!(snap.len, 2);
    assert_eq!(&snap.left[..2], &[1.0, 3.0]);
    assert_eq!(&snap.right[..2], &[2.0, 4.0]);

    let mono = Arc::new(PcmRing::new());
    let mut tap = PcmTap::new(Arc::clone(&mono), 1, 48_000, None);
    tap.push_sample(0, 0.25);
    tap.flush();

    mono.snapshot(&mut snap);
    assert_eq!(snap.len, 1);
    assert_eq!(snap.left[0], snap.right[0]);
    assert!(!snap.stereo);
}

#[test]
fn tap_flushes_on_its_own_once_staging_fills() {
    let ring = Arc::new(PcmRing::new());
    let mut tap = PcmTap::new(Arc::clone(&ring), 1, 48_000, None);
    for i in 0..FLUSH_FRAMES {
        tap.push_sample(0, i as f32);
    }

    // 没有手动 flush：攒满一批就该自动落环。
    let mut snap = PcmSnapshot::default();
    ring.snapshot(&mut snap);
    assert_eq!(snap.len, FLUSH_FRAMES);
}
#[test]
fn successful_push_publishes_fresh_activity_timestamp() {
    let ring = PcmRing::new();
    let before = Instant::now();
    ring.push(&[1.0, 2.0], &[3.0, 4.0], 48_000);
    let after = Instant::now();

    let activity = ring.activity();
    assert_eq!(activity.generation, 0);
    assert_eq!(activity.frames, 2);
    let updated_at = activity
        .updated_at
        .expect("successful push has a timestamp");
    assert!(updated_at >= before);
    assert!(updated_at <= after);
}

#[test]
fn empty_push_does_not_update_activity_timestamp() {
    let ring = PcmRing::new();
    ring.push(&[1.0], &[2.0], 48_000);
    let before = ring.activity();

    ring.push(&[], &[3.0], 48_000);

    let after = ring.activity();
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.frames, before.frames);
    assert_eq!(after.updated_at, before.updated_at);
}

#[test]
fn failed_try_lock_does_not_update_activity_timestamp() {
    let ring = PcmRing::new();
    ring.push(&[1.0], &[2.0], 48_000);
    let before = ring.activity();
    {
        let _guard = ring.inner.lock();
        ring.push(&[3.0], &[4.0], 48_000);
    }

    let after = ring.activity();
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.frames, before.frames);
    assert_eq!(after.updated_at, before.updated_at);
}

#[test]
fn reset_and_rate_change_clear_activity_generation_metadata() {
    let ring = PcmRing::new();
    ring.push(&[1.0, 2.0], &[3.0, 4.0], 48_000);
    let first = ring.activity();

    ring.reset();
    let reset = ring.activity();
    assert_ne!(reset.generation, first.generation);
    assert_eq!(reset.frames, 0);
    assert_eq!(reset.updated_at, None);

    // Start the reset generation before changing its established rate.
    ring.push(&[9.0], &[10.0], 48_000);
    let resumed = ring.activity();

    let before_rate_change = Instant::now();
    ring.push(&[5.0], &[6.0], 96_000);
    let after_rate_change = Instant::now();
    let second = ring.activity();
    assert_eq!(second.frames, 1);
    assert_ne!(second.generation, resumed.generation);
    let updated_at = second
        .updated_at
        .expect("successful rate change push has a timestamp");
    assert!(updated_at >= before_rate_change);
    assert!(updated_at <= after_rate_change);

    ring.push(&[7.0], &[8.0], 44_100);
    let changed_rate = ring.activity();
    assert_ne!(changed_rate.generation, second.generation);
    assert_eq!(changed_rate.frames, 1);
    assert!(changed_rate.updated_at.is_some());
}

#[test]
fn activity_does_not_consume_or_modify_samples() {
    let ring = PcmRing::new();
    ring.push(&[1.0, 2.0], &[3.0, 4.0], 48_000);
    let mut before = PcmSnapshot::default();
    let mut after = PcmSnapshot::default();
    ring.snapshot(&mut before);

    let activity = ring.activity();

    ring.snapshot(&mut after);
    assert_eq!(activity.frames, before.len);
    assert_eq!(after.len, before.len);
    assert_eq!(after.sample_rate, before.sample_rate);
    assert_eq!(&after.left[..after.len], &before.left[..before.len]);
    assert_eq!(&after.right[..after.len], &before.right[..before.len]);
}
