//! The active queue exposed to consumers.

use gpui::{App, Entity};
use logic::queue::{QueuePosition, QueueSections};
use music::Track;

use crate::{Origin, Queue, QueueKind};

/// A cheap handle to the active play order. Only requested tracks are copied from the queue,
/// never the whole playlist.
#[derive(Clone)]
pub struct QueueView {
    queue: Entity<Queue>,
    pub sections: QueueSections,
    pub playing: bool,
}

impl QueueView {
    pub(crate) fn new(queue: Entity<Queue>, playing: bool, cx: &App) -> Self {
        let sections = queue.read(cx).sections.clone();
        Self {
            queue,
            sections,
            playing,
        }
    }

    pub fn editable(&self) -> bool {
        true
    }

    pub fn revision(&self, cx: &App) -> u64 {
        self.queue.read(cx).revision()
    }

    pub fn origin(&self, cx: &App) -> Option<Origin> {
        self.queue.read(cx).origin().cloned()
    }

    /// Why the track at `position` is in the queue.
    pub fn kind(&self, position: QueuePosition, cx: &App) -> Option<QueueKind> {
        self.queue.read(cx).kind(position)
    }

    pub fn uid(&self, position: QueuePosition, cx: &App) -> Option<String> {
        self.queue.read(cx).uid(position).map(str::to_owned)
    }

    pub fn track(&self, position: QueuePosition, cx: &App) -> Option<Track> {
        let queue = self.queue.read(cx);
        match position {
            QueuePosition::Past(at) => queue.past().nth(at),
            QueuePosition::Current => queue.current(),
            QueuePosition::Upcoming(at) => queue.upcoming().nth(at),
        }
        .cloned()
    }
}
