//! The play order around the current song: what was played, what plays now and what comes next.
//!
//! [`Order`] is the whole model. It knows where a hand-queued song goes, how shuffle mixes and
//! restores a collection, what a selection does to the songs it skips, and what is saved for the
//! next launch. It holds no UI state, so the desktop app wraps it in an entity and a phone can
//! drive it from another player.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use music::{ArtistRef, Track};
use serde::{Deserialize, Serialize};

mod sections;
pub use sections::{QueuePosition, QueueSections, QueueSlot};

const PAST_LIMIT: usize = 500;
const KEPT_PAST: usize = 20;
const KEPT_UPCOMING: usize = 200;

/// A queue as it is saved for the next launch. `O` is how the app names the collection a track
/// was queued from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    default,
    bound(serialize = "O: Serialize", deserialize = "O: Deserialize<'de>")
)]
pub struct Resume<O> {
    pub provider: String,
    pub position: f32,
    /// The one origin older versions saved for the whole queue, read only to restore such a queue.
    #[serde(skip_serializing)]
    pub origin: Option<O>,
    /// The collections the saved tracks were queued from. Each stub points into this list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub origins: Vec<O>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<Stub>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub past: Vec<Stub>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub upcoming: Vec<Stub>,
    /// Legacy boundary, read only to migrate queues saved before per-song provenance.
    #[serde(skip_serializing)]
    pub manual: usize,
}

/// A track as a saved queue keeps it: what the queue and the player bar show of it, which is
/// enough to list it and to ask the provider for it again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stub {
    pub id: String,
    pub name: String,
    pub artists: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub credited: Vec<ArtistRef>,
    pub album: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub album_artist: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    pub seconds: f32,
    pub explicit: bool,
    /// Which of the resume's `origins` the track was queued from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<usize>,
    /// Provenance survives saving and replaying history.
    #[serde(skip_serializing_if = "QueueKind::is_context")]
    pub kind: QueueKind,
}

impl<O> Default for Resume<O> {
    fn default() -> Self {
        Self {
            provider: String::new(),
            position: 0.,
            origin: None,
            origins: Vec::new(),
            current: None,
            past: Vec::new(),
            upcoming: Vec::new(),
            manual: 0,
        }
    }
}

/// One occurrence with identity, collection and provenance independent of position.
/// Source and active orders share its immutable song metadata.
#[derive(Clone, Debug, PartialEq)]
struct Entry<O> {
    uid: String,
    track: Arc<Track>,
    origin: Option<O>,
    kind: QueueKind,
}

/// Where fetched tracks go in the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueuePlacement {
    /// Right after the current track.
    Next,
    /// After the tracks queued by hand, ahead of the rest of the source.
    End,
    /// After everything, once the source has run out.
    Last,
    /// After this many upcoming tracks; a gap past the end appends.
    Gap(usize),
}

impl QueuePlacement {
    /// The toast key for a placement worth announcing.
    pub fn toast(self, source: &str) -> Option<String> {
        match self {
            Self::Next => Some(format!("toast-next-{source}")),
            Self::End => Some(format!("toast-queued-{source}")),
            Self::Last => Some(format!("toast-last-{source}")),
            Self::Gap(_) => None,
        }
    }
}

/// Why a song is in play order, independent of where it currently sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueKind {
    #[default]
    Context,
    Queued,
    Suggested,
}

impl QueueKind {
    fn is_context(&self) -> bool {
        *self == Self::Context
    }

    pub fn heading(self) -> &'static str {
        match self {
            Self::Context => "queue-up-next",
            Self::Queued => "queue-next-in-queue",
            Self::Suggested => "queue-similar",
        }
    }
}

impl<O> Entry<O> {
    fn new(track: Track, origin: Option<O>, kind: QueueKind) -> Self {
        Self::with_uid(queue_uid(), track, origin, kind)
    }

    fn with_uid(uid: String, track: Track, origin: Option<O>, kind: QueueKind) -> Self {
        Self {
            uid,
            track: Arc::new(track),
            origin,
            kind,
        }
    }
}

/// A track removed from the queue on its own, and where it sat. Handing it back to
/// [`Order::put_back`] undoes the removal.
pub struct Removed<O> {
    entry: Entry<O>,
    index: usize,
    batch: u64,
}

impl<O> Removed<O> {
    /// The name of the removed track.
    pub fn name(&self) -> &str {
        &self.entry.track.name
    }
}

fn stub(track: &Track) -> Option<Stub> {
    Some(Stub {
        id: track.id.clone()?,
        name: track.name.clone(),
        artists: track.artists.clone(),
        credited: track.artist_refs.clone(),
        album: track.album.clone(),
        album_artist: track.album_artist.clone(),
        album_id: track.album_id.clone(),
        cover: track.cover.clone(),
        seconds: track.duration.as_secs_f32(),
        explicit: track.explicit,
        from: None,
        kind: QueueKind::Context,
    })
}

fn hydrate(stub: Stub) -> Track {
    Track {
        id: Some(stub.id),
        name: stub.name,
        playable: true,
        artists: stub.artists,
        artist_refs: stub.credited,
        album: stub.album,
        album_artist: stub.album_artist,
        album_id: stub.album_id,
        cover: stub.cover,
        duration: Duration::from_secs_f32(stub.seconds.max(0.)),
        explicit: stub.explicit,
        ..Default::default()
    }
}

/// Snapshots provenance and collection metadata for the next launch, omitting suggestions.
fn record<'a, O: Clone + PartialEq + 'a>(
    provider: &str,
    past: &[Entry<O>],
    current: Option<&Entry<O>>,
    upcoming: impl Iterator<Item = &'a Entry<O>>,
) -> Resume<O> {
    let mut origins: Vec<O> = Vec::new();
    let mut save = |entry: &Entry<O>| {
        let mut stub = stub(&entry.track)?;
        stub.kind = entry.kind;
        stub.from = entry.origin.as_ref().map(|origin| {
            match origins.iter().position(|known| known == origin) {
                Some(index) => index,
                None => {
                    origins.push(origin.clone());
                    origins.len() - 1
                }
            }
        });
        Some(stub)
    };
    let past = past[past.len().saturating_sub(KEPT_PAST)..]
        .iter()
        .filter_map(&mut save)
        .collect();
    let current = current.and_then(&mut save);
    let upcoming = upcoming
        .filter(|entry| entry.kind != QueueKind::Suggested)
        .take(KEPT_UPCOMING)
        .filter_map(save)
        .collect();
    Resume {
        provider: provider.to_owned(),
        position: 0.,
        origin: None,
        origins,
        current,
        past,
        upcoming,
        manual: 0,
    }
}

