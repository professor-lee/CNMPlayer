pub(crate) mod cover_pipeline;
pub mod cover_renderer;
pub(crate) mod drawer;
pub(crate) mod frame_clock;
pub mod graphics_overlay;

/// Mark one final dirty frame when a continuous animation ends.
///
/// The event loops call this before `set_continuous(false)`. Dirty frames are
/// submitted immediately, so the terminal receives the animation's final state
/// without waiting for the idle heartbeat. Only the true→false edge is marked;
/// repeating idle turns must not keep the clock dirty.
pub(crate) fn mark_animation_end_frame(
    clock: &mut frame_clock::FrameClock,
    was_continuous: bool,
    continuous: bool,
) {
    if was_continuous && !continuous {
        clock.mark_dirty();
    }
}

#[cfg(feature = "easter-egg")]
pub mod mascot;
#[cfg(feature = "easter-egg")]
pub mod mascot_frames;
pub(crate) mod motion;
pub(crate) mod snapshot;
pub(crate) mod wake;
