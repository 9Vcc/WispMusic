use std::cell::RefCell;
use std::collections::HashSet;
use std::ops::Deref;
use std::rc::{Rc, Weak};

use gpui::{App, Context, Entity};
use logic::queue::{Order, Removed};
use music::Track;

use crate::{AppSettings, Origin, Outcome, Session, SessionEvent, Toast, Toasts};

mod view;
pub(crate) use logic::queue::gap_target;
pub use logic::queue::{QueueKind, QueuePlacement, QueuePosition, QueueSections, QueueSlot, Stub};
pub use view::QueueView;

/// A queue as it is saved for the next launch.
pub type Resume = logic::queue::Resume<Origin>;

/// A track removed from the queue on its own, and where it sat.
type Taken = Removed<Origin>;

/// What an Undo on a queue toast puts back. The toast's handler holds the only strong
/// reference, so the tracks are let go once the toast is, and taking it empties it.
type Snapshot<T> = Rc<RefCell<Option<T>>>;

/// A toast the queue posted with an Undo, and a weak handle on what that Undo puts back.
struct Reported<T> {
    toast: usize,
    undo: Weak<RefCell<Option<T>>>,
}

impl<T> Reported<T> {
    /// What the toast's Undo puts back, while the toast is still up and its Undo unused, so a
    /// change made meanwhile can join it.
    fn live(&self, cx: &mut App) -> Option<Snapshot<T>> {
        let shown = Toasts::entity(cx)
            .read(cx)
            .shown()
            .iter()
            .any(|toast| toast.id == self.toast);
        let snapshot = self.undo.upgrade().filter(|_| shown)?;
        let unused = snapshot.borrow().is_some();
        unused.then_some(snapshot)
    }
}

/// The play order as the app holds it. [`Order`] decides what plays when, and this entity adds
/// what an edit owes the rest of the app: a redraw, the queue saved for the next launch and the
/// Undo toasts. Reading goes straight through to the order.
pub struct Queue {
    order: Order<Origin>,
    removed: Option<Reported<Vec<Taken>>>,
    session: Entity<Session>,
    settings: Entity<AppSettings>,
}

impl Deref for Queue {
    type Target = Order<Origin>;

    fn deref(&self) -> &Self::Target {
        &self.order
    }
}

impl Queue {
    pub fn new(
        session: Entity<Session>,
        settings: Entity<AppSettings>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(&session, |this, _, event, cx| match event {
            SessionEvent::SignedOut => this.switch_provider(None, cx),
            SessionEvent::SignedIn => this.sync_provider(cx),
            SessionEvent::Reconnected | SessionEvent::WentOffline | SessionEvent::LocalChanged => {}
        })
        .detach();

        let shuffle = settings.read(cx).shuffle();

        Self {
            order: Order::new(shuffle),
            removed: None,
            session,
            settings,
        }
    }

