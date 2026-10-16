//! Transient UI snapshots prepared without submitting a terminal frame.
use ratatui::{Frame, Terminal, backend::Backend, buffer::Buffer};

/// Render into the staging buffer, then move its cells into the snapshot,
/// without submitting a frame. Autoresize first aligns its geometry; at an
/// unchanged size, the last submitted buffer and terminal are untouched.
/// The caller drops the snapshot as soon as the transition finishes.
pub(crate) fn capture<B: Backend>(
    terminal: &mut Terminal<B>,
    render: impl FnOnce(&mut Frame<'_>),
) -> Result<Buffer, B::Error> {
    terminal.autoresize()?;
    let mut frame = terminal.get_frame();
    let area = frame.area();
    frame.buffer_mut().reset();
    render(&mut frame);
    Ok(std::mem::replace(frame.buffer_mut(), Buffer::empty(area)))
}

#[cfg(test)]
mod tests;
