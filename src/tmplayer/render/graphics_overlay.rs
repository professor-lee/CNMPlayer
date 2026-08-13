use image::{DynamicImage, GenericImageView, imageops::FilterType};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui_image::{
    Resize, StatefulImage,
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::{cell::LazyCell, collections::HashMap};

use crate::{
    data::config::GraphicsProtocol, render::graphics_overlay::map_segment_to_cover_crop,
    tmplayer::app::state::AppState,
};

const FALLBACK_CELL_W_PX: u32 = 8;
const FALLBACK_CELL_H_PX: u32 = 16;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SegmentKey {
    slot: TmCoverSlot,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TmCoverSlot {
    InfoCover,
    PlaylistCover,
}

pub struct GraphicsOverlay {
    picker: Picker,
    last_term_size: Option<(u16, u16)>,
    last_content_hash: Option<u64>,
    segment_protocols: HashMap<SegmentKey, StatefulProtocol>,
}

impl GraphicsOverlay {
    pub fn new(graphics_protocol: GraphicsProtocol) -> Self {
        let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
        if let Some(proto) = graphics_protocol.to_ratatui_protocol() {
            picker.set_protocol_type(proto);
        }
        Self {
            picker,
            last_term_size: None,
            last_content_hash: None,
            segment_protocols: HashMap::new(),
        }
    }

    pub fn paint(
        &mut self,
        app: &AppState,
        frame: &mut Frame<'_>,
        info_cover: Option<&[u8]>,
        playlist_cover: Option<&[u8]>,
        info_rect: Option<Rect>,
        playlist_rect: Option<Rect>,
    ) {
        if app.config.graphics_protocol == GraphicsProtocol::Off {
            self.clear_all();
            return;
        }

        let size = frame.area();
        let current_size = (size.width, size.height);

        if self.last_term_size != Some(current_size) {
            self.last_term_size = Some(current_size);
            self.clear_all();
        }

        let hash = compute_content_hash(info_cover, playlist_cover, info_rect, playlist_rect);

        if self.last_content_hash != Some(hash) {
            self.last_content_hash = Some(hash);
            self.clear_all();
        }

        let info_image_fn = || info_cover.and_then(|x| image::load_from_memory(x).ok());
        let playlist_image_fn = || playlist_cover.and_then(|x| image::load_from_memory(x).ok());

        let halfblocks_mode = self.picker.protocol_type() == ProtocolType::Halfblocks;
        let size = self.picker.font_size();
        let cell_w_px = if size.width == 0 {
            FALLBACK_CELL_W_PX
        } else {
            u32::from(size.width)
        };
        let cell_h_px = if size.height == 0 {
            FALLBACK_CELL_H_PX
        } else {
            u32::from(size.height)
        };

        let mut paint = |rect: Rect, slot: TmCoverSlot, img: &dyn Fn() -> Option<DynamicImage>| {
            let segments = vec![rect];
            let img = LazyCell::new(img);

            for segment in segments {
                let segment_key = SegmentKey {
                    slot,
                    x: segment.x,
                    y: segment.y,
                    width: segment.width,
                    height: segment.height,
                };

                let need_init = !self.segment_protocols.contains_key(&segment_key);
                if need_init && let Some(img) = &*img {
                    let (img_w, img_h) = img.dimensions();
                    let crop = if halfblocks_mode {
                        map_segment_to_cover_crop_fill(
                            rect, segment, img_w, img_h, cell_w_px, cell_h_px,
                        )
                    } else {
                        map_segment_to_cover_crop(rect, segment, img_w, img_h)
                    };
                    let Some((crop_x, crop_y, crop_w, crop_h)) = crop else {
                        continue;
                    };
                    let mut cropped = img.crop_imm(crop_x, crop_y, crop_w, crop_h);
                    if halfblocks_mode {
                        let target_px_w = u32::from(segment.width).saturating_mul(cell_w_px);
                        let target_px_h = u32::from(segment.height).saturating_mul(cell_h_px);
                        if target_px_w > 0 && target_px_h > 0 {
                            cropped = cropped.resize_exact(
                                target_px_w,
                                target_px_h,
                                FilterType::Triangle,
                            );
                        }
                    }
                    let proto = self.picker.new_resize_protocol(cropped);
                    self.segment_protocols.insert(segment_key.clone(), proto);
                }

                if let Some(proto) = self.segment_protocols.get_mut(&segment_key) {
                    let widget = if halfblocks_mode {
                        StatefulImage::default().resize(Resize::Crop(None))
                    } else {
                        StatefulImage::default()
                    };
                    frame.render_stateful_widget(widget, segment, proto);
                }
            }
        };

        if let Some(rect) = info_rect {
            paint(rect, TmCoverSlot::InfoCover, &info_image_fn);
        }

        if let Some(rect) = playlist_rect {
            paint(rect, TmCoverSlot::PlaylistCover, &playlist_image_fn);
        }
    }

    fn clear_all(&mut self) {
        self.segment_protocols.clear();
    }
}

fn compute_content_hash(
    info: Option<&[u8]>,
    playlist: Option<&[u8]>,
    info_rect: Option<Rect>,
    playlist_rect: Option<Rect>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    info.map(|x| x.as_ptr()).hash(&mut hasher);
    playlist.map(|x| x.as_ptr()).hash(&mut hasher);
    info_rect.hash(&mut hasher);
    playlist_rect.hash(&mut hasher);
    hasher.finish()
}

fn map_segment_to_cover_crop_fill(
    base: Rect,
    segment: Rect,
    image_w: u32,
    image_h: u32,
    cell_w_px: u32,
    cell_h_px: u32,
) -> Option<(u32, u32, u32, u32)> {
    if base.width == 0 || base.height == 0 || segment.width == 0 || segment.height == 0 {
        return None;
    }
    if image_w == 0 || image_h == 0 {
        return None;
    }

    let (view_x, view_y, view_w, view_h) = cover_viewport_fill(
        image_w,
        image_h,
        base.width,
        base.height,
        cell_w_px,
        cell_h_px,
    );

    let base_w = base.width as u64;
    let base_h = base.height as u64;

    let rel_x0 = segment.x.saturating_sub(base.x) as u64;
    let rel_y0 = segment.y.saturating_sub(base.y) as u64;
    let rel_x1 = rel_x0.saturating_add(segment.width as u64);
    let rel_y1 = rel_y0.saturating_add(segment.height as u64);

    let view_w_u64 = view_w as u64;
    let view_h_u64 = view_h as u64;

    let src_x = view_x.saturating_add((rel_x0.saturating_mul(view_w_u64) / base_w) as u32);
    let src_y = view_y.saturating_add((rel_y0.saturating_mul(view_h_u64) / base_h) as u32);

    let src_x_end = view_x.saturating_add(
        ((rel_x1.saturating_mul(view_w_u64) + base_w.saturating_sub(1)) / base_w).min(view_w_u64)
            as u32,
    );
    let src_y_end = view_y.saturating_add(
        ((rel_y1.saturating_mul(view_h_u64) + base_h.saturating_sub(1)) / base_h).min(view_h_u64)
            as u32,
    );

    if src_x >= image_w || src_y >= image_h {
        return None;
    }

    let src_w = src_x_end
        .saturating_sub(src_x)
        .max(1)
        .min(image_w.saturating_sub(src_x));
    let src_h = src_y_end
        .saturating_sub(src_y)
        .max(1)
        .min(image_h.saturating_sub(src_y));

    if src_w == 0 || src_h == 0 {
        return None;
    }

    Some((src_x, src_y, src_w, src_h))
}

fn cover_viewport_fill(
    image_w: u32,
    image_h: u32,
    target_w: u16,
    target_h: u16,
    cell_w_px: u32,
    cell_h_px: u32,
) -> (u32, u32, u32, u32) {
    if target_w == 0 || target_h == 0 || image_w == 0 || image_h == 0 {
        return (0, 0, image_w.max(1), image_h.max(1));
    }

    let cell_w_px = cell_w_px.max(1);
    let cell_h_px = cell_h_px.max(1);

    let image_ratio = image_w as f64 / image_h as f64;
    let target_ratio = (target_w as f64 * cell_w_px as f64) / (target_h as f64 * cell_h_px as f64);

    if (image_ratio - target_ratio).abs() < f64::EPSILON {
        return (0, 0, image_w, image_h);
    }

    // cover semantics: always fill area, crop overflowing side.
    if image_ratio > target_ratio {
        let crop_w = ((image_h as f64) * target_ratio)
            .round()
            .clamp(1.0, image_w as f64) as u32;
        let crop_x = (image_w.saturating_sub(crop_w)) / 2;
        (crop_x, 0, crop_w, image_h)
    } else {
        let crop_h = ((image_w as f64) / target_ratio)
            .round()
            .clamp(1.0, image_h as f64) as u32;
        let crop_y = (image_h.saturating_sub(crop_h)) / 2;
        (0, crop_y, image_w, crop_h)
    }
}
