//! 全屏页「矢量模式」：把 PCM 波形画成李萨如图。
//!
//! 数据源与示波器相同（`audio::pcm_tap` 的共享环），映射不同：左声道作 X、
//! 右声道作 Y，逐样本画出立体声相位图（业内通行的 goniometer / vectorscope）。
//! 面板中心为原点，不画坐标轴；幅度经峰值保持自动缩放，图形始终撑满可用区域。
//!
//! 阶段 A 只接档位线：本文件先落状态骨架与重绘门禁接口，光栅与打断动画
//! 在后续提交里补齐，避免一次提交同时动枚举、布局与算法。

use crate::tmplayer::app::state::AppState;
use ratatui::Frame;
use ratatui::layout::Rect;
use std::time::Duration;

/// 矢量模式的全部可变状态：李萨如光栅、自动缩放包络与打断动画。
///
/// 存放在 [`AppState`] 里（与示波器的 `ScopeScratch` 同位），渲染路径零分配。
#[derive(Debug, Default)]
pub struct VectorState;

impl VectorState {
    /// 与示波器包络同型的推进入口：`enabled` 表示当前档位是否为矢量模式，
    /// 关掉时状态复位，切回来不会残留旧画面。
    pub(crate) fn tick(&mut self, enabled: bool, playing: bool, dt: Duration) {
        let _ = (enabled, playing, dt);
    }

    /// 快动画（分散 / 回位）进行中：需要 `spectrum_hz` 高帧率推完。
    pub(crate) fn is_animating(&self) -> bool {
        false
    }

    /// 停稳后的极慢悬浮：图形静止但仍在动，需要基础帧率持续重绘。
    pub(crate) fn is_floating(&self) -> bool {
        false
    }
}

pub fn render(f: &mut Frame, area: Rect, app: &mut AppState) {
    let _ = (f, area, app);
}