fn local(track: &Track) -> bool {
    track.id.as_deref().is_some_and(music::is_local_id)
}

fn sift<T>(
    past: &mut Vec<T>,
    current: &mut Option<T>,
    upcoming: &mut VecDeque<T>,
    source: &mut Vec<T>,
    keep: impl Fn(&T) -> bool,
) -> bool {
    let tally = |past: &Vec<T>, current: &Option<T>, upcoming: &VecDeque<T>, source: &Vec<T>| {
        past.len() + usize::from(current.is_some()) + upcoming.len() + source.len()
    };

    let before = tally(past, current, upcoming, source);
    past.retain(&keep);
    if current.as_ref().is_some_and(|item| !keep(item)) {
        *current = None;
    }
    upcoming.retain(&keep);
    source.retain(&keep);
    before != tally(past, current, upcoming, source)
}

/// Takes the next song out of `upcoming`: its first in queue order, or any one at random.
fn take_next<O>(upcoming: &mut VecDeque<Entry<O>>, shuffle: bool) -> Option<Entry<O>> {
    match shuffle {
        true => {
            let at = (!upcoming.is_empty()).then(|| fastrand::usize(..upcoming.len()))?;
            upcoming.remove(at)
        }
        false => upcoming.pop_front(),
    }
}

/// The collection in its own order, with occurrences that are not part of it kept after, each
/// occurrence once. Random play needs this so a loop never rearranges the list by the order
/// songs happened to be picked in.
fn ordered_entries<O: Clone>(
    source: &[Entry<O>],
    past: Vec<Entry<O>>,
    current: Option<Entry<O>>,
    upcoming: VecDeque<Entry<O>>,
) -> Vec<Entry<O>> {
    let known: HashSet<String> = source.iter().map(|entry| entry.uid.clone()).collect();
    let mut entries = source.to_vec();
    entries.extend(
        past.into_iter()
            .chain(current)
            .chain(upcoming)
            .filter(|entry| !known.contains(&entry.uid)),
    );
    entries
}

/// Takes a random song for the previous button from the history or what is left.
fn take_previous_random<O>(
    past: &mut Vec<Entry<O>>,
    upcoming: &mut VecDeque<Entry<O>>,
) -> Option<Entry<O>> {
    while past.len() + upcoming.len() > 0 {
        let at = fastrand::usize(..past.len() + upcoming.len());
        let entry = match at < past.len() {
            true => past.remove(at),
            false => upcoming.remove(at - past.len())?,
        };
        if entry.track.playable {
            return Some(entry);
        }
    }
    None
}

/// Keeps the latest play of each occurrence in the history.
/// Drops repeats of one occurrence a merge brought together, keeping the first.
fn unique<O>(entries: Vec<Entry<O>>) -> Vec<Entry<O>> {
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .filter(|entry| seen.insert(entry.uid.clone()))
        .collect()
}

fn unique_played<O>(past: &mut Vec<Entry<O>>) {
    let mut seen = HashSet::new();
    let mut kept = Vec::with_capacity(past.len());
    for entry in past.drain(..).rev() {
        if seen.insert(entry.uid.clone()) {
            kept.push(entry);
        }
    }
    kept.reverse();
    *past = kept;
}

fn move_item<T>(items: &mut VecDeque<T>, from: usize, to: usize) -> bool {
    if from >= items.len() || to >= items.len() || from == to {
        return false;
    }
    let Some(item) = items.remove(from) else {
        return false;
    };
    items.insert(to, item);
    true
}

fn select_past<T>(
    past: &mut Vec<T>,
    current: &mut Option<T>,
    upcoming: &mut VecDeque<T>,
    index: usize,
) -> bool {
    if index >= past.len() {
        return false;
    }

    let mut replay = VecDeque::from(past.split_off(index));
    let selected = replay.pop_front().expect("past index was checked");
    if let Some(playing) = current.replace(selected) {
        replay.push_back(playing);
    }
    replay.append(upcoming);
    *upcoming = replay;
    true
}

fn select_upcoming<T>(
    past: &mut Vec<T>,
    current: &mut Option<T>,
    upcoming: &mut VecDeque<T>,
    index: usize,
    preserve: impl Fn(&T) -> bool,
) -> bool {
    let Some(selected) = upcoming.remove(index) else {
        return false;
    };

    past.extend(current.replace(selected));
    let mut kept = VecDeque::new();
    for entry in upcoming.drain(..index) {
        match preserve(&entry) {
            true => kept.push_back(entry),
            false => past.push(entry),
        }
    }
    kept.append(upcoming);
    *upcoming = kept;
    true
}

fn trim<T>(past: &mut Vec<T>, limit: usize) {
    past.drain(..past.len().saturating_sub(limit));
}

fn in_order<'a, T: PartialEq + 'a>(sequence: impl Iterator<Item = &'a T>, source: &[T]) -> bool {
    sequence.eq(source.iter())
}

/// The index a song at `from` ends up at when it is dropped into `gap` of a list of `len`, the gap
/// `n` being the one in front of the song at `n`. Taking the song out first moves every gap
/// behind it down by one.
pub fn gap_target(from: usize, gap: usize, len: usize) -> usize {
    let gap = gap.min(len);
    if gap > from { gap - 1 } else { gap }
}

/// Pairs each track with the collection it was queued from.
fn tagged<O: Clone>(
    tracks: impl IntoIterator<Item = Track>,
    origin: Option<O>,
    kind: QueueKind,
) -> impl Iterator<Item = Entry<O>> {
    tracks
        .into_iter()
        .map(move |track| Entry::new(track, origin.clone(), kind))
}

/// The collection order a play order keeps for reset and unshuffle.
fn source_of<O>(entries: impl IntoIterator<Item = Entry<O>>) -> Vec<Entry<O>> {
    entries
        .into_iter()
        .filter(|entry| entry.kind == QueueKind::Context)
        .collect()
}

/// The occurrences a rewind starts over with: the history, then the current song, then the
/// order, each occurrence once.
fn rewind_entries<O>(
    past: Vec<Entry<O>>,
    current: Option<Entry<O>>,
    upcoming: VecDeque<Entry<O>>,
) -> Vec<Entry<O>> {
    let mut entries = past;
    entries.extend(current);
    entries.extend(upcoming);
    unique(entries)
}

/// One provider's play order, kept while another provider is on show.
#[derive(Clone)]
struct Held<O> {
    past: Vec<Entry<O>>,
    current: Option<Entry<O>>,
    upcoming: VecDeque<Entry<O>>,
    source: Vec<Entry<O>>,
}

