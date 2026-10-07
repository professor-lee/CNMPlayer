//! Bounded, progressive cover preparation. Paint only reads already prepared cells.
use crate::render::graphics_overlay::cover_viewport;
use crate::render::wake::WakeSignal;
use image::{DynamicImage, imageops::FilterType};
use ratatui::{buffer::Buffer, layout::Rect, widgets::StatefulWidget};
use ratatui_image::{Resize, StatefulImage, picker::Picker};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, mpsc};

const MAX_JOBS: usize = 2;
const CACHE_ENTRIES: usize = 32;
const CACHE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CoverKey {
    pub hash: u64,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImagePhase {
    Stable,
    Moving,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoverStatus {
    Loading,
    Preview,
    Ready,
    Failed,
    Hidden,
}

#[derive(Debug)]
enum Source {
    Bytes(Vec<u8>),
    Image(Arc<DynamicImage>),
}

#[derive(Debug)]
struct Request {
    key: CoverKey,
    generation: u64,
    source: Source,
}

#[derive(Debug)]
enum Prepared {
    Preview(Buffer),
    Finished(Option<Buffer>),
}

#[derive(Debug)]
struct ResultFrame {
    key: CoverKey,
    generation: u64,
    result: Prepared,
}

#[derive(Debug)]
struct Pending {
    generation: u64,
    // Owned only by this active request; removed on completion/supersession.
    preview: Option<Buffer>,
}

#[derive(Debug)]
struct Worker {
    tx: mpsc::SyncSender<Request>,
    rx: mpsc::Receiver<ResultFrame>,
}

/// One worker per UI lifetime. Cached content identities share the current
/// geometries; old resize variants are removed after each demand preparation.
/// The LRU budget applies to inactive entries. Visible work cannot evict itself.
#[derive(Debug)]
pub(crate) struct CoverPipeline {
    wake: WakeSignal,
    worker: Option<Worker>,
    jobs: HashSet<(CoverKey, u64)>,
    pending: HashMap<CoverKey, Pending>,
    frames: HashMap<CoverKey, Buffer>,
    failures: HashSet<CoverKey>,
    order: VecDeque<CoverKey>,
    needed: HashSet<CoverKey>,
    generation: u64,
}

impl CoverPipeline {
    pub fn new(wake: WakeSignal) -> Self {
        Self {
            wake,
            worker: None,
            jobs: HashSet::new(),
            pending: HashMap::new(),
            frames: HashMap::new(),
            failures: HashSet::new(),
            order: VecDeque::new(),
            needed: HashSet::new(),
            generation: 0,
        }
    }

    pub fn wake(&self) -> WakeSignal {
        self.wake.clone()
    }

    pub fn begin_frame(&mut self) {
        self.needed.clear();
    }

    pub fn request_bytes(&mut self, key: CoverKey, bytes: &[u8], phase: ImagePhase) -> CoverStatus {
        self.request(key, phase, || Source::Bytes(bytes.to_vec()))
    }

    pub fn request_image(&mut self, key: CoverKey, image: &Arc<DynamicImage>, phase: ImagePhase) -> CoverStatus {
        self.request(key, phase, || Source::Image(image.clone()))
    }

    fn request(&mut self, key: CoverKey, _phase: ImagePhase, source: impl FnOnce() -> Source) -> CoverStatus {
        if key.width == 0 || key.height == 0 {
            return CoverStatus::Hidden;
        }
        self.needed.insert(key);
        if self.frames.contains_key(&key) {
            self.order.retain(|entry| *entry != key);
            self.order.push_back(key);
            return CoverStatus::Ready;
        }
        if self.failures.contains(&key) || self.pending.contains_key(&key) {
            return self.status(key);
        }
        if self.jobs.len() >= MAX_JOBS {
            return CoverStatus::Loading;
        }
        self.start_worker();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let request = Request { key, generation, source: source() };
        match self.worker.as_ref().unwrap().tx.try_send(request) {
            Ok(()) => {
                self.jobs.insert((key, generation));
                self.pending.insert(key, Pending { generation, preview: None });
            }
            Err(mpsc::TrySendError::Full(_)) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.failures.insert(key);
            }
        }
        self.status(key)
    }

    fn start_worker(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let (tx, requests) = mpsc::sync_channel::<Request>(MAX_JOBS);
        let (results, rx) = mpsc::sync_channel::<ResultFrame>(MAX_JOBS * 2);
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            while let Ok(request) = requests.recv() {
                let key = request.key;
                let generation = request.generation;
                let send = |result| {
                    if results.send(ResultFrame { key, generation, result }).is_err() {
                        return false;
                    }
                    wake.notify();
                    true
                };
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let image = match request.source {
                        Source::Bytes(bytes) => Arc::new(image::load_from_memory(&bytes).ok()?),
                        Source::Image(image) => image,
                    };
                    let preview = prepare_preview(&image, key.width, key.height);
                    if !send(Prepared::Preview(preview)) {
                        return None;
                    }
                    prepare_chafa(&image, key.width, key.height)
                }));
                let frame = match outcome {
                    Ok(frame) => frame,
                    Err(_) => {
                        log::warn!("cover preparation panicked for {:x}", key.hash);
                        None
                    }
                };
                if !send(Prepared::Finished(frame)) {
                    break;
                }
            }
        });
        self.worker = Some(Worker { tx, rx });
    }

    pub fn poll(&mut self) -> bool {
        let Some(worker) = &self.worker else {
            return false;
        };
        let mut changed = false;
        while let Ok(result) = worker.rx.try_recv() {
            let current = self.pending.get(&result.key)
                .is_some_and(|pending| pending.generation == result.generation);
            match result.result {
                Prepared::Preview(frame) if current => {
                    self.pending.get_mut(&result.key).unwrap().preview = Some(frame);
                    changed = true;
                }
                Prepared::Preview(_) => {}
                Prepared::Finished(frame) => {
                    self.jobs.remove(&(result.key, result.generation));
                    // Also wake a subsequent preparation when admission was full.
                    changed = true;
                    if current {
                        self.pending.remove(&result.key);
                        if let Some(frame) = frame {
                            self.frames.insert(result.key, frame);
                            self.order.retain(|key| *key != result.key);
                            self.order.push_back(result.key);
                        } else {
                            self.failures.insert(result.key);
                        }
                    }
                }
            }
        }
        changed
    }

    pub fn end_frame(&mut self) {
        let sizes: HashSet<_> = self.needed.iter().map(|key| (key.width, key.height)).collect();
        self.frames.retain(|key, _| sizes.contains(&(key.width, key.height)));
        self.failures.retain(|key| sizes.contains(&(key.width, key.height)));
        self.pending.retain(|key, _| self.needed.contains(key));
        self.order.retain(|key| self.frames.contains_key(key));
        let mut bytes: usize = self.frames.values().map(frame_bytes).sum();
        while self.frames.len() > CACHE_ENTRIES || bytes > CACHE_BYTES {
            let Some(index) = self.order.iter().position(|key| !self.needed.contains(key)) else {
                break;
            };
            let key = self.order.remove(index).unwrap();
            if let Some(frame) = self.frames.remove(&key) {
                bytes = bytes.saturating_sub(frame_bytes(&frame));
            }
        }
        // Failure records are small but should not grow through arbitrary covers.
        if self.failures.len() > CACHE_ENTRIES {
            self.failures.retain(|key| self.needed.contains(key));
        }
    }

    pub fn status(&self, key: CoverKey) -> CoverStatus {
        if key.width == 0 || key.height == 0 {
            CoverStatus::Hidden
        } else if self.frames.contains_key(&key) {
            CoverStatus::Ready
        } else if self.failures.contains(&key) {
            CoverStatus::Failed
        } else if self.pending.get(&key).is_some_and(|pending| pending.preview.is_some()) {
            CoverStatus::Preview
        } else {
            CoverStatus::Loading
        }
    }

    pub fn frame(&self, key: CoverKey) -> Option<&Buffer> {
        self.frames.get(&key).or_else(|| self.pending.get(&key)?.preview.as_ref())
    }

    /// Move/crop prepared cells without decoding, sampling, or encoding again.
    pub fn paint(&self, target: &mut Buffer, area: Rect, clip: Rect, dx: i16, source_row: u16, key: CoverKey) {
        let Some(frame) = self.frame(key) else { return; };
        let clip = clip.intersection(target.area).intersection(Rect::new(area.x, area.y, area.width, area.height));
        let origin_x = i32::from(area.x) + i32::from(dx);
        let left = i32::from(clip.left()).max(origin_x);
        let right = i32::from(clip.right()).min(origin_x + i32::from(key.width));
        let bottom = clip.bottom().min(area.y.saturating_add(key.height.saturating_sub(source_row)));
        for y in clip.top()..bottom {
            let sy = source_row + y - area.y;
            for x in left..right {
                target[(x as u16, y)].clone_from(&frame[((x - origin_x) as u16, sy)]);
            }
        }
    }
}

