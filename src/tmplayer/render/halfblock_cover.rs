use crate::tmplayer::render::cover_cache::CoverKey;
use image::imageops::FilterType;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::StatefulWidget;
use ratatui_image::{Resize, StatefulImage, picker::Picker};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, SyncSender};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverStatus {
    Loading,
    Ready,
    Hidden,
}

#[derive(Debug)]
struct Request {
    key: CoverKey,
    bytes: Vec<u8>,
}
#[derive(Debug)]
struct ResultFrame {
    key: CoverKey,
    frame: Option<Buffer>,
}

/// Bounded background preparation using ratatui-image's chafa-backed Halfblocks protocol.
/// No terminal capability query or escape-sequence image protocol is used.
#[derive(Debug)]
pub struct HalfblockCovers {
    tx: SyncSender<Request>,
    rx: Receiver<ResultFrame>,
    pending: HashMap<CoverKey, u8>,
    frames: HashMap<CoverKey, Buffer>,
    order: VecDeque<CoverKey>,
    failures: HashMap<CoverKey, u8>,
}

impl HalfblockCovers {
    pub fn new() -> Self {
        let (tx, requests) = mpsc::sync_channel::<Request>(2);
        let (results, rx) = mpsc::sync_channel::<ResultFrame>(2);
        std::thread::spawn(move || {
            while let Ok(request) = requests.recv() {
                let frame = prepare(&request.bytes, request.key.width, request.key.height);
                if results
                    .send(ResultFrame {
                        key: request.key,
                        frame,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            tx,
            rx,
            pending: HashMap::new(),
            frames: HashMap::new(),
            order: VecDeque::new(),
            failures: HashMap::new(),
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.rx.try_recv() {
            let attempts = self.pending.remove(&result.key).unwrap_or(0);
            match result.frame {
                Some(frame) => {
                    self.failures.remove(&result.key);
                    self.frames.insert(result.key, frame);
                }
                None => {
                    let failures = attempts.saturating_add(1);
                    self.frames.remove(&result.key);
                    self.failures.insert(result.key, failures);
                }
            }
            self.order.retain(|key| *key != result.key);
            self.order.push_back(result.key);
            while self.order.len() > 8 {
                if let Some(old) = self.order.pop_front() {
                    self.frames.remove(&old);
                    self.failures.remove(&old);
                }
            }
            changed = true;
        }
        changed
    }

    /// Queue missing or retryable preparation without doing image work on the UI thread.
    /// Only completed worker failures count toward hiding a cover.
    pub fn status(&mut self, area: Rect, hash: u64, bytes: &[u8]) -> CoverStatus {
        if area.is_empty() {
            return CoverStatus::Hidden;
        }
        let key = CoverKey {
            hash,
            width: area.width,
            height: area.height,
        };
        if self.frames.contains_key(&key) {
            return CoverStatus::Ready;
        }
        let failures = self.failures.get(&key).copied().unwrap_or(0);
        if failures >= 10 {
            return CoverStatus::Hidden;
        }
        if self.pending.len() < 2 && !self.pending.contains_key(&key) {
            let request = Request {
                key,
                bytes: bytes.to_vec(),
            };
            if self.tx.try_send(request).is_ok() {
                self.pending.insert(key, failures);
            }
        }
        CoverStatus::Loading
    }

    pub fn paint(&mut self, target: &mut Buffer, area: Rect, hash: u64, bytes: &[u8]) {
        self.paint_segment(target, area, area, 0, hash, bytes);
    }

    /// Translate a full cached content frame, clipping only its destination.
    /// Preparing a cropped segment would change both the image viewport and cache key.
    pub fn paint_segment(
        &mut self,
        target: &mut Buffer,
        area: Rect,
        clip: Rect,
        dx: i16,
        hash: u64,
        bytes: &[u8],
    ) {
        if self.status(area, hash, bytes) != CoverStatus::Ready {
            return;
        }
        let key = CoverKey {
            hash,
            width: area.width,
            height: area.height,
        };
        if let Some(frame) = self.frames.get(&key) {
            let clip = clip.intersection(target.area);
            for y in 0..area.height {
                let dest_y = u32::from(area.y) + u32::from(y);
                if dest_y < u32::from(clip.y) || dest_y >= u32::from(clip.bottom()) {
                    continue;
                }
                for x in 0..area.width {
                    let dest_x = i32::from(area.x) + i32::from(x) + i32::from(dx);
                    if dest_x < i32::from(clip.x) || dest_x >= i32::from(clip.right()) {
                        continue;
                    }
                    if let Some(cell) = target.cell_mut((dest_x as u16, dest_y as u16)) {
                        *cell = frame[(x, y)].clone();
                    }
                }
            }
        }
    }
}

fn prepare(bytes: &[u8], width: u16, height: u16) -> Option<Buffer> {
    if width == 0 || height == 0 {
        return None;
    }
    let image = image::load_from_memory(bytes).ok()?;
    let (x, y, w, h) = crate::render::graphics_overlay::cover_viewport(
        image.width(),
        image.height(),
        width,
        height,
    );
    // Match the previous Halfblocks overlay: crop to fill, resize in font pixels,
    // then render StatefulImage with Crop. Feeding chafa a 1x2 sample grid instead
    // loses the richer glyph palette selected by the linked ratatui-image backend.
    let picker = Picker::halfblocks();
    let font = picker.font_size();
    let pixels = image.crop_imm(x, y, w, h).resize_exact(
        u32::from(width) * u32::from(font.width),
        u32::from(height) * u32::from(font.height),
        FilterType::Triangle,
    );
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let mut protocol = picker.new_resize_protocol(pixels);
    StatefulImage::default()
        .resize(Resize::Crop(None))
        .render(area, &mut buffer, &mut protocol);
    // Propagate resize/encoding errors returned by ratatui-image. The linked
    // chafa backend currently reports success unconditionally; failures inside
    // its C API that are not reported cannot be distinguished here.
    protocol.last_encoding_result()?.ok()?;
    Some(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::{Duration, Instant};

    fn image_bytes() -> Vec<u8> {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(5, 3, |x, y| {
            image::Rgb([(x * 40) as u8, (y * 70) as u8, ((x + y) * 30) as u8])
        }));
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        encoded.into_inner()
    }

    fn wait_for_result(covers: &mut HalfblockCovers) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !covers.poll() {
            assert!(Instant::now() < deadline, "cover preparation timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn failed_preparation_retries_once_pending_and_recovers_to_cached_ready() {
        let mut covers = HalfblockCovers::new();
        let area = Rect::new(2, 1, 4, 2);
        let key = CoverKey {
            hash: 1,
            width: area.width,
            height: area.height,
        };
        let mut target = Buffer::empty(Rect::new(0, 0, 8, 4));
        let untouched = target.clone();
        for _ in 0..32 {
            assert_eq!(covers.status(area, 1, b"invalid"), CoverStatus::Loading);
            covers.paint(&mut target, area, 1, b"invalid");
        }
        assert_eq!(target, untouched);
        assert_eq!(covers.pending.len(), 1);
        assert_eq!(covers.pending[&key], 0);
        assert!(covers.frames.is_empty());
        wait_for_result(&mut covers);
        assert_eq!(covers.failures[&key], 1);

        let bytes = image_bytes();
        assert_eq!(covers.status(area, 1, &bytes), CoverStatus::Loading);
        wait_for_result(&mut covers);
        assert_eq!(covers.status(area, 1, &bytes), CoverStatus::Ready);
        assert_eq!(covers.order.len(), 1);
        let expected = prepare(&bytes, area.width, area.height).unwrap();
        for _ in 0..32 {
            // Invalid bytes would fail if a ready key were prepared again.
            assert_eq!(covers.status(area, 1, b"invalid"), CoverStatus::Ready);
            covers.paint(&mut target, area, 1, b"invalid");
            assert!(covers.pending.is_empty());
            assert!(!covers.poll());
        }
        for y in 0..area.height {
            for x in 0..area.width {
                assert_eq!(target[(area.x + x, area.y + y)], expected[(x, y)]);
            }
        }
    }

    #[test]
    fn tenth_completed_failure_hides_only_that_key_and_stops_preparing() {
        let mut covers = HalfblockCovers::new();
        let area = Rect::new(0, 0, 3, 2);
        let key = CoverKey {
            hash: 1,
            width: area.width,
            height: area.height,
        };
        for attempt in 1..=10 {
            for _ in 0..16 {
                assert_eq!(covers.status(area, 1, b"invalid"), CoverStatus::Loading);
            }
            assert_eq!(covers.pending[&key], attempt - 1);
            wait_for_result(&mut covers);
            assert_eq!(covers.failures[&key], attempt);
            assert!(!covers.frames.contains_key(&key));
            assert_eq!(covers.order.len(), 1);
        }
        let mut target = Buffer::empty(area);
        let untouched = target.clone();
        for _ in 0..32 {
            assert_eq!(covers.status(area, 1, b"invalid"), CoverStatus::Hidden);
            covers.paint(&mut target, area, 1, b"invalid");
            assert!(covers.pending.is_empty());
            assert!(!covers.poll());
        }
        assert_eq!(target, untouched);
        let bytes = image_bytes();
        assert_eq!(covers.status(area, 2, &bytes), CoverStatus::Loading);
        let resized = Rect::new(0, 0, 4, 2);
        assert_eq!(covers.status(resized, 1, &bytes), CoverStatus::Loading);
        assert_eq!(covers.pending.len(), 2);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !covers.pending.is_empty() {
            covers.poll();
            assert!(Instant::now() < deadline, "changed cover keys timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(covers.status(area, 2, &bytes), CoverStatus::Ready);
        assert_eq!(covers.status(resized, 1, &bytes), CoverStatus::Ready);
        assert_eq!(covers.status(area, 1, &bytes), CoverStatus::Hidden);
        assert_eq!(
            covers.status(Rect::default(), 1, &bytes),
            CoverStatus::Hidden
        );
    }

    #[test]
    fn cached_segment_translates_and_clips_without_preparing_cropped_images() {
        let mut covers = HalfblockCovers::new();
        let bytes = image_bytes();
        let area = Rect::new(3, 2, 4, 2);
        assert_eq!(covers.status(area, 1, &bytes), CoverStatus::Loading);
        wait_for_result(&mut covers);
        let frame = prepare(&bytes, area.width, area.height).unwrap();
        let clip = Rect::new(2, 1, 5, 3);
        for dx in [-5, -2, 0, 2, 5] {
            let mut target = Buffer::empty(Rect::new(1, 1, 8, 4));
            let mut expected = target.clone();
            for y in 0..area.height {
                for x in 0..area.width {
                    let dest_x = i32::from(area.x + x) + i32::from(dx);
                    let dest_y = area.y + y;
                    if dest_x >= i32::from(clip.x)
                        && dest_x < i32::from(clip.right())
                        && dest_y >= clip.y
                        && dest_y < clip.bottom()
                    {
                        expected[(dest_x as u16, dest_y)] = frame[(x, y)].clone();
                    }
                }
            }
            covers.paint_segment(&mut target, area, clip, dx, 1, b"invalid");
            assert_eq!(target, expected);
            assert!(covers.pending.is_empty());
            assert_eq!(covers.frames.len(), 1);
        }
    }

    #[test]
    fn preparation_cache_and_pending_requests_stay_bounded() {
        let mut covers = HalfblockCovers::new();
        let area = Rect::new(0, 0, 2, 1);
        for hash in 0..12 {
            assert_eq!(covers.status(area, hash, b"invalid"), CoverStatus::Loading);
            wait_for_result(&mut covers);
            assert!(covers.frames.len() + covers.failures.len() <= 8);
            assert_eq!(
                covers.frames.len() + covers.failures.len(),
                covers.order.len()
            );
        }
        assert_eq!(covers.failures.len(), 8);
        for hash in 12..32 {
            assert_eq!(covers.status(area, hash, b"invalid"), CoverStatus::Loading);
            assert!(covers.pending.len() <= 2);
        }
        assert_eq!(covers.pending.len(), 2);
    }

    #[test]
    fn prepared_cells_match_baseline_stateful_halfblocks_charset_and_colors() {
        use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
        use ratatui::{Terminal, backend::TestBackend};
        use ratatui_image::picker::ProtocolType;

        for (image_width, image_height) in [(31, 17), (17, 31), (32, 32)] {
            let image =
                DynamicImage::ImageRgba8(RgbaImage::from_fn(image_width, image_height, |x, y| {
                    Rgba([
                        (x * 31 % 256) as u8,
                        (y * 47 % 256) as u8,
                        ((x + y) * 23 % 256) as u8,
                        if (x + y) % 5 == 0 { 0 } else { 255 },
                    ])
                }));
            let mut encoded = Cursor::new(Vec::new());
            image
                .write_to(&mut encoded, image::ImageFormat::Png)
                .unwrap();
            for (width, height) in [(1, 1), (12, 6), (9, 7)] {
                // Baseline 7961505 GraphicsOverlay Halfblocks path, with its
                // query-free fallback Picker. Keep this independent of prepare().
                let mut picker = Picker::halfblocks();
                picker.set_protocol_type(ProtocolType::Halfblocks);
                let font = picker.font_size();
                let (iw, ih) = image.dimensions();
                let ratio = f64::from(width) * f64::from(font.width)
                    / (f64::from(height) * f64::from(font.height));
                let (x, y, w, h) = if f64::from(iw) / f64::from(ih) > ratio {
                    let w = (f64::from(ih) * ratio).round().clamp(1.0, f64::from(iw)) as u32;
                    ((iw - w) / 2, 0, w, ih)
                } else {
                    let h = (f64::from(iw) / ratio).round().clamp(1.0, f64::from(ih)) as u32;
                    (0, (ih - h) / 2, iw, h)
                };
                let cropped = image.crop_imm(x, y, w, h).resize_exact(
                    u32::from(width) * u32::from(font.width),
                    u32::from(height) * u32::from(font.height),
                    FilterType::Triangle,
                );
                let mut protocol = picker.new_resize_protocol(cropped);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| {
                        frame.render_stateful_widget(
                            StatefulImage::default().resize(Resize::Crop(None)),
                            frame.area(),
                            &mut protocol,
                        );
                    })
                    .unwrap();
                let actual = prepare(encoded.get_ref(), width, height).unwrap();
                let expected = terminal.backend().buffer();
                for y in 0..height {
                    for x in 0..width {
                        assert_eq!(actual[(x, y)].symbol(), expected[(x, y)].symbol());
                        assert_eq!(actual[(x, y)].fg, expected[(x, y)].fg);
                        assert_eq!(actual[(x, y)].bg, expected[(x, y)].bg);
                    }
                }
            }
        }
    }
}