/// One play order around the current song. Entries retain their provenance when moved;
/// `source` keeps only the original collection order for reset and unshuffle. Every edit that
/// changes the order moves [`Order::revision`] on, which is how an owner learns to redraw and
/// save.
pub struct Order<O> {
    past: Vec<Entry<O>>,
    current: Option<Entry<O>>,
    upcoming: VecDeque<Entry<O>>,
    source: Vec<Entry<O>>,
    provider: Option<String>,
    held: HashMap<String, Held<O>>,
    /// Derived runs are rebuilt on edits, so playback-clock redraws never scan the playlist.
    pub sections: QueueSections,
    shuffle: bool,
    revision: u64,
    /// Counts the times the suggestions were replaced or dropped.
    suggested: u64,
}

impl<O: Clone + PartialEq> Order<O> {
    /// An empty order that shuffles what it is given when `shuffle` is on.
    pub fn new(shuffle: bool) -> Self {
        Self {
            past: Vec::new(),
            current: None,
            upcoming: VecDeque::new(),
            source: Vec::new(),
            provider: None,
            held: HashMap::new(),
            sections: QueueSections::new(0, false, [], shuffle),
            shuffle,
            revision: 0,
            suggested: 0,
        }
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;
        self.changed();
    }

    /// Changes only this queue's order, leaving the saved shuffle preference alone.
    pub fn force_shuffle(&mut self, on: bool) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;
        self.changed();
    }

    fn changed(&mut self) {
        unique_played(&mut self.past);
        trim(&mut self.past, PAST_LIMIT);
        self.sections = QueueSections::new(
            self.past.len(),
            self.current.is_some(),
            self.upcoming.iter().map(|entry| entry.kind),
            self.shuffle,
        );
        self.revision = self.revision.wrapping_add(1);
    }

    fn blank(&self) -> bool {
        self.past.is_empty() && self.current.is_none() && self.upcoming.is_empty()
    }

    /// The provider this order belongs to.
    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    /// Adopts the current provider when the order first fills, without touching its contents.
    pub fn ensure_provider(&mut self, next: Option<String>) {
        if self.provider.is_none() && !self.blank() {
            self.provider = next;
        }
    }

    /// Puts this order aside when the provider changes, and brings back the one the new
    /// provider left behind. A provider with no queue of its own starts empty.
    pub fn switch_provider(&mut self, next: Option<String>) {
        if self.provider == next {
            return;
        }
        let before = self.blank();
        if let Some(old) = self.provider.take()
            && !before
        {
            self.held.insert(old, self.hold_contents());
        }
        self.clear_contents();
        self.provider = next;
        let restored = self
            .provider
            .as_ref()
            .and_then(|provider| self.held.remove(provider));
        match restored {
            Some(held) => {
                self.restore_contents(held);
                self.changed();
            }
            None => self.cleared(),
        }
    }

    /// Empties the visible queue without touching the resume another provider saved.
    fn cleared(&mut self) {
        self.sections = QueueSections::new(0, false, [], self.shuffle);
        self.revision = self.revision.wrapping_add(1);
    }

    fn hold_contents(&self) -> Held<O> {
        Held {
            past: self.past.clone(),
            current: self.current.clone(),
            upcoming: self.upcoming.clone(),
            source: self.source.clone(),
        }
    }

    fn restore_contents(&mut self, held: Held<O>) {
        self.past = held.past;
        self.current = held.current;
        self.upcoming = held.upcoming;
        self.source = held.source;
        self.suggested += 1;
    }

    fn clear_contents(&mut self) {
        self.past.clear();
        self.current = None;
        self.upcoming.clear();
        self.source.clear();
        self.suggested += 1;
    }

    /// What to save of this order for the next launch of `provider`, or nothing when it is empty.
    pub fn record(&self, provider: &str) -> Option<Resume<O>> {
        (!self.blank()).then(|| {
            record(
                provider,
                &self.past,
                self.current.as_ref(),
                self.upcoming.iter(),
            )
        })
    }

    /// Restores a saved queue. A queue saved by an older version has one origin for all of it,
    /// which goes to every track except the ones queued by hand.
    pub fn revive(&mut self, resume: Resume<O>) -> Option<Track> {
        if !resume.provider.is_empty() {
            self.provider = Some(resume.provider.clone());
        }
        let origins = resume.origins;
        let entry = |stub: Stub| {
            let kind = stub.kind;
            let origin = stub.from.and_then(|index| origins.get(index).cloned());
            Entry::new(hydrate(stub), origin, kind)
        };
        self.past = resume.past.into_iter().map(entry).collect();
        self.current = resume.current.map(entry);
        self.upcoming = resume.upcoming.into_iter().map(entry).collect();
        for entry in self.upcoming.iter_mut().take(resume.manual) {
            entry.kind = QueueKind::Queued;
        }
        self.suggested += 1;
        if let Some(origin) = resume.origin {
            self.past
                .iter_mut()
                .chain(self.current.as_mut())
                .chain(self.upcoming.iter_mut())
                .filter(|entry| entry.kind == QueueKind::Context)
                .for_each(|entry| entry.origin = Some(origin.clone()));
        }
        self.source = source_of(
            self.past
                .iter()
                .chain(self.current.as_ref())
                .chain(self.upcoming.iter())
                .cloned(),
        );
        self.changed();
        self.current().cloned()
    }

    /// Drops the recommendations, saying whether there were any.
    fn drop_suggested(&mut self) -> bool {
        let before = self.upcoming.len();
        self.upcoming
            .retain(|entry| entry.kind != QueueKind::Suggested);
        before != self.upcoming.len()
    }

    /// Drops everything but the listener's own files, as when the account signs out.
    pub fn purge(&mut self) {
        let suggested = self.drop_suggested();
        self.suggested += 1;
        let sifted = sift(
            &mut self.past,
            &mut self.current,
            &mut self.upcoming,
            &mut self.source,
            |entry| local(&entry.track),
        );
        if suggested || sifted {
            self.changed();
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn past(&self) -> impl ExactSizeIterator<Item = &Track> {
        self.past.iter().map(|entry| entry.track.as_ref())
    }

    pub fn current(&self) -> Option<&Track> {
        self.current.as_ref().map(|entry| entry.track.as_ref())
    }

    /// Ends a queue that has run out. Its last song moves to the queue history.
    pub fn finish(&mut self) {
        let Some(last) = self.current.take() else {
            return;
        };
        self.past.push(last);
        self.changed();
    }

    /// The collection the current track was queued from. `None` while nothing plays or when the
    /// user added the current track on its own.
    pub fn origin(&self) -> Option<&O> {
        self.current.as_ref()?.origin.as_ref()
    }

    /// The occurrence identity of the song playing, independent of its queue position.
    pub fn current_uid(&self) -> Option<&str> {
        self.current.as_ref().map(|entry| entry.uid.as_str())
    }

    /// Every track the queue holds, for reading a remote queue without asking the provider
    /// again for what is already here.
    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.past
            .iter()
            .chain(self.current.as_ref())
            .chain(self.upcoming.iter())
            .map(|entry| entry.track.as_ref())
    }

    /// The underlying collection, retained while a hand-queued track is playing.
    pub fn context_origin(&self) -> Option<&O> {
        self.source.iter().find_map(|entry| entry.origin.as_ref())
    }

    /// Where the current track sits in the collection the queue was started from. `None` when it
    /// is not in that collection, as for a track queued by hand.
    pub fn place(&self) -> Option<usize> {
        let current = self.current.as_ref()?;
        self.source
            .iter()
            .position(|entry| entry.uid == current.uid)
    }

    /// The full upcoming play order, including recommendations.
    pub fn upcoming(&self) -> impl ExactSizeIterator<Item = &Track> {
        self.upcoming.iter().map(|entry| entry.track.as_ref())
    }

    /// How many playable songs are left to play before the recommendations.
    pub fn ahead(&self) -> usize {
        self.upcoming
            .iter()
            .filter(|entry| entry.kind != QueueKind::Suggested && entry.track.playable)
            .count()
    }

    /// Replaces the current row with a fuller copy of the same song, as when a queued stub is
    /// resolved by the provider for its duration, artists or lyrics.
    pub fn replace_current(&mut self, track: &Track) {
        let Some(current) = &mut self.current else {
            return;
        };
        if current.track.id != track.id {
            return;
        }
        current.track = Arc::new(track.clone());
        self.changed();
    }

    pub fn similar(&self) -> impl Iterator<Item = &Track> {
        self.upcoming
            .iter()
            .filter(|entry| entry.kind == QueueKind::Suggested)
            .map(|entry| entry.track.as_ref())
    }

    pub fn kind(&self, position: QueuePosition) -> Option<QueueKind> {
        self.entry(position).map(|entry| entry.kind)
    }

    fn entry(&self, position: QueuePosition) -> Option<&Entry<O>> {
        match position {
            QueuePosition::Past(at) => self.past.get(at),
            QueuePosition::Current => self.current.as_ref(),
            QueuePosition::Upcoming(at) => self.upcoming.get(at),
        }
    }

    /// Resolves an occurrence identity without depending on a previously rendered index.
    pub fn position(&self, uid: &str) -> Option<QueuePosition> {
        if self.current_uid() == Some(uid) {
            return Some(QueuePosition::Current);
        }
        self.past
            .iter()
            .position(|entry| entry.uid == uid)
            .map(QueuePosition::Past)
            .or_else(|| {
                self.upcoming
                    .iter()
                    .position(|entry| entry.uid == uid)
                    .map(QueuePosition::Upcoming)
            })
    }

    pub fn uid(&self, position: QueuePosition) -> Option<&str> {
        self.entry(position).map(|entry| entry.uid.as_str())
    }

    /// Replaces the suggestions with `tracks` from the radio `origin`.
    pub fn suggest(&mut self, tracks: Vec<Track>, origin: O) {
        self.drop_suggested();
        self.suggested += 1;
        self.upcoming
            .extend(tagged(tracks, Some(origin), QueueKind::Suggested));
        self.changed();
    }

    /// Adds suggestions after the ones already there, for a station topped up before the queue
    /// has played through it.
    pub fn extend_similar(&mut self, tracks: Vec<Track>, origin: O) {
        if tracks.is_empty() {
            return;
        }
        self.upcoming
            .extend(tagged(tracks, Some(origin), QueueKind::Suggested));
        self.changed();
    }

    pub fn clear_similar(&mut self) {
        if !self.drop_suggested() {
            return;
        }
        self.suggested += 1;
        self.changed();
    }

    pub fn ids(&self) -> HashSet<String> {
        self.past
            .iter()
            .chain(self.current.as_ref())
            .chain(self.upcoming.iter())
            .filter_map(|entry| entry.track.id.clone())
            .collect()
    }

    pub fn remove_tracks(&mut self, ids: &[String]) -> bool {
        if ids.is_empty() {
            return false;
        }
        let matches = |entry: &Entry<O>| entry.track.id.as_ref().is_some_and(|id| ids.contains(id));
        let current_removed = self.current.as_ref().is_some_and(matches);
        if sift(
            &mut self.past,
            &mut self.current,
            &mut self.upcoming,
            &mut self.source,
            |entry| !matches(entry),
        ) {
            self.changed();
        }
        current_removed
    }

    pub fn is_empty(&self) -> bool {
        self.current.is_none() && self.upcoming.is_empty()
    }

    pub fn has_next(&self) -> bool {
        !self.upcoming.is_empty()
    }

    /// Whether the track `next` would hand back is a radio suggestion rather than one the queue
    /// was started with.
    pub fn next_is_suggested(&self) -> bool {
        self.upcoming
            .front()
            .is_some_and(|entry| entry.kind == QueueKind::Suggested)
    }

    pub fn has_previous(&self) -> bool {
        !self.past.is_empty()
    }

    /// Drops everything queued, keeping the suggestions, and answers what it dropped so the
    /// owner can offer to put it back.
    pub fn clear_upcoming(&mut self) -> Vec<Removed<O>> {
        let mut removed = Vec::new();
        let mut kept = 0;
        self.upcoming.retain(|entry| {
            if entry.kind == QueueKind::Suggested {
                kept += 1;
                return true;
            }
            removed.push(Removed {
                entry: entry.clone(),
                index: kept,
                batch: self.suggested,
            });
            false
        });
        if !removed.is_empty() {
            self.changed();
        }
        removed
    }

    pub fn reordered(&self) -> bool {
        if self.current.is_none() {
            return false;
        }
        let sequence = self
            .past
            .iter()
            .chain(self.current.as_ref())
            .chain(self.upcoming.iter());

        !self.source.is_empty() && !in_order(sequence, &self.source)
    }

    pub fn reset(&mut self) -> Option<Track> {
        if self.source.is_empty() {
            return None;
        }

        let index = self.place().unwrap_or_default();
        let source = self.source.clone();
        self.fill(source, index)
    }

    /// Makes `tracks` the queue, all of them queued from `origin`, and plays the one at `index`.
    pub fn start(&mut self, tracks: Vec<Track>, index: usize, origin: Option<O>) -> Option<Track> {
        self.fill(tagged(tracks, origin, QueueKind::Context).collect(), index)
    }

    /// Rebuilds play order while retaining only collection entries for reset and unshuffle.
    fn fill(&mut self, entries: Vec<Entry<O>>, index: usize) -> Option<Track> {
        if index >= entries.len() {
            return None;
        }

        self.source = source_of(entries.iter().cloned());
        let mut past = entries;
        self.upcoming = past.split_off(index + 1).into();
        self.suggested += 1;
        self.current = past.pop();
        self.past = past;
        self.changed();
        self.current().cloned()
    }

    /// Inserts hand-queued tracks at `placement`, keeping their order and provenance.
    pub fn insert(
        &mut self,
        tracks: impl IntoIterator<Item = Track>,
        origin: Option<O>,
        placement: QueuePlacement,
    ) {
        let (at, kind) = match placement {
            QueuePlacement::Next => (0, QueueKind::Queued),
            QueuePlacement::End => (self.manual_end(), QueueKind::Queued),
            // what plays once the collection has run out is not part of the hand-queued run
            QueuePlacement::Last => (self.context_end(), QueueKind::Context),
            QueuePlacement::Gap(gap) => {
                // a song dropped among the suggestions lands at the end of the queue instead
                let at = gap.min(self.context_end());
                let kind = match joins_manual(self.upcoming.iter(), at) {
                    true => QueueKind::Queued,
                    false => QueueKind::Context,
                };
                (at, kind)
            }
        };
        self.insert_entries(at, tagged(tracks, origin, kind));
        self.changed();
    }

    /// The gap right after the hand-queued run, where Add to queue puts a song. Hand-queued
    /// songs only ever sit in that run, which songs replayed from the history may precede.
    pub fn manual_end(&self) -> usize {
        self.upcoming
            .iter()
            .rposition(|entry| entry.kind == QueueKind::Queued)
            .map_or(0, |at| at + 1)
    }

    /// Adds the next stretch of the station the queue was started from, both to the source and
    /// after what is queued.
    pub fn extend_context(&mut self, tracks: Vec<Track>, origin: O) {
        if tracks.is_empty() {
            return;
        }
        let at = self.context_end();
        let entries: Vec<Entry<O>> = tagged(tracks, Some(origin), QueueKind::Context).collect();
        self.source.extend(entries.iter().cloned());
        self.insert_entries(at, entries);
        self.changed();
    }

    /// Inserts `entries` at `at`, keeping their order.
    fn insert_entries(&mut self, at: usize, entries: impl IntoIterator<Item = Entry<O>>) {
        for (offset, entry) in entries.into_iter().enumerate() {
            self.upcoming.insert(at + offset, entry);
        }
    }

    /// The gap before trailing recommendations, even after earlier recommendations were replayed.
    fn context_end(&self) -> usize {
        self.upcoming
            .iter()
            .rposition(|entry| entry.kind != QueueKind::Suggested)
            .map_or(0, |at| at + 1)
    }

    /// Moves a song to `gap`. Landing inside or right after the hand-queued run joins it and
    /// leaving that run leaves it, and a suggestion dragged ahead of the queue's end becomes part
    /// of the queue, so the next radio refill keeps it.
    pub fn move_upcoming_to_gap(&mut self, from: usize, gap: usize) {
        // only a suggestion moves among the suggestions, anything else stops at the queue's end
        let gap = match self.upcoming.get(from).map(|entry| entry.kind) {
            Some(QueueKind::Suggested) | None => gap,
            Some(_) => gap.min(self.context_end()),
        };
        let to = gap_target(from, gap, self.upcoming.len());
        if !move_item(&mut self.upcoming, from, to) {
            return;
        }
        let others = self
            .upcoming
            .iter()
            .enumerate()
            .filter(|(at, _)| *at != to)
            .map(|(_, entry)| entry);
        let joins = joins_manual(others, to);
        let in_body = self
            .upcoming
            .iter()
            .skip(to + 1)
            .any(|entry| entry.kind != QueueKind::Suggested);
        if let Some(entry) = self.upcoming.get_mut(to) {
            entry.kind = match (joins, entry.kind) {
                (true, _) => QueueKind::Queued,
                (false, QueueKind::Queued) => QueueKind::Context,
                (false, QueueKind::Suggested) if in_body => QueueKind::Context,
                (false, kind) => kind,
            };
            // a hand-queued song is no longer the collection's, or a reshuffle would bring it
            // back from the collection a second time
            if entry.kind == QueueKind::Queued {
                let uid = entry.uid.clone();
                self.source.retain(|kept| kept.uid != uid);
            }
        }
        self.changed();
    }

    /// Removes an upcoming song or suggestion and answers it with its position, for Undo.
    pub fn remove(&mut self, position: QueuePosition) -> Option<Removed<O>> {
        let QueuePosition::Upcoming(index) = position else {
            return None;
        };
        let entry = self.upcoming.remove(index)?;
        self.changed();
        Some(Removed {
            entry,
            index,
            batch: self.suggested,
        })
    }

    /// Restores removals newest first, clamping positions to today's queue. Occurrences already
    /// played and recommendations from a replaced batch are not restored.
    pub fn put_back(&mut self, removed: Vec<Removed<O>>) {
        // A reset or shuffle may have restored the same occurrence already. Identities
        // distinguish those from separate additions of the same song.
        let restoring: HashSet<_> = removed.iter().map(|taken| &taken.entry.uid).collect();
        self.upcoming
            .retain(|entry| !restoring.contains(&entry.uid));
        for Removed {
            entry,
            index,
            batch,
        } in removed.into_iter().rev()
        {
            if (entry.kind == QueueKind::Suggested && batch != self.suggested)
                || self.current_uid() == Some(&entry.uid)
                || self.past.iter().any(|played| played.uid == entry.uid)
            {
                continue;
            }
            self.upcoming.insert(index.min(self.upcoming.len()), entry);
        }
        self.changed();
    }

    /// Takes the song that plays next, in queue order or, with shuffle on, one at random from
    /// what is left. Also says whether it came from the radio suggestions.
    pub fn next(&mut self) -> Option<(Track, bool)> {
        let next = take_next(&mut self.upcoming, self.shuffle)?;
        let suggested = next.kind == QueueKind::Suggested;
        let track = next.track.clone();
        if let Some(played) = self.current.replace(next) {
            self.past.push(played);
        }
        self.changed();
        Some(((*track).clone(), suggested))
    }

    /// Starts the queue over from its first song, for repeat all. What was queued by hand joins
    /// the collection it played in, so the next loop has no stale hand-queued run.
    pub fn rewind(&mut self) -> Option<Track> {
        self.drop_suggested();
        let mut entries = rewind_entries(
            std::mem::take(&mut self.past),
            self.current.take(),
            std::mem::take(&mut self.upcoming),
        );
        for entry in &mut entries {
            entry.kind = QueueKind::Context;
        }
        let start = match self.shuffle && !entries.is_empty() {
            true => fastrand::usize(..entries.len()),
            false => 0,
        };
        self.fill(entries, start)
    }

    /// Random play wraps in the collection's own order: a random song starts the new loop, but
    /// the list itself is never rearranged by the order songs happened to play in.
    pub fn rewind_ordered(&mut self) -> Option<Track> {
        self.drop_suggested();
        let mut entries = ordered_entries(
            &self.source,
            std::mem::take(&mut self.past),
            self.current.take(),
            std::mem::take(&mut self.upcoming),
        );
        for entry in &mut entries {
            entry.kind = QueueKind::Context;
        }
        let start = match self.shuffle && !entries.is_empty() {
            true => fastrand::usize(..entries.len()),
            false => 0,
        };
        self.fill(entries, start)
    }

    /// Empties the play order, history included.
    pub fn clear_all(&mut self) {
        self.clear_contents();
        self.changed();
    }

    /// Takes a random song other than the current one for the previous button, from the history
    /// or what is left. `None` when nothing else can play.
    pub fn previous_random(&mut self) -> Option<Track> {
        let entry = take_previous_random(&mut self.past, &mut self.upcoming)?;
        if let Some(played) = self.current.replace(entry) {
            self.past.push(played);
        }
        self.changed();
        self.current().cloned()
    }

    pub fn previous(&mut self) -> Option<Track> {
        if let Some(index) = self.past.iter().rposition(|entry| entry.track.playable) {
            return self.select(QueuePosition::Past(index));
        }
        let last = self.upcoming.len().checked_sub(1)?;
        self.select(QueuePosition::Upcoming(last))
    }

    /// Selects a song in play order. Replaying history queues the interrupted songs first;
    /// selecting the collection preserves manual additions, while selecting a suggestion skips them.
    pub fn select(&mut self, position: QueuePosition) -> Option<Track> {
        match position {
            QueuePosition::Past(index) => {
                if !select_past(&mut self.past, &mut self.current, &mut self.upcoming, index) {
                    return None;
                }
            }
            QueuePosition::Upcoming(index) => {
                let preserve = self.upcoming.get(index)?.kind == QueueKind::Context;
                select_upcoming(
                    &mut self.past,
                    &mut self.current,
                    &mut self.upcoming,
                    index,
                    |entry| preserve && entry.kind == QueueKind::Queued,
                );
            }
            _ => return None,
        }
        self.changed();
        self.current().cloned()
    }
}

