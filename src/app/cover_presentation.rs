use crate::render::cover_pipeline::CoverKey;
use crate::render::cover_pipeline::CoverPipeline;
use crate::render::cover_pipeline::CoverStatus;
use crate::render::cover_pipeline::ImagePhase;
use crate::render::wake::WakeSignal;
use image::DynamicImage;
use ratatui::Frame;
use ratatui::layout::Rect;
use std::sync::Arc;

/// Layout records visible resources; task admission happens after painting,
/// never inside a widget or while a terminal output buffer is borrowed.
pub(crate) struct CoverPresentation {
    pipeline: CoverPipeline,
    demands: Vec<(CoverKey, Arc<DynamicImage>)>,
}

impl CoverPresentation {
    pub(super) fn new(wake: WakeSignal) -> Self {
        Self {
            pipeline: CoverPipeline::new(wake),
            demands: Vec::new(),
        }
    }

    pub(crate) fn begin_frame(&mut self) {
        self.demands.clear();
        self.pipeline.begin_frame();
    }

    pub(super) fn show(
        &mut self,
        frame: &mut Frame,
        key: CoverKey,
        image: &Arc<DynamicImage>,
        area: Rect,
        source_row: u16,
    ) {
        self.pipeline.observe(key);
        if matches!(
            self.pipeline.status(key),
            CoverStatus::Loading | CoverStatus::Preview
        ) {
            self.demands.push((key, image.clone()));
        }
        self.pipeline
            .paint(frame.buffer_mut(), area, area, 0, source_row, key);
    }

    pub(crate) fn prepare(&mut self) {
        for (key, image) in self.demands.drain(..) {
            self.pipeline.request_image(key, &image, ImagePhase::Stable);
        }
        self.pipeline.end_frame();
    }

    pub(crate) fn poll(&mut self) -> bool {
        self.pipeline.poll()
    }
}