fn frame_bytes(frame: &Buffer) -> usize {
    frame.content.len() * std::mem::size_of::<ratatui::buffer::Cell>()
}

fn prepare_preview(image: &DynamicImage, width: u16, height: u16) -> Buffer {
    let (x, y, w, h) = cover_viewport(image.width(), image.height(), width, height);
    let pw = u32::from(width.min(16));
    let ph = u32::from(height.min(8)) * 2;
    let pixels = image.crop_imm(x, y, w, h).resize_exact(pw, ph, FilterType::Triangle).to_rgb8();
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
    for row in 0..height {
        let upper = u32::from(row) * 2 * ph / (u32::from(height) * 2);
        let lower = ((u32::from(row) * 2 + 1) * ph / (u32::from(height) * 2)).min(ph - 1);
        for col in 0..width {
            let px = u32::from(col) * pw / u32::from(width);
            let a = pixels.get_pixel(px, upper).0;
            let b = pixels.get_pixel(px, lower).0;
            buffer[(col, row)]
                .set_char('▀')
                .set_fg(ratatui::style::Color::Rgb(a[0], a[1], a[2]))
                .set_bg(ratatui::style::Color::Rgb(b[0], b[1], b[2]));
        }
    }
    buffer
}

fn prepare_chafa(image: &DynamicImage, width: u16, height: u16) -> Option<Buffer> {
    let (x, y, w, h) = cover_viewport(image.width(), image.height(), width, height);
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
    StatefulImage::default().resize(Resize::Crop(None)).render(area, &mut buffer, &mut protocol);
    protocol.last_encoding_result()?.ok()?;
    Some(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn image() -> Arc<DynamicImage> {
        Arc::new(DynamicImage::ImageRgb8(image::RgbImage::from_fn(32, 32, |x, y| {
            image::Rgb([(x * 7) as u8, (y * 7) as u8, ((x + y) * 3) as u8])
        })))
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
        let key = CoverKey { hash: 1, width: 6, height: 3 };
        covers.begin_frame();
        covers.request_image(key, &image(), ImagePhase::Stable);
        covers.end_frame();
        wait(&mut covers, key);
        assert_eq!(covers.status(key), CoverStatus::Ready);
        assert!(covers.pending.is_empty(), "preview must end with the loading request");
        let frame = covers.frame(key).unwrap().clone();
        covers.begin_frame();
        assert_eq!(covers.request_bytes(key, b"invalid", ImagePhase::Moving), CoverStatus::Ready);
        covers.end_frame();
        assert_eq!(covers.frame(key), Some(&frame));
        assert!(covers.jobs.is_empty());
        let resized = CoverKey { width: 4, height: 2, ..key };
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
        let old = CoverKey { hash: 1, width: 4, height: 2 };
        covers.begin_frame();
        covers.request_bytes(old, b"invalid", ImagePhase::Stable);
        covers.end_frame();
        wait(&mut covers, old);
        for _ in 0..4 {
            assert_eq!(covers.request_bytes(old, b"invalid", ImagePhase::Stable), CoverStatus::Failed);
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
        let key = CoverKey { hash: 1, width: 6, height: 3 };
        let preview = prepare_preview(&image(), key.width, key.height);
        assert!(preview.content.iter().all(|cell| cell.symbol() == "▀"));
        let mut covers = CoverPipeline::new(WakeSignal::default());
        covers.pending.insert(key, Pending { generation: 1, preview: Some(preview.clone()) });
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 5));
        covers.paint(&mut buffer, Rect::new(2, 1, 6, 2), Rect::new(2, 1, 6, 2), -1, 1, key);
        for y in 1..3 {
            for x in 2..7 {
                assert_eq!(buffer[(x, y)], preview[(x - 1, y)]);
            }
        }
        assert_eq!(buffer[(7, 1)].symbol(), " ");
    }
}
