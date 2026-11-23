//! The rules of a station, the radio a queue is topped up from so it never runs dry.

use std::collections::HashSet;

use music::Track;

/// How few tracks may be left to play before radio asks for the next batch of suggestions.
pub const RADIO_LOOKAHEAD: usize = 10;
/// How few playable tracks may be left before the FM station asks for the next stretch.
pub const FM_AHEAD: usize = 30;
/// How many continuations in a row may bring nothing new before the station is dropped.
pub const DRY_LIMIT: u8 = 3;

/// How many stretches in a row have brought nothing new once one lands with `fresh` tracks.
pub fn dry_after(dry: u8, fresh: usize) -> u8 {
    match fresh {
        0 => dry.saturating_add(1),
        _ => 0,
    }
}

/// Whether a station that came back empty `dry` times in a row is dropped.
pub fn ran_dry(dry: u8) -> bool {
    dry >= DRY_LIMIT
}

/// Whether a queue with `ahead` tracks left is short enough to ask for more.
pub fn runs_low(ahead: usize, lookahead: usize) -> bool {
    ahead < lookahead
}

/// Keeps the playable tracks that are not among `heard`, the ids a queue already holds.
pub fn unheard(tracks: &mut Vec<Track>, heard: &HashSet<String>) {
    // A provider can hand the same audio back under another album_audio_id; comparing the hash
    // as well keeps a song from playing twice after a fast skip. Repeats inside one stretch are
    // dropped too, so a batch never queues the same song twice.
    let mut ids: HashSet<String> = heard.clone();
    let mut hashes: HashSet<String> = heard
        .iter()
        .filter_map(|id| {
            id.split_once('_')
                .map(|(hash, _)| hash.to_ascii_lowercase())
        })
        .collect();
    tracks.retain(|track| {
        let Some(id) = track.id.as_ref().filter(|_| track.playable) else {
            return false;
        };
        let hash = id
            .split_once('_')
            .map(|(hash, _)| hash.to_ascii_lowercase());
        if ids.contains(id) || hash.as_ref().is_some_and(|hash| hashes.contains(hash)) {
            return false;
        }
        if let Some(hash) = hash {
            hashes.insert(hash);
        }
        ids.insert(id.clone());
        true
    });
}

/// A station the queue was started from, and the continuation its next stretch is fetched with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Station<O> {
    pub origin: O,
    pub next: String,
}

impl<O> Station<O> {
    pub fn new(origin: O, next: String) -> Self {
        Self { origin, next }
    }
}
#[cfg(test)]
mod tests {
    use music::Track;

    use super::{dry_after, ran_dry, unheard};

    fn track(id: &str) -> Track {
        Track {
            id: Some(id.to_owned()),
            playable: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_dry_run_is_dropped_after_the_limit() {
        assert_eq!(dry_after(2, 0), 3);
        assert_eq!(dry_after(2, 1), 0);
        assert!(ran_dry(3));
        assert!(!ran_dry(2));
    }

    #[test]
    fn unheard_removes_ids_and_the_same_audio_under_another_variant() {
        let mut tracks = vec![track("abc_1"), track("abc_2"), track("def_3")];
        let heard = std::collections::HashSet::new();
        unheard(&mut tracks, &heard);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[1].id.as_deref(), Some("def_3"));
    }
}
