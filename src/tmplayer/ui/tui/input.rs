use super::layout::{UiLayout, centered_rect};
use crate::tmplayer::app::state::{AppState, Overlay};
use crate::tmplayer::ui::components::control_buttons;
use crate::tmplayer::ui::panels::info_panel::{self, download_cells, heart_cells};
use crate::tmplayer::utils::input::Action;
use ratatui::layout::Rect;

pub fn hit_test(layout: &UiLayout, app: &AppState, col: u16, row: u16) -> Option<Action> {
    if matches!(
        app.overlay,
        Overlay::DownloadPathEditModal | Overlay::MprisNameEditModal
    ) {
        return None;
    }
    // Eq modal consumes clicks first
    if app.overlay == Overlay::EqModal {
        let area = centered_rect(layout.full, 44, 31);
        let inner = area.inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 1,
        });
        if inner.height >= 3 {
            let bars_rect = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: inner.height.saturating_sub(3),
            };

            if contains(bars_rect, col, row) {
                const BANDS: usize = crate::tmplayer::app::state::EQ_BANDS;
                const BAR_W: u16 = 2;
                const GAP: u16 = 1;

                let gaps_w = GAP.saturating_mul((BANDS as u16).saturating_sub(1));
                let mut cw = if bars_rect.width > gaps_w {
                    (bars_rect.width - gaps_w) / (BANDS as u16)
                } else {
                    BAR_W
                };
                cw = cw.clamp(BAR_W, 10);
                let total_w: u16 = cw.saturating_mul(BANDS as u16) + gaps_w;
                let x0 = bars_rect.x + (bars_rect.width.saturating_sub(total_w)) / 2;
                if col < x0 || col >= x0 + total_w {
                    return None;
                }

                // Find band by fixed widths; then require click within the centered BAR_W region.
                let mut band: Option<usize> = None;
                for b in 0..BANDS {
                    let col_start = x0 + (b as u16) * (cw + GAP);
                    let col_end = col_start + cw;
                    if col >= col_start && col < col_end {
                        let left_pad = cw.saturating_sub(BAR_W) / 2;
                        let bar_start = col_start + left_pad;
                        let bar_end = bar_start + BAR_W;
                        if col < bar_start || col >= bar_end {
                            return None;
                        }
                        band = Some(b);
                        break;
                    }
                }

                let band = band?;

                // fixed height mapping: prefer 25 rows (12..0..-12)
                let want_h: u16 = 25;
                let bars_h = if bars_rect.height >= want_h {
                    want_h
                } else {
                    bars_rect.height.max(3)
                };
                let y0 = bars_rect.y + (bars_rect.height.saturating_sub(bars_h)) / 2;
                if row < y0 || row >= y0 + bars_h {
                    return None;
                }
                let rr = (row - y0) as i32;

                let db_i = if bars_h == want_h {
                    (12 - rr).clamp(-12, 12)
                } else {
                    let mid = (bars_h as i32) / 2;
                    if rr == mid {
                        0
                    } else if rr < mid {
                        let level = (mid - rr) as f32;
                        let max = mid.max(1) as f32;
                        ((12.0 * (level / max)).round() as i32).clamp(0, 12)
                    } else {
                        let level = (rr - mid) as f32;
                        let max = (bars_h as i32 - 1 - mid).max(1) as f32;
                        (-(12.0 * (level / max)).round() as i32).clamp(-12, 0)
                    }
                };

                return Some(Action::EqSetBandDb {
                    band,
                    db: db_i as f32,
                });
            }
        }
    }

    // 其余弹窗盖住整页：未命中弹窗自身的点击一律吞掉，不许穿透到底层的
    // 进度条/音量/控制/播放列表（否则在设置弹窗上点一下就可能误播、
    // 误 seek、误改音量）。播放列表面板只占左栏，不在拦截范围内。
    if app.overlay != Overlay::None && app.overlay != Overlay::Playlist {
        // 设置类弹窗的条目行：单击聚焦（双击在事件循环里判定为 Enter）。
        if let Some(index) = layout.modal_rows.hit(col, row) {
            return Some(Action::ModalSelect(index));
        }
        return None;
    }

    // The rendered playlist (including its slide shell) covers every information control.
    if contains(layout.playlist_rect, col, row) || contains(layout.playlist_list_inner, col, row) {
        if contains(layout.playlist_list_inner, col, row) {
            let offset = row.saturating_sub(layout.playlist_list_inner.y) as usize;
            if offset < app.playlist_list_rows {
                return Some(Action::PlaylistSelect(app.playlist_list_scroll + offset));
            }
        }
        return None;
    }

    if contains(layout.info_controls, col, row) {
        return control_buttons::hit_test(layout.info_controls, app, col, row);
    }

    // Heart/download icons may occupy multiple cells in ASCII mode.
    if let Some((heart_x, heart_y, heart_width)) = heart_cells(layout.info_meta, app)
        && row == heart_y
        && col >= heart_x
        && col < heart_x + heart_width
    {
        return Some(Action::ToggleFavorite);
    }
    if app.download_state != crate::tmplayer::DownloadIconState::Hidden
        && let Some((download_x, download_y, download_width)) =
            download_cells(layout.info_meta, app)
        && row == download_y
        && col >= download_x
        && col < download_x + download_width
    {
        return Some(Action::ToggleDownload);
    }

    // 作者名贴 meta 块第 2 行左端，多作者按字符位置分段（"A / B" 点谁的名字进谁）：
    // 只有名字画出来的那几格可点，连接符与行尾空白都不算。
    for (index, rect) in info_panel::artist_row_hits(layout.info_meta, &app.player.track.artist) {
        if contains(rect, col, row) {
            return Some(Action::OpenAuthorPage(index));
        }
    }

    // 专辑名贴 meta 块第 3 行左端，同样只算画出来的字符：点了打开专辑页。
    if contains(
        info_panel::album_row_rect(layout.info_meta, &app.player.track.album),
        col,
        row,
    ) {
        return Some(Action::OpenAlbumPage);
    }

    if let Some(volume) = volume_at(layout, col, row) {
        return Some(Action::SetVolume(volume));
    }

    if contains(layout.info_progress, col, row)
        && let Some(ratio) = progress_for_drag(layout, col)
    {
        return Some(Action::SeekToFraction(ratio));
    }

    None
}

