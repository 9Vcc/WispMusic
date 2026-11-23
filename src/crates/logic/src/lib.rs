//! Pure listening rules shared by the players, with no UI toolkit underneath.
//!
//! The desktop app wraps these types in GPUI entities and keeps only redraw, settings and
//! toasts there.

pub mod lyrics;
pub mod queue;
pub mod station;
