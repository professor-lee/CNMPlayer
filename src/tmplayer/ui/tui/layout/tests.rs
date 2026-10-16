use super::ModalRows;

impl ModalRows {
    /// 已登记的行数（只在测试里用，release 构建不该带上）。
    pub(in crate::tmplayer::ui::tui) fn len(&self) -> usize {
        self.len
    }
}