    /// Runs `edit` on the order. When it changed anything, the queue adopts the current
    /// provider, is saved and is redrawn.
    fn edit<T>(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Order<Origin>) -> T) -> T {
        let before = self.order.revision();
        let answer = edit(&mut self.order);
        let provider = self.session.read(cx).provider_slug().map(str::to_owned);
        self.order.ensure_provider(provider);
        if self.order.revision() != before {
            self.remember(cx);
            cx.notify();
        }
        answer
    }

    fn remember(&mut self, cx: &mut Context<Self>) {
        let resume = self
            .order
            .provider()
            .and_then(|provider| self.order.record(provider));
        self.settings
            .update(cx, |settings, cx| settings.set_resume(resume, cx));
    }

    pub fn set_shuffle(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.order.shuffle() == on {
            return;
        }
        self.settings
            .update(cx, |settings, cx| settings.set_shuffle(on, cx));
        self.edit(cx, |order| order.set_shuffle(on));
    }

    pub(crate) fn force_shuffle(&mut self, on: bool, cx: &mut Context<Self>) {
        self.edit(cx, |order| order.force_shuffle(on));
    }

    pub(crate) fn sync_provider(&mut self, cx: &mut Context<Self>) {
        let next = self.session.read(cx).provider_slug().map(str::to_owned);
        self.switch_provider(next, cx);
    }

    fn switch_provider(&mut self, next: Option<String>, cx: &mut Context<Self>) {
        if self.order.provider() != next.as_deref() {
            self.removed = None;
        }
        let before = self.order.revision();
        let was_empty = self.order.is_empty();
        self.order.switch_provider(next);
        if self.order.revision() != before {
            // Switching an empty order is startup (or a sign-out already without a queue):
            // saving here would write `resume: None` and erase the queue the last run left for
            // the provider before playback has a chance to adopt it.
            if !was_empty {
                self.remember(cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn revive(&mut self, resume: Resume, cx: &mut Context<Self>) -> Option<Track> {
        self.edit(cx, |order| order.revive(resume))
    }

    pub fn replace_current(&mut self, track: &Track, cx: &mut Context<Self>) {
        self.edit(cx, |order| order.replace_current(track));
    }

    pub fn suggest(&mut self, tracks: Vec<Track>, origin: Origin, cx: &mut Context<Self>) {
        self.edit(cx, |order| order.suggest(tracks, origin));
    }

    pub fn extend_similar(&mut self, tracks: Vec<Track>, origin: Origin, cx: &mut Context<Self>) {
        self.edit(cx, |order| order.extend_similar(tracks, origin));
    }

    pub fn clear_similar(&mut self, cx: &mut Context<Self>) {
        self.edit(cx, |order| order.clear_similar());
    }

    pub fn remove_tracks(&mut self, ids: &[String], cx: &mut Context<Self>) -> bool {
        self.edit(cx, |order| order.remove_tracks(ids))
    }

    /// The current track and what is still ahead. FM refills only need to avoid replaying this
    /// window; older history is allowed back so a recommendation pool can keep moving.
    pub fn ahead_ids(&self) -> HashSet<String> {
        self.current()
            .into_iter()
            .chain(self.upcoming())
            .filter_map(|track| track.id.clone())
            .collect()
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) -> Option<Track> {
        self.edit(cx, Order::reset)
    }

    pub fn start(
        &mut self,
        tracks: Vec<Track>,
        index: usize,
        origin: Option<Origin>,
        cx: &mut Context<Self>,
    ) -> Option<Track> {
        self.edit(cx, |order| order.start(tracks, index, origin))
    }

    pub fn insert(
        &mut self,
        tracks: impl IntoIterator<Item = Track>,
        origin: Option<Origin>,
        placement: QueuePlacement,
        cx: &mut Context<Self>,
    ) {
        self.edit(cx, |order| order.insert(tracks, origin, placement));
    }

    pub(crate) fn extend_context(
        &mut self,
        tracks: Vec<Track>,
        origin: Origin,
        cx: &mut Context<Self>,
    ) {
        self.edit(cx, |order| order.extend_context(tracks, origin));
    }

    pub fn move_upcoming_to_gap(&mut self, from: usize, gap: usize, cx: &mut Context<Self>) {
        self.edit(cx, |order| order.move_upcoming_to_gap(from, gap));
    }

    pub fn remove(&mut self, position: QueuePosition, cx: &mut Context<Self>) {
        let Some(taken) = self.edit(cx, |order| order.remove(position)) else {
            return;
        };
        self.report_removal(taken, cx);
    }

    fn report_removal(&mut self, taken: Taken, cx: &mut Context<Self>) {
        let live = self
            .removed
            .as_ref()
            .and_then(|reported| Some((reported.toast, reported.live(cx)?)));
        match live {
            Some((id, snapshot)) => {
                if let Some(removed) = snapshot.borrow_mut().as_mut() {
                    removed.push(taken);
                }
                let toast = Self::removal_toast(snapshot, cx);
                Toasts::settle(id, toast, cx);
            }
            None => {
                let snapshot = Rc::new(RefCell::new(Some(vec![taken])));
                let undo = Rc::downgrade(&snapshot);
                let toast = Self::removal_toast(snapshot, cx);
                let toast = Toasts::post(toast, cx);
                self.removed = Some(Reported { toast, undo });
            }
        }
    }

    fn removal_toast(snapshot: Snapshot<Vec<Taken>>, cx: &mut Context<Self>) -> Toast {
        let toast = match snapshot.borrow().as_deref() {
            Some([only]) => {
                Toast::new(Outcome::Plain, "toast-queue-removed-track").name(only.name().to_owned())
            }
            removed => Toast::new(Outcome::Plain, "toast-queue-removed-tracks")
                .count(removed.map_or(0, <[Taken]>::len)),
        };
        Self::undo_toast(toast, snapshot, cx)
    }

    fn undo_toast(toast: Toast, snapshot: Snapshot<Vec<Taken>>, cx: &mut Context<Self>) -> Toast {
        let queue = cx.entity().downgrade();
        toast.undo(move |cx| {
            let Some(removed) = snapshot.borrow_mut().take() else {
                return;
            };
            queue
                .update(cx, |queue, cx| queue.put_back(removed, cx))
                .ok();
        })
    }

    fn put_back(&mut self, removed: Vec<Taken>, cx: &mut Context<Self>) {
        self.edit(cx, |order| order.put_back(removed));
    }

    pub fn next(&mut self, cx: &mut Context<Self>) -> Option<(Track, bool)> {
        self.edit(cx, Order::next)
    }

    /// Puts the last song in history when the queue runs out, leaving nothing as now playing.
    pub fn finish(&mut self, cx: &mut Context<Self>) {
        self.edit(cx, Order::finish);
    }

    pub fn rewind(&mut self, cx: &mut Context<Self>) -> Option<Track> {
        self.edit(cx, Order::rewind)
    }

    pub fn rewind_ordered(&mut self, cx: &mut Context<Self>) -> Option<Track> {
        self.edit(cx, Order::rewind_ordered)
    }

    pub fn clear_all(&mut self, cx: &mut Context<Self>) {
        self.edit(cx, Order::clear_all);
    }

    pub fn previous_random(&mut self, cx: &mut Context<Self>) -> Option<Track> {
        self.edit(cx, Order::previous_random)
    }

    pub fn previous(&mut self, cx: &mut Context<Self>) -> Option<Track> {
        self.edit(cx, Order::previous)
    }

    pub fn select(&mut self, position: QueuePosition, cx: &mut Context<Self>) -> Option<Track> {
        self.edit(cx, |order| order.select(position))
    }
}