/// 滚轮是否落在播放列表面板上（面板打开时才响应滚动聚焦）。
pub fn wheel_over_playlist(layout: &UiLayout, app: &AppState, col: u16, row: u16) -> bool {
    app.overlay == Overlay::Playlist && contains(layout.playlist_rect, col, row)
}

/// 音量条上某列对应的音量（0..=1）；不在条内时返回 `None`（点击用）。
pub fn volume_at(layout: &UiLayout, col: u16, row: u16) -> Option<f32> {
    contains(layout.info_volume, col, row).then(|| ratio_in_bar(layout.info_volume, col))
}

/// 按住拖动时的音量换算：列超出条子按端点钳制、不再要求落在条内，
/// 这样"起点在条内、拖出条外"仍持续生效。
pub fn volume_for_drag(layout: &UiLayout, col: u16) -> Option<f32> {
    (layout.info_volume.width > 2 && layout.info_volume.height > 0)
        .then(|| ratio_in_bar(layout.info_volume, col))
}

/// 进度拖动与点击共用实际绘制的轨道；离开轨道行后仍按列换算并钳制。
pub fn progress_for_drag(layout: &UiLayout, col: u16) -> Option<f32> {
    (layout.info_progress.width > 1 && layout.info_progress.height > 0)
        .then(|| ratio_in_track(layout.info_progress, col))
}

pub(super) fn contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

fn ratio_in_bar(r: Rect, col: u16) -> f32 {
    if r.width <= 2 {
        return 0.0;
    }
    let inner = (r.width - 2) as f32;
    let x = col.saturating_sub(r.x + 1) as f32;
    (x / inner).clamp(0.0, 1.0)
}

fn ratio_in_track(r: Rect, col: u16) -> f32 {
    if r.width <= 1 {
        return 0.0;
    }
    let denom = (r.width - 1) as f32;
    let x = col.saturating_sub(r.x) as f32;
    (x / denom).clamp(0.0, 1.0)
}
