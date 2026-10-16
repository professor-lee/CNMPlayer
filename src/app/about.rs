use super::App;
use super::Overlay;
#[cfg(feature = "easter-egg")]
use super::input_controller::HitRect;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
#[cfg(feature = "easter-egg")]
use std::time::Instant;

/// about 彩蛋的推进阶段。
#[cfg(feature = "easter-egg")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum EasterEggPhase {
    /// 未触发：about 弹窗是平常的样子。
    #[default]
    Idle,
    /// 蓄力：版本行背景的噪点由稀到密。
    Charging,
    /// 迸发：噪点带自下向上划过内容区并离开。
    Bursting,
    /// 已激活：内容区显示可点击的形象。
    Active,
}

/// about 彩蛋的全部运行时状态。
///
/// 动画帧是编译期烘焙好的常量（见 `render::mascot_frames`），这里不持有帧数据。
/// 退出 about 时整体 `Default::default()` 复位。
#[cfg(feature = "easter-egg")]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AboutEasterEgg {
    /// 版本行已被点击的次数，累计到 `TRIGGER_CLICKS` 触发。
    version_clicks: u8,
    pub(crate) phase: EasterEggPhase,
    /// 当前阶段的起始时刻（蓄力、迸发各自计时）。
    pub(crate) phase_started_at: Option<Instant>,
    /// 进入激活态的时刻，用于驱动边框噪点的环形流动。
    pub(crate) mascot_activated_at: Option<Instant>,
    /// 最近一次点击形象的时刻；`None` 表示形象静止。
    pub(crate) jelly_started_at: Option<Instant>,
    /// 版本行与形象的命中区域，绘制时写回、鼠标事件时查询。
    pub(crate) version_hit: Option<HitRect>,
    pub(crate) mascot_hit: Option<HitRect>,
}

impl App {
    pub(super) fn handle_settings_about_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Enter => {
                // 彩蛋状态随 about 一起复位：下次进来需重新点满触发次数。
                #[cfg(feature = "easter-egg")]
                {
                    self.about_egg = AboutEasterEgg::default();
                }
                self.overlay = Some(Overlay::Settings);
            }
            KeyCode::Char('t') | KeyCode::Char('T')
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.close_overlay();
            }
            // 空格等价于点击一次形象：重新计时即打断当前动画重头播放。
            #[cfg(feature = "easter-egg")]
            KeyCode::Char(' ')
                if key.modifiers.is_empty() && self.about_egg.phase == EasterEggPhase::Active =>
            {
                self.about_egg.jelly_started_at = Some(Instant::now());
            }
            _ => {}
        }
    }

    /// about 弹窗里的点击：累计版本行点击触发彩蛋，或让形象再弹一次。
    #[cfg(feature = "easter-egg")]
    pub(super) fn handle_settings_about_click(&mut self, col: u16, row: u16) {
        use crate::render::mascot;

        if self.about_egg.phase == EasterEggPhase::Active {
            if let Some(rect) = self.about_egg.mascot_hit
                && rect.contains(col, row)
            {
                // 播放中再次点击即重新计时，等价于打断当前动画重头播放。
                self.about_egg.jelly_started_at = Some(Instant::now());
            }
            return;
        }

        if self.about_egg.phase != EasterEggPhase::Idle {
            return;
        }

        let Some(rect) = self.about_egg.version_hit else {
            return;
        };
        if !rect.contains(col, row) {
            return;
        }

        self.about_egg.version_clicks = self.about_egg.version_clicks.saturating_add(1);
        if self.about_egg.version_clicks >= mascot::TRIGGER_CLICKS {
            self.about_egg.phase = EasterEggPhase::Charging;
            self.about_egg.phase_started_at = Some(Instant::now());
        }
    }

    /// 推进 about 彩蛋的蓄力/迸发阶段（time-based，与帧率解耦）。
    #[cfg(feature = "easter-egg")]
    pub(super) fn tick_about_easter_egg(&mut self) {
        use crate::render::mascot;

        // 离开 about 后不该留下动画状态。
        if !matches!(self.overlay, Some(Overlay::SettingsAbout)) {
            self.about_egg = AboutEasterEgg::default();
            return;
        }

        // 果冻是一次性的：播完就回到静止形象。
        if let Some(started_at) = self.about_egg.jelly_started_at
            && started_at.elapsed() >= mascot::JELLY_DURATION
        {
            self.about_egg.jelly_started_at = None;
        }

        let Some(started_at) = self.about_egg.phase_started_at else {
            return;
        };
        let elapsed = started_at.elapsed();

        match self.about_egg.phase {
            EasterEggPhase::Charging => {
                if elapsed >= mascot::CHARGE_DURATION {
                    self.about_egg.phase = EasterEggPhase::Bursting;
                    self.about_egg.phase_started_at = Some(Instant::now());
                }
            }
            EasterEggPhase::Bursting => {
                if elapsed >= mascot::BURST_DURATION {
                    self.about_egg.phase = EasterEggPhase::Active;
                    self.about_egg.phase_started_at = None;
                    self.about_egg.mascot_activated_at = Some(Instant::now());
                }
            }
            EasterEggPhase::Idle | EasterEggPhase::Active => {}
        }
    }
}
