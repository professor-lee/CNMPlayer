use super::*;
use std::time::{Duration, Instant};

fn image() -> Arc<DynamicImage> {
    Arc::new(DynamicImage::ImageRgb8(image::RgbImage::from_fn(
        32,
        32,
        |x, y| image::Rgb([(x * 7) as u8, (y * 7) as u8, ((x + y) * 3) as u8]),
    )))
}

fn wait(covers: &mut CoverPipeline, key: CoverKey) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !matches!(covers.status(key), CoverStatus::Ready | CoverStatus::Failed) {
        covers.poll();
        assert!(Instant::now() < deadline, "cover preparation timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn final_cells_reuse_content_and_resize_discards_old_geometry() {
    let mut covers = CoverPipeline::new(WakeSignal::default());
    let key = CoverKey {
        hash: 1,
        width: 6,
        height: 3,
    };
    covers.begin_frame();
    covers.request_image(key, &image(), ImagePhase::Stable);
    covers.end_frame();
    wait(&mut covers, key);
    assert_eq!(covers.status(key), CoverStatus::Ready);
    assert!(
        covers.pending.is_empty(),
        "preview must end with the loading request"
    );
    let frame = covers.frame(key).unwrap().clone();
    covers.begin_frame();
    assert_eq!(
        covers.request_bytes(key, b"invalid", ImagePhase::Moving),
        CoverStatus::Ready
    );
    covers.end_frame();
    assert_eq!(covers.frame(key), Some(&frame));
    assert!(covers.jobs.is_empty());
    let resized = CoverKey {
        width: 4,
        height: 2,
        ..key
    };
    covers.begin_frame();
    covers.request_image(resized, &image(), ImagePhase::Stable);
    covers.end_frame();
    assert!(covers.frame(key).is_none());
    wait(&mut covers, resized);
    assert_eq!(covers.frames.len(), 1);
}

#[test]
fn failure_ends_loading_without_retries_and_superseded_preview_stays_absent() {
    let mut covers = CoverPipeline::new(WakeSignal::default());
    let old = CoverKey {
        hash: 1,
        width: 4,
        height: 2,
    };
    covers.begin_frame();
    covers.request_bytes(old, b"invalid", ImagePhase::Stable);
    covers.end_frame();
    wait(&mut covers, old);
    for _ in 0..4 {
        assert_eq!(
            covers.request_bytes(old, b"invalid", ImagePhase::Stable),
            CoverStatus::Failed
        );
    }
    assert!(covers.jobs.is_empty());
    let new = CoverKey { hash: 2, ..old };
    covers.begin_frame();
    covers.request_image(new, &image(), ImagePhase::Stable);
    covers.end_frame();
    covers.begin_frame();
    covers.end_frame();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !covers.jobs.is_empty() {
        covers.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(covers.frame(new).is_none());
    assert!(covers.pending.is_empty());
}

#[test]
fn preview_is_colored_halfblocks_and_partial_paint_preserves_source_rows() {
    let key = CoverKey {
        hash: 1,
        width: 6,
        height: 3,
    };
    let preview = prepare_preview(&image(), key.width, key.height);
    assert!(preview.content.iter().all(|cell| cell.symbol() == "▀"));
    let mut covers = CoverPipeline::new(WakeSignal::default());
    covers.pending.insert(
        key,
        Pending {
            generation: 1,
            preview: Some(preview.clone()),
        },
    );
    let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 5));
    covers.paint(
        &mut buffer,
        Rect::new(2, 1, 6, 2),
        Rect::new(2, 1, 6, 2),
        -1,
        1,
        key,
    );
    for y in 1..3 {
        for x in 2..7 {
            assert_eq!(buffer[(x, y)], preview[(x - 1, y)]);
        }
    }
    assert_eq!(buffer[(7, 1)].symbol(), " ");
}