/// Whether a song put at `at` joins the hand-queued run in `upcoming`: inside it or right after
/// it, or at the very top when there is no run.
fn joins_manual<'a, O: 'a>(upcoming: impl Iterator<Item = &'a Entry<O>>, at: usize) -> bool {
    let run = upcoming
        .enumerate()
        .filter(|(_, entry)| entry.kind == QueueKind::Queued)
        .fold(None, |run, (index, _)| match run {
            None => Some((index, index)),
            Some((first, _)) => Some((first, index)),
        });
    match run {
        Some((first, last)) => (first..=last + 1).contains(&at),
        None => at == 0,
    }
}

/// Makes a fresh identity for a newly queued occurrence, never for a change of position.
fn queue_uid() -> String {
    format!("wisp-{:016x}", fastrand::u64(..))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::time::Duration;

    use music::{ArtistRef, Track};

    use super::{
        Entry, gap_target, hydrate, in_order, local, move_item, ordered_entries, record,
        rewind_entries, select_past, select_upcoming, sift, stub, take_next, take_previous_random,
        trim, unique_played,
    };

    fn track(id: &str) -> Track {
        Track {
            id: Some(id.to_owned()),
            name: id.to_owned(),
            playable: true,
            duration: Duration::from_secs(180),
            ..Default::default()
        }
    }

    fn entry(id: &str) -> Entry<()> {
        Entry {
            uid: format!("test-{id}"),
            track: Arc::new(track(id)),
            origin: None,
            kind: super::QueueKind::Context,
        }
    }

    fn ids(entries: &VecDeque<Entry<()>>) -> Vec<String> {
        entries
            .iter()
            .map(|entry| entry.track.name.clone())
            .collect::<Vec<_>>()
    }

    fn listing(count: usize) -> Vec<Entry<()>> {
        (0..count).map(|index| entry(&index.to_string())).collect()
    }

    #[test]
    fn the_history_keeps_only_the_latest_play_of_an_occurrence() {
        let mut past = vec![entry("a"), entry("b"), entry("a")];
        unique_played(&mut past);
        assert_eq!(ids(&VecDeque::from(past.clone())), ["b", "a"]);
    }

    #[test]
    fn rewinding_a_reshuffled_queue_keeps_each_occurrence_once() {
        // A replayed occurrence can sit in the history and the order at once; the wrap must
        // keep it once.
        let source = listing(3);
        let played = source[0].clone();
        let past = vec![played.clone()];
        let upcoming: VecDeque<Entry<()>> = source.iter().cloned().collect();

        let merged = rewind_entries(past, None, upcoming);
        assert_eq!(
            merged
                .iter()
                .filter(|entry| entry.uid == played.uid)
                .count(),
            1
        );
        assert_eq!(merged.len(), source.len());
    }

    #[test]
    fn ordering_next_takes_the_front_of_the_queue() {
        let mut upcoming: VecDeque<Entry<()>> = listing(3).into();
        assert_eq!(take_next(&mut upcoming, false).unwrap().track.name, "0");
        assert_eq!(ids(&upcoming), ["1", "2"]);
    }

    #[test]
    fn shuffled_next_takes_one_of_what_is_left() {
        let mut upcoming: VecDeque<Entry<()>> = listing(5).into();
        for expected in (1..=5).rev() {
            let before = ids(&upcoming);
            let taken = take_next(&mut upcoming, true).expect("a track to take");
            assert_eq!(upcoming.len() + 1, expected);
            assert!(before.contains(&taken.track.name));
        }
        assert!(take_next(&mut upcoming, true).is_none());
    }

    #[test]
    fn shuffled_previous_takes_from_history_and_what_is_left() {
        let mut past = vec![entry("a")];
        let mut upcoming: VecDeque<Entry<()>> = listing(2).into();
        let mut seen = Vec::new();
        while let Some(entry) = take_previous_random(&mut past, &mut upcoming) {
            seen.push(entry.track.name.clone());
        }
        seen.sort();
        assert_eq!(seen, ["0", "1", "a"]);
    }

    #[test]
    fn a_random_loop_keeps_the_collection_order() {
        let source = listing(3);
        let past = vec![source[0].clone()];
        let current = Some(source[1].clone());
        let upcoming: VecDeque<Entry<()>> = vec![entry("queued")].into();

        let entries = ordered_entries(&source, past, current, upcoming);
        let names: Vec<_> = entries
            .into_iter()
            .map(|entry| entry.track.name.clone())
            .collect();
        assert_eq!(names, ["0", "1", "2", "queued"]);
    }

    #[test]
    fn shuffled_previous_works_on_the_first_song() {
        let mut past = Vec::new();
        let mut upcoming: VecDeque<Entry<()>> = listing(2).into();
        assert!(take_previous_random(&mut past, &mut upcoming).is_some());
    }

    #[test]
    fn spots_a_reordered_queue() {
        let source = [1, 2, 3, 4];

        assert!(in_order([1, 2, 3, 4].iter(), &source));
        assert!(!in_order([1, 3, 2, 4].iter(), &source));
    }

    #[test]
    fn moves_items_in_both_directions() {
        let mut items = VecDeque::from([1, 2, 3, 4]);

        assert!(move_item(&mut items, 0, 2));
        assert_eq!(items, [2, 3, 1, 4]);
        assert!(move_item(&mut items, 3, 1));
        assert_eq!(items, [2, 4, 3, 1]);
    }

    #[test]
    fn ignores_invalid_moves() {
        let mut items = VecDeque::from([1, 2, 3]);

        assert!(!move_item(&mut items, 1, 1));
        assert!(!move_item(&mut items, 3, 0));
        assert!(!move_item(&mut items, 0, 3));
        assert_eq!(items, [1, 2, 3]);
    }

    #[test]
    fn selecting_past_rebuilds_the_queue_from_that_track() {
        let mut past = vec![entry("1"), entry("2"), entry("3")];
        let mut current = Some(entry("4"));
        let mut queued = entry("5");
        queued.kind = super::QueueKind::Queued;
        let mut upcoming = VecDeque::from([queued, entry("6")]);

        assert!(select_past(&mut past, &mut current, &mut upcoming, 1));
        assert_eq!(past, [entry("1")]);
        assert_eq!(current, Some(entry("2")));
        assert_eq!(ids(&upcoming), ["3", "4", "5", "6"]);
        assert_eq!(
            upcoming.iter().map(|entry| entry.kind).collect::<Vec<_>>(),
            [
                super::QueueKind::Context,
                super::QueueKind::Context,
                super::QueueKind::Queued,
                super::QueueKind::Context
            ]
        );
    }

    #[test]
    fn selecting_invalid_past_track_keeps_the_queue() {
        let mut past = vec![1, 2];
        let mut current = Some(3);
        let mut upcoming = VecDeque::from([4, 5]);

        assert!(!select_past(&mut past, &mut current, &mut upcoming, 2));
        assert_eq!(past, [1, 2]);
        assert_eq!(current, Some(3));
        assert_eq!(upcoming, [4, 5]);
    }

    #[test]
    fn selecting_upcoming_moves_skipped_tracks_to_the_past() {
        let mut past = vec![1];
        let mut current = Some(2);
        let mut upcoming = VecDeque::from([3, 4, 5]);

        assert!(select_upcoming(
            &mut past,
            &mut current,
            &mut upcoming,
            1,
            |_| false
        ));
        assert_eq!(past, [1, 2, 3]);
        assert_eq!(current, Some(4));
        assert_eq!(upcoming, [5]);

        // Selecting collection playback keeps explicit additions available afterwards.
        let mut past = vec![1];
        let mut current = Some(2);
        let mut upcoming = VecDeque::from([3, 4, 5, 6]);
        assert!(select_upcoming(
            &mut past,
            &mut current,
            &mut upcoming,
            2,
            |entry| *entry == 3
        ));
        assert_eq!(past, [1, 2, 4]);
        assert_eq!(current, Some(5));
        assert_eq!(upcoming, [3, 6]);
    }

    #[test]
    fn selecting_invalid_upcoming_track_keeps_the_queue() {
        let mut past = vec![1];
        let mut current = Some(2);
        let mut upcoming = VecDeque::from([3, 4]);

        assert!(!select_upcoming(
            &mut past,
            &mut current,
            &mut upcoming,
            2,
            |_| false
        ));
        assert_eq!(past, [1]);
        assert_eq!(current, Some(2));
        assert_eq!(upcoming, [3, 4]);
    }

    #[test]
    fn converts_gaps_to_insertion_indices() {
        assert_eq!(gap_target(0, 3, 4), 2);
        assert_eq!(gap_target(0, 4, 4), 3);
        assert_eq!(gap_target(3, 1, 4), 1);
        assert_eq!(gap_target(2, 0, 4), 0);
        assert_eq!(gap_target(1, 1, 4), 1);
        assert_eq!(gap_target(1, 2, 4), 1);
        assert_eq!(gap_target(0, 10, 4), 3);
    }

    #[test]
    fn gap_moves_match_visual_positions() {
        let mut items = VecDeque::from([1, 2, 3, 4]);

        let to = gap_target(0, 3, items.len());
        assert!(move_item(&mut items, 0, to));
        assert_eq!(items, [2, 3, 1, 4]);

        let to = gap_target(3, 0, items.len());
        assert!(move_item(&mut items, 3, to));
        assert_eq!(items, [4, 2, 3, 1]);

        let to = gap_target(1, 2, items.len());
        assert!(!move_item(&mut items, 1, to));
        assert_eq!(items, [4, 2, 3, 1]);
    }

    #[test]
    fn a_saved_track_keeps_what_the_player_bar_shows() {
        let mut source = track("abc");
        source.artists = "One, Two".to_owned();
        source.artist_refs = vec![
            ArtistRef {
                name: "One".to_owned(),
                id: Some("a1".to_owned()),
                channel: false,
            },
            ArtistRef {
                name: "Two".to_owned(),
                id: None,
                channel: false,
            },
        ];
        source.album = "Album".to_owned();
        source.album_id = Some("al".to_owned());
        source.cover = Some("https://cover".to_owned());
        source.explicit = true;

        let saved = stub(&source).expect("a track with an id");
        let json = serde_json::to_string(&saved).expect("a serializable stub");
        let restored = hydrate(serde_json::from_str(&json).expect("a readable stub"));

        assert_eq!(restored.id, source.id);
        assert_eq!(restored.name, source.name);
        assert_eq!(restored.artists, source.artists);
        assert_eq!(restored.artist_refs, source.artist_refs);
        assert_eq!(restored.album, source.album);
        assert_eq!(restored.album_id, source.album_id);
        assert_eq!(restored.cover, source.cover);
        assert_eq!(restored.duration, source.duration);
        assert!(restored.explicit);
        assert!(restored.playable);
    }

    #[test]
    fn a_track_without_an_id_is_never_saved() {
        let mut orphan = track("abc");
        orphan.id = None;

        assert!(stub(&orphan).is_none());
    }

    #[test]
    fn a_record_caps_the_history_and_the_queue() {
        let past = listing(60);
        let mut upcoming = listing(300);
        upcoming[1].kind = super::QueueKind::Queued;
        let current = entry("now");

        let resume = record("netease", &past, Some(&current), upcoming.iter());

        assert_eq!(resume.provider, "netease");
        assert_eq!(resume.position, 0.);
        assert_eq!(resume.current.map(|stub| stub.id), Some("now".to_owned()));
        assert_eq!(resume.past.len(), 20);
        assert_eq!(resume.past.first().map(|stub| stub.id.as_str()), Some("40"));
        assert_eq!(resume.upcoming.len(), 200);
        assert_eq!(resume.upcoming[0].kind, super::QueueKind::Context);
        assert_eq!(resume.upcoming[1].kind, super::QueueKind::Queued);
        assert_eq!(resume.manual, 0);
        assert_eq!(
            resume.upcoming.first().map(|stub| stub.id.as_str()),
            Some("0")
        );
    }

    #[test]
    fn sifting_keeps_only_what_is_wanted() {
        let mut past = vec![1, 2, 3];
        let mut current = Some(4);
        let mut upcoming = VecDeque::from([5, 6]);
        let mut source = vec![1, 2, 3, 4, 5, 6];

        assert!(sift(
            &mut past,
            &mut current,
            &mut upcoming,
            &mut source,
            |item| item % 2 == 0
        ));
        assert_eq!(past, [2]);
        assert_eq!(current, Some(4));
        assert_eq!(upcoming, [6]);
        assert_eq!(source, [2, 4, 6]);
    }

    #[test]
    fn sifting_reports_an_untouched_queue() {
        let mut past = vec![1];
        let mut current = Some(2);
        let mut upcoming = VecDeque::from([3]);
        let mut source = vec![1, 2, 3];

        assert!(!sift(
            &mut past,
            &mut current,
            &mut upcoming,
            &mut source,
            |_| true
        ));
        assert_eq!(past, [1]);
        assert_eq!(current, Some(2));
        assert_eq!(upcoming, [3]);
    }

    #[test]
    fn signing_out_leaves_only_imported_tracks() {
        let imported = format!("{}song.flac", music::LOCAL_TRACK_PREFIX);
        let mut past = vec![track("streamed"), track(&imported)];
        let mut current = Some(track("playing"));
        let mut upcoming = VecDeque::from([track("next"), track(&imported)]);
        let mut source = vec![track("streamed"), track(&imported)];

        assert!(sift(
            &mut past,
            &mut current,
            &mut upcoming,
            &mut source,
            local
        ));
        assert_eq!(past.len(), 1);
        assert!(current.is_none());
        assert_eq!(upcoming.len(), 1);
        assert_eq!(source.len(), 1);
        assert!(past.iter().chain(&source).all(local));
    }

    #[test]
    fn an_imported_track_outlives_the_session() {
        let imported = format!("{}song.flac", music::LOCAL_TRACK_PREFIX);
        let mut past = Vec::new();
        let mut current = Some(track(&imported));
        let mut upcoming = VecDeque::new();
        let mut source = Vec::new();

        assert!(!sift(
            &mut past,
            &mut current,
            &mut upcoming,
            &mut source,
            local
        ));
        assert!(current.is_some());
    }

    #[test]
    fn shuffle_changes_the_pick_not_the_order() {
        let mut order = super::Order::new(false);
        order.start(vec![track("a"), track("b"), track("c")], 0, None::<()>);
        order.set_shuffle(true);
        let upcoming: Vec<_> = order.upcoming().map(|track| track.name.clone()).collect();
        assert_eq!(upcoming, ["b", "c"]);
        order.set_shuffle(false);
        let upcoming: Vec<_> = order.upcoming().map(|track| track.name.clone()).collect();
        assert_eq!(upcoming, ["b", "c"]);
    }

    #[test]
    fn finishing_clears_the_current_song_and_rewinds_to_the_first() {
        let mut order = super::Order::new(false);
        order.start(vec![track("a"), track("b")], 0, None::<()>);
        assert_eq!(
            order.next().map(|(track, _)| track.name.clone()),
            Some("b".to_owned())
        );
        order.finish();
        assert!(order.current().is_none());
        assert!(order.has_previous());
        assert!(!order.reordered());
        assert_eq!(
            order.rewind().map(|track| track.name.clone()),
            Some("a".to_owned())
        );
    }

    #[test]
    fn switching_providers_keeps_each_order() {
        let mut order = super::Order::new(false);
        order.start(vec![track("a"), track("b")], 0, Some("p1"));
        order.ensure_provider(Some("p1".to_owned()));
        order.switch_provider(Some("p2".to_owned()));
        assert!(order.is_empty());
        order.start(vec![track("c")], 0, Some("p2"));
        order.ensure_provider(Some("p2".to_owned()));
        order.switch_provider(Some("p1".to_owned()));
        assert_eq!(order.current().map(|track| track.name.as_str()), Some("a"));
        assert_eq!(order.ahead(), 1);
    }

    #[test]
    fn ahead_counts_only_playable_songs() {
        let mut order = super::Order::new(false);
        let mut tail = track("tail");
        tail.playable = false;
        order.start(vec![track("head"), tail], 0, None::<()>);
        assert_eq!(order.ahead(), 0);
    }

    #[test]
    fn previous_on_the_first_song_wraps_to_the_tail() {
        let mut order = super::Order::new(false);
        order.start(vec![track("a"), track("b"), track("c")], 0, None::<()>);
        assert_eq!(
            order.previous().map(|track| track.name.clone()),
            Some("c".to_owned())
        );
    }

    #[test]
    fn next_reports_when_it_came_from_the_suggestions() {
        let mut order = super::Order::new(false);
        order.start(vec![track("a"), track("b")], 0, None::<()>);
        order.suggest(vec![track("s")], ());
        assert_eq!(
            order.next().map(|(track, _)| track.name.clone()),
            Some("b".to_owned())
        );
        assert_eq!(order.next().map(|(_, suggested)| suggested), Some(true));
    }

    #[test]
    fn trimming_drops_the_oldest_history() {
        let mut past = vec![1, 2, 3, 4, 5];

        trim(&mut past, 3);
        assert_eq!(past, [3, 4, 5]);

        trim(&mut past, 3);
        assert_eq!(past, [3, 4, 5]);

        trim(&mut past, 0);
        assert!(past.is_empty());
    }
}
