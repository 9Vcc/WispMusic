mod accounts;
mod cookie;
mod password;
mod phone;
mod qr;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    App, Context, Div, ElementId, Entity, EntityId, FocusHandle, Pixels, ScrollHandle, Window, div,
    px,
};
use ui::{Input, Menu, Picker, Scrollbar, SelectNext, SelectPrevious, Submit};

pub(crate) use accounts::AccountPicker;
pub(crate) use cookie::CookiePrompt;
pub(crate) use password::PasswordPrompt;
pub(crate) use phone::PhonePrompt;
pub(crate) use qr::QrPrompt;

const SEARCH_HEIGHT: Pixels = px(320.);
const SELECTED_LEAD: usize = 2;

#[derive(Clone)]
pub(crate) struct SearchPopup {
    input: Entity<Input>,
    scrollbar: Entity<Scrollbar>,
    cursor: Rc<Cell<usize>>,
    query: Rc<RefCell<String>>,
    open: Rc<Cell<bool>>,
    restore: FocusHandle,
}

impl SearchPopup {
    /// `restore` is the handle the popup hands focus back to when it closes, which the
    /// owning view has to track on an element it always draws.
    pub(crate) fn new(
        hint: &'static str,
        watcher: EntityId,
        restore: FocusHandle,
        cx: &mut App,
    ) -> Self {
        Self {
            input: cx.new(|cx| Input::new(hint, cx).compact().tucked()),
            scrollbar: cx.new(|_| Scrollbar::inset().watching(watcher)),
            cursor: Rc::new(Cell::new(0)),
            query: Rc::new(RefCell::new(String::new())),
            open: Rc::new(Cell::new(false)),
            restore,
        }
    }

    pub(crate) fn input(&self) -> Entity<Input> {
        self.input.clone()
    }

    pub(crate) fn query(&self) -> String {
        self.query.borrow().clone()
    }

    pub(crate) fn changed(&self, cx: &App) {
        let query = self.input.read(cx).text().trim().to_lowercase();
        if *self.query.borrow() == query {
            return;
        }
        *self.query.borrow_mut() = query;
        self.cursor.set(0);
        self.scrollbar.read(cx).scroll().scroll_to_item(0);
    }

    /// Follows the popover's open state: the search input takes focus while the popup
    /// is open and gives it back to `restore` on close. Without that hand back focus
    /// stays on an input that is no longer drawn, which leaves the dispatch path empty,
    /// so an action raised by a later click reaches no handler at all.
    ///
    /// Both directions hand the focus over through `Window::defer` rather than doing it
    /// on the spot. `sync` runs inside `Render`, which gpui calls while building the
    /// frame, and a `track_focus` element reports its own focus to the accessibility tree
    /// as it prepaints — parents before children. Changing the window's focus here would
    /// therefore make one node report for the focus it had at the top of the frame and a
    /// later one report for the focus it has now, and `A11y::set_focus` panics on the
    /// second call in a frame ("set_focus called more than once in a single frame").
    /// That crashed the app when the font picker was opened or a typeface chosen.
    pub(crate) fn sync(
        &self,
        open: bool,
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.open.replace(open) == open {
            return;
        }
        match open {
            true => self.place(selected, cx),
            false => self.input.update(cx, |input, cx| input.set_text("", cx)),
        }
        // Rendering can run after an earlier node claimed focus for this frame.
        let popup = self.clone();
        window.defer(cx, move |window, cx| {
            if popup.open.get() != open {
                return;
            }
            match open {
                true => popup.input.update(cx, |input, cx| input.focus(window, cx)),
                false => window.focus(&popup.restore, cx),
            }
        });
    }

    /// Moves the cursor to `selected` and scrolls it into view, for when the
    /// list changes under an open popup; `None` goes back to the top.
    pub(crate) fn place(&self, selected: Option<usize>, cx: &App) {
        let selected = selected.unwrap_or_default();
        self.cursor.set(selected);
        self.scrollbar
            .read(cx)
            .scroll()
            .scroll_to_item(selected.saturating_sub(SELECTED_LEAD));
    }

    pub(crate) fn cursor(&self, count: usize) -> usize {
        self.cursor.get().min(count.saturating_sub(1))
    }

    pub(crate) fn scroll(&self, cx: &App) -> ScrollHandle {
        self.scrollbar.read(cx).scroll().clone()
    }

    pub(crate) fn height(&self) -> Pixels {
        SEARCH_HEIGHT
    }

    pub(crate) fn menu(&self, id: impl Into<ElementId>, width: Pixels) -> Menu {
        Menu::new(id)
            .w(width)
            .max_h(SEARCH_HEIGHT)
            .scrollbar(self.scrollbar.clone())
            .header(self.input.clone())
    }

    pub(crate) fn controls<V: 'static>(
        &self,
        picker: Picker,
        count: usize,
        on_submit: impl Fn(&mut V, usize, &mut Window, &mut Context<V>) + 'static,
        cx: &mut Context<V>,
    ) -> Div {
        let next = self.clone();
        let previous = self.clone();
        let submit = self.clone();

        div()
            .on_action(cx.listener(move |_, _: &SelectNext, _, cx| {
                next.walk(count, 1, cx);
                cx.notify();
            }))
            .on_action(cx.listener(move |_, _: &SelectPrevious, _, cx| {
                previous.walk(count, -1, cx);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &Submit, window, cx| {
                if count > 0 {
                    on_submit(this, submit.cursor(count), window, cx);
                }
            }))
            .child(picker)
    }

    fn walk(&self, count: usize, step: isize, cx: &App) {
        if count == 0 {
            return;
        }
        let place = self.cursor(count) as isize + step;
        let cursor = place.rem_euclid(count as isize) as usize;
        self.cursor.set(cursor);
        self.scrollbar.read(cx).scroll().scroll_to_item(cursor);
    }
}

pub(crate) fn matches_query(id: &str, label: &str, query: &str) -> bool {
    query.is_empty() || label.to_lowercase().contains(query) || id.to_lowercase().contains(query)
}
