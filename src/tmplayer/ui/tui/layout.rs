use super::input::contains;
use ratatui::layout::Rect;

#[derive(Debug, Default, Clone, Copy)]
pub struct UiLayout {
    pub full: Rect,
    pub left: Rect,
    pub right: Rect,
    pub left_width: u16,

    pub info_progress: Rect,
    pub info_volume: Rect,
    pub info_controls: Rect,
    /// 标题/爱心行（爱心贴该行右端）。
    pub info_meta: Rect,

    pub playlist_rect: Rect,
    pub playlist_inner: Rect,
    pub playlist_list_inner: Rect,

    pub spectrum_rect: Rect,
    /// 当前弹窗（若有）的条目行。
    pub modal_rows: ModalRows,
}

pub(super) fn centered_rect(size: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(size.width.saturating_sub(4)).max(10);
    let h = height.min(size.height.saturating_sub(4)).max(6);
    Rect {
        x: size.x + (size.width.saturating_sub(w)) / 2,
        y: size.y + (size.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// 当前弹窗里可点击的条目行，按条目序号逐行登记。
///
/// `Copy` 且定长，好跟着 `UiLayout` 一起传出来；渲染时填、`hit_test` 时查，
/// 两边共用同一份矩形，不会各算一遍偏移。
#[derive(Debug, Clone, Copy)]
pub struct ModalRows {
    rows: [(usize, Rect); ModalRows::MAX],
    len: usize,
}

impl ModalRows {
    /// 单个弹窗的行数上限。取最长的一个（按键提示弹窗 17 条）再留些余量；
    /// 超出的行会被丢弃，所以新增更长的弹窗列表时要同步调大。
    pub const MAX: usize = 24;

    /// 登记一行：`index` 是该行代表的**条目序号**，不是登记次序。
    ///
    /// 弹窗列表会被截断（终端太矮）或被滚动（按键提示弹窗），行号与条目号
    /// 并不相等，消费端要的是条目号，所以两者必须分开存。
    /// 超出上限的行被丢弃（渲染本身也会被裁掉）。
    pub(super) fn push(&mut self, rect: Rect, index: usize) {
        if self.len < Self::MAX && rect.width > 0 && rect.height > 0 {
            self.rows[self.len] = (index, rect);
            self.len += 1;
        }
    }

    /// 某个条目序号画在哪一行（该条目未显示时为 `None`）。
    pub fn get(&self, index: usize) -> Option<Rect> {
        self.rows[..self.len]
            .iter()
            .find(|(row_index, _)| *row_index == index)
            .map(|(_, rect)| *rect)
    }

    /// 命中的条目序号。
    pub(super) fn hit(&self, col: u16, row: u16) -> Option<usize> {
        self.rows[..self.len]
            .iter()
            .find(|(_, rect)| contains(*rect, col, row))
            .map(|(index, _)| *index)
    }
}

impl Default for ModalRows {
    fn default() -> Self {
        Self {
            rows: [(0, Rect::default()); Self::MAX],
            len: 0,
        }
    }
}

#[cfg(test)]
mod tests;
