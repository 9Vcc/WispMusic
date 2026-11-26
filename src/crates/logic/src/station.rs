//! The rules of a station, the radio a queue is topped up from so it never runs dry.

use std::collections::HashSet;

use anyhow::{Context as _, Result};
use music::{MusicApi, RadioSeed, Track};

/// How few tracks may be left to play before radio asks for the next batch of suggestions.
pub const RADIO_LOOKAHEAD: usize = 10;
/// How few playable tracks may be left before the FM station asks for the next stretch.
pub const FM_AHEAD: usize = 30;
/// How many continuations in a row may bring nothing new before the station is dropped.
pub const DRY_LIMIT: u8 = 3;

/// How many of a collection's songs seed one stretch of its blended radio.
const BLEND_WIDTH: usize = 3;
/// How many of a collection's songs a blended radio is seeded from before it ends.
const BLEND_SEEDS: usize = 24;
/// What a blended radio's continuation starts with, so it is never mistaken for one the
/// provider handed out. The index of the next seed follows on its own line, then every seed.
const BLEND_MARK: &str = "wisp-blend";

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

/// The radio of the `seed` collection `id`, from its start or from the continuation `from`.
///
/// The collection's songs are read, shuffled and used as seeds for a blend of their track
/// radios. It opens with one of them alone, the way a song's radio opens with the song, so
/// playback starts as soon as the collection is read and the radio follows. Each later stretch
/// blends a few more of them until the seeds run out, and none of the seeds is dealt out again.
pub async fn collection_radio(
    client: &dyn MusicApi,
    seed: RadioSeed,
    id: &str,
    from: Option<&str>,
) -> Result<(Vec<Track>, Option<String>)> {
    if let Some(from) = from {
        if let Some(rest) = from.strip_prefix(BLEND_MARK) {
            let mut lines = rest.lines().filter(|line| !line.is_empty());
            let at = lines.next().and_then(|at| at.parse().ok()).unwrap_or(0);
            return blend(client, lines.map(str::to_owned).collect(), at).await;
        }
        return client.track_radio(id, Some(from)).await;
    }
    let songs = match seed {
        RadioSeed::Album => client.album_tracks(id).await?,
        RadioSeed::Playlist => client.playlist_tracks(id).await?,
        RadioSeed::Artist => client.artist_tracks(id).await?,
    };
    let mut seen = HashSet::new();
    let mut songs = songs
        .into_iter()
        .filter(|song| song.playable)
        .filter(|song| song.id.as_ref().is_some_and(|id| seen.insert(id.clone())))
        .collect::<Vec<_>>();
    fastrand::shuffle(&mut songs);
    songs.truncate(BLEND_SEEDS);
    let seeds = songs
        .iter()
        .filter_map(|song| song.id.clone())
        .collect::<Vec<_>>();
    let opener = songs
        .into_iter()
        .next()
        .context("the collection has no song to seed a radio")?;
    Ok((vec![opener], Some(continuation(&seeds, 0))))
}

/// One stretch of a blended radio: the track radios of the [`BLEND_WIDTH`] seeds from `at`,
/// asked for at once and dealt out in turn, with every seed left out. A seed whose radio fails
/// is skipped, and the stretch fails only when every one of them did. It ends the radio once no
/// seed is left after it.
async fn blend(
    client: &dyn MusicApi,
    seeds: Vec<String>,
    at: usize,
) -> Result<(Vec<Track>, Option<String>)> {
    let round = seeds
        .get(at..(at + BLEND_WIDTH).min(seeds.len()))
        .unwrap_or_default();
    let radios = futures::future::join_all(
        round
            .iter()
            .map(|seed| async move { client.track_radio(seed, None).await }),
    )
    .await;
    let mut failure = None;
    let mut lanes = Vec::new();
    for (seed, radio) in round.iter().zip(radios) {
        match radio {
            Ok((mut tracks, _)) => {
                tracks.retain(|track| !seeded(track, &seeds));
                lanes.push(tracks);
            }
            Err(error) => {
                log::warn!("station: no radio of {seed} to blend: {error:#}");
                failure.get_or_insert(error);
            }
        }
    }
    if let (true, Some(error)) = (lanes.is_empty(), failure) {
        return Err(error);
    }
    let after = at + round.len();
    let next = (after < seeds.len()).then(|| continuation(&seeds, after));
    Ok((deal(lanes), next))
}

/// The continuation of a blended radio of `seeds` whose next stretch starts at seed `at`.
fn continuation(seeds: &[String], at: usize) -> String {
    format!("{BLEND_MARK}\n{at}\n{}", seeds.join("\n"))
}

/// Whether `track` is one of the collection's songs the radio grew from.
fn seeded(track: &Track, seeds: &[String]) -> bool {
    track
        .id
        .as_ref()
        .is_some_and(|id| seeds.contains(id))
}

/// Deals the tracks of `lanes` out in turn, first of each, then second of each, leaving out any
/// track an earlier lane already dealt.
fn deal(lanes: Vec<Vec<Track>>) -> Vec<Track> {
    let mut seen = HashSet::new();
    let mut lanes = lanes.into_iter().map(Vec::into_iter).collect::<Vec<_>>();
    let mut dealt = Vec::new();
    loop {
        let mut any = false;
        for lane in &mut lanes {
            let Some(track) = lane.next() else {
                continue;
            };
            any = true;
            if track.id.as_ref().is_none_or(|id| seen.insert(id.clone())) {
                dealt.push(track);
            }
        }
        if !any {
            return dealt;
        }
    }
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
