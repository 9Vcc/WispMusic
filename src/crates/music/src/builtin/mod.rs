//! Built-in host sources that stay available without a provider plugin.
//!
//! The modules keep their service-specific endpoints private; the app only depends on the
//! neutral registry and the unblock configuration.

mod lyrics_a;
mod lyrics_b;
mod unblock;
mod unm;

pub use unm::{UnblockConfig, set_unblock_config};

use std::sync::Arc;

use crate::{LyricsProvider, Quality, Track};

pub fn lyrics_sources() -> Vec<Arc<dyn LyricsProvider>> {
    vec![
        Arc::new(lyrics_a::SourceA::new()),
        Arc::new(lyrics_b::SourceB::new()),
    ]
}

pub(crate) async fn unblock_for(track: &Track, quality: Quality) -> Option<(String, &'static str)> {
    let info = unm::TrackInfo::from_track(track)?;
    unblock::match_url(&info, quality).await
}

/// Remembers a track the player already knows, so an unblock can start without asking the
/// provider for its detail again.
pub fn remember(track: &Track) {
    unm::remember(track);
}

/// The unblock path for a track the player has just remembered. `None` falls back to asking
/// the provider for the detail.
pub(crate) async fn unblock_for_id(
    track_id: &str,
    quality: Quality,
) -> Option<(String, &'static str)> {
    let info = unm::remembered(track_id)?;
    unblock::match_url(&info, quality).await
}