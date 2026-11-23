use std::cell::Cell;
use std::rc::Rc;

use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, MouseButton,
    MouseDownEvent, Pixels, Point, Render, ScrollHandle, SharedString, Task, WeakEntity, Window,
    div, px,
};
use i18n::t;
use input::SEARCH_CONTEXT;
use music::{Album, Playlist, SavedArtist, Track};
use router::{Destination, navigate};
use ui::Input;

use crate::chrome::Chrome;
use crate::shared::menus::{ItemMenu, album_menu, artist_menu, playlist_menu};
use state::{
    AlbumHit, ArtistHit, Genres, Hit, Io, Kind, Network, Playback, PlaylistHit, Search, Wisp,
};
use ui::ActiveTheme as _;
use ui::{
    Activate, Button, Card, Cascade, Deck, Deselect, Dismiss, Dropdown, InlineLink, InlineLinks,
    Pinnable, Popup, Room, Scrollbar, Scroller, SelectLeft, SelectNext, SelectPrevious,
    SelectRight, Separator, Submit, Text, Theme, Unfold, VAST, clock, eyebrow, scrolled, snapped,
    tabular, vacant,
};

use crate::shared::cards;
use crate::shared::cells;
use crate::shared::pins::Pinned as _;
use crate::shared::shelves;
use crate::shared::trouble;

const RAIL: Pixels = gpui::px(12.);
/// How tall the band is where results fade out as they scroll up under what sits above them.
const FADE: Pixels = gpui::px(12.);
/// How many entries of the search history the panel lists.
const PAST: usize = 8;
/// How many songs the panel suggests.
const SUGGESTED: usize = 6;
/// How many placeholder rows stand in while the first suggestions load.
const SUGGESTED_WAITING: usize = 3;
const ROW_GAP: f32 = 0.25;
const BULLET: SharedString = SharedString::new_static("·");
const SONGS: &[Kind] = &[Kind::Song];
const ARTISTS: &[Kind] = &[Kind::Artist];
const RELEASES: &[Kind] = &[Kind::Album, Kind::Playlist];
use crate::shared::tracks::{PlaybackStatus, playback_status};

type Play = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone)]
enum HitMenu {
    Song(Box<Track>),
    Album(AlbumHit),
    Playlist(PlaylistHit),
    Artist(ArtistHit),
}

impl HitMenu {
    fn of(hit: &Hit) -> Option<Self> {
        match hit {
            Hit::Song(track) => Some(Self::Song(track.clone())),
            Hit::Album(album) => Some(Self::Album(album.clone())),
            Hit::Playlist(list) => Some(Self::Playlist(list.clone())),
            Hit::Artist(artist) if artist.id.is_some() => Some(Self::Artist(artist.clone())),
            Hit::Artist(_) => None,
        }
    }
}

/// One row of the panel under the search field: an open link, or a suggested hit while
/// something is typed.
#[derive(Clone)]
enum Offer {
    Link(String),
    Keyword(String),
}

impl Offer {
    /// What tells one row from another when the suggestions fill in under it.
    fn key(&self) -> String {
        match self {
            Offer::Link(query) => format!("link:{query}"),
            Offer::Keyword(keyword) => format!("keyword:{keyword}"),
        }
    }
}

/// The search page. Typing asks `suggestions`, whose best hits show in a panel under the field,
/// and only Enter or the Search button asks `search` for the full results the page lists.
pub(crate) struct SearchView {
    input: Entity<Input>,
    search: Entity<Search>,
    suggestions: Entity<Search>,
    /// The text the field last held, so a change of caret or selection is not taken for typing.
    typed: String,
    /// Whether the panel under the field was put away, by Escape, a press outside or a submit.
    /// A click in the field or typing brings it back.
    dismissed: bool,
    /// The panel row the arrow keys are on.
    pick: Option<usize>,
    /// The `key` of the row `pick` is on, so the pick stays with it as the suggestions reorder.
    picked: Option<String>,
    /// The panel's rows surfacing one after another as the suggestions come in.
    cascade: Cascade,
    /// The panel's box, stretching to fit as rows come and go.
    unfold: Unfold,
    /// [wisp] 关键词联想（与原版一致的下拉内容）与它的加载态。
    keywords: Vec<String>,
    keywords_loading: bool,
    keyword_task: Option<Task<()>>,
    /// The runtime the keyword suggestions are fetched on. They have to run there rather than on
    /// the UI thread this entity lives on: reqwest hands its connection driver to `tokio::spawn`,
    /// which panics when polled outside a tokio runtime.
    io: Io,
    /// Whether the arrow keys are on the best match, the stop between the field and the lists.
    on_best: bool,
    genres: Entity<Genres>,
    playback: Entity<Playback>,
    playback_status: PlaybackStatus,
    songs: Entity<Scrollbar>,
    artists: Entity<Scrollbar>,
    albums: Entity<Scrollbar>,
    mixed: Entity<Scrollbar>,
    browsing: Entity<Scrollbar>,
    track_menu: ItemMenu,
    context_menu: Option<(HitMenu, Point<Pixels>)>,
    focus: FocusHandle,
    cursor: Option<(usize, usize)>,
    rows: [usize; 3],
    lead: Rc<Cell<Pixels>>,
}

impl SearchView {
    pub(crate) fn new(
        search: Entity<Search>,
        suggestions: Entity<Search>,
        genres: Entity<Genres>,
        playback: Entity<Playback>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&genres, |_, _, cx| cx.notify()).detach();
        let input = cx.new(|cx| {
            Input::new("search-placeholder", cx)
                .icon("icons/search.svg")
                .clearable()
        });

        cx.observe(&input, |this, input, cx| {
            let query = input.read(cx).text().to_owned();
            if query == this.typed {
                return;
            }
            this.typed = query.clone();
            this.dismissed = false;
            this.pick = None;
            this.ask_keywords(&query, cx);
            // An emptied field goes back to browsing rather than keeping the last results.
            if query.trim().is_empty() {
                this.search.update(cx, |search, cx| search.ask("", cx));
            }
            cx.notify();
        })
        .detach();
        let field = input.read(cx).focus_handle(cx);
        cx.on_focus(&field, window, |this, _, cx| {
            this.pick = None;
            cx.notify();
        })
        .detach();
        cx.on_blur(&field, window, |this, _, cx| {
            this.dismissed = true;
            cx.notify();
        })
        .detach();

        cx.observe(&search, |this, _, cx| {
            this.track_menu.reset(cx);
            this.context_menu = None;
            this.cursor = None;
            this.on_best = false;
            this.rows = [0; 3];
            cx.notify();
        })
        .detach();
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();
        let library = Wisp::global(cx).library.clone();
        cx.observe(&library, |_, _, cx| cx.notify()).detach();
        let current_playback = playback_status(&playback, cx);
        cx.observe(&playback, |this, playback, cx| {
            let current = playback_status(&playback, cx);
            if this.playback_status != current {
                this.playback_status = current;
                cx.notify();
            }
        })
        .detach();

        let asked = input.read(cx).text().to_owned();
        search.update(cx, |search, cx| search.ask(&asked, cx));

        let me = cx.entity_id();
        let playlist_scrollbar = cx.new(|_| Scrollbar::inset().watching(me));

        Self {
            input,
            search,
            suggestions,
            keywords: Vec::new(),
            keywords_loading: false,
            keyword_task: None,
            io: Io::global(cx),
            typed: String::new(),
            dismissed: false,
            pick: None,
            picked: None,
            cascade: Cascade::default(),
            unfold: Unfold::default(),
            on_best: false,
            genres,
            playback,
            playback_status: current_playback,
            songs: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(me)),
            artists: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(me)),
            albums: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(me)),
            mixed: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(me)),
            browsing: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(me)),
            track_menu: ItemMenu::new(playlist_scrollbar, cx),
            context_menu: None,
            focus: cx.focus_handle(),
            cursor: None,
            rows: [0; 3],
            lead: Rc::new(Cell::new(Pixels::ZERO)),
        }
    }

    /// Puts the keyboard in the field when the page opens and leaves the panel put away. A click
    /// in the field or typing brings it up.
    pub(crate) fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismissed = true;
        self.pick = None;
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Fills the field with `query` and runs the full search for it, as if it had been typed
    /// and submitted here. The quick search hands its query over this way.
    ///
    /// `typed` is set alongside the field on purpose: the observer on the field compares the two
    /// and returns early when they agree, which is what keeps this text from asking for
    /// suggestions of the word it just filled in, and from reopening the panel.
    pub(crate) fn search_for(&mut self, query: &str, cx: &mut Context<Self>) {
        self.typed = query.to_owned();
        self.dismissed = true;
        self.pick = None;
        self.input
            .update(cx, |input, cx| input.set_text(query.to_owned(), cx));
        self.suggestions
            .update(cx, |suggestions, cx| suggestions.ask(query, cx));
        self.run(query, cx);
    }

    /// Moves the keyboard back to the field without opening the panel, for the arrow keys and
    /// Escape coming back up out of the results.
    fn return_to_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The rows of the panel: the best suggested hits while something is typed.
    fn offers(&self, cx: &App) -> Vec<Offer> {
        let mut offers = Vec::new();
        if Wisp::global(cx).links.read(cx).recognizes(&self.typed, cx) {
            offers.push(Offer::Link(self.typed.trim().to_owned()));
        }
        offers.extend(
            self.keywords
                .iter()
                .filter(|keyword| !keyword.trim().is_empty())
                .map(|keyword| Offer::Keyword(keyword.clone())),
        );
        offers
    }

    /// [wisp] 拉取关键词联想（与原版同一个接口）。结果到来时若输入已变则丢弃。
    ///
    /// The fetch runs on the tokio runtime rather than on the UI thread this entity lives on.
    /// That is not an optimisation: reqwest's connection driver is handed to `tokio::spawn`, so
    /// polling the response future on the gpui executor panics with "there is no reactor
    /// running" the moment the request opens a connection.
    fn ask_keywords(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = query.trim();
        if query.is_empty() {
            self.keywords_loading = false;
            self.keyword_task = None;
            self.keywords.clear();
            return;
        }
        let Some(client) = Wisp::global(cx).session.read(cx).client() else {
            log::debug!("search: keywords skipped, no client connected");
            return;
        };
        log::debug!("search: asking keywords for {query:?}");
        self.keywords_loading = true;
        self.keywords.clear();
        let asked = query.to_owned();
        let fetch = asked.clone();
        let io = self.io.clone();
        let task = cx.spawn(async move |this, cx| {
            // `JoinHandle` only answers once the runtime has run the future; a task that was
            // cancelled or panicked reports a failed join, which logs the same as no
            // suggestions.
            let loaded = io
                .spawn(async move { client.search_keywords(&fetch).await })
                .await;
            this.update(cx, |this, cx| {
                this.keywords_loading = false;
                // 联想结果只对发起它的那次输入有效；输入已变就丢弃。
                if this.typed.trim() != asked {
                    log::debug!("search: keywords dropped, the field moved on");
                    return;
                }
                match loaded {
                    Ok(Ok(list)) => {
                        log::debug!("search: keywords -> {} items", list.len());
                        this.keywords = list;
                    }
                    Ok(Err(error)) => {
                        log::warn!("search: keywords failed: {error:#}");
                    }
                    Err(error) => {
                        log::warn!("search: the keywords task ended: {error}");
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.keyword_task = Some(task);
    }

    /// Whether the panel shows: the field has the keyboard, nothing put the panel away, and it
    /// has rows or is still waiting on the first suggestions.
    ///
    /// The rows act on the press rather than the release precisely because the field lets go of
    /// the focus on a press outside itself: by the time the release comes the panel this test
    /// turned away is already gone.
    fn panel_open(&self, window: &Window, cx: &App) -> bool {
        if self.dismissed || !self.typing(window, cx) {
            return false;
        }
        let waiting = !self.typed.trim().is_empty() && self.keywords_loading;
        waiting || !self.offers(cx).is_empty()
    }

    /// Puts the arrow keys on panel row `at` and notes which row that is.
    fn choose(&mut self, at: Option<usize>, cx: &App) {
        self.pick = at;
        self.picked = at.and_then(|at| self.offers(cx).get(at).map(Offer::key));
    }

    /// Keeps the pick on the row it was on after the suggestions changed, or on the same place
    /// when that row is gone.
    fn keep_pick(&mut self, cx: &App) {
        let Some(at) = self.pick else {
            return;
        };
        let offers = self.offers(cx);
        let found = self
            .picked
            .as_ref()
            .and_then(|key| offers.iter().position(|offer| offer.key() == *key));
        let kept = found.or_else(|| offers.len().checked_sub(1).map(|last| at.min(last)));
        self.choose(kept, cx);
    }

    fn close_panel(&mut self, cx: &mut Context<Self>) {
        self.dismissed = true;
        self.pick = None;
        cx.notify();
    }

    /// Searches by the resolved name or lets the full search load the link's metadata.
    fn run(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = query.trim().to_owned();
        if query.is_empty() {
            return;
        }
        let suggestions = self.suggestions.read(cx);
        if suggestions.query() == query
            && let Some(name) = suggestions.linked_query().map(str::to_owned)
        {
            self.search_text(&name, cx);
            return;
        }
        self.search_text(&query, cx);
    }

    fn search_text(&mut self, query: &str, cx: &mut Context<Self>) {
        self.search.update(cx, |search, cx| search.ask(query, cx));
        self.close_panel(cx);
    }

    /// Enter in the field: the panel row the arrow keys are on, or else a full search for what
    /// is typed.
    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        let picked = self
            .pick
            .filter(|_| self.panel_open(window, cx))
            .and_then(|at| self.offers(cx).into_iter().nth(at));
        match picked {
            Some(offer) => self.take(offer, cx),
            None => {
                let query = self.typed.clone();
                self.run(&query, cx);
            }
        }
    }

    /// Acts on a panel row. A hit, from the history or suggested, opens the way it does in the
    /// results and moves to the top of the history.
    ///
    /// It takes no window on purpose: the press that reaches it is dispatched while the window
    /// is borrowed, so anything that reaches for the window again from here fails. The caller
    /// hands the keyboard back to the field instead.
    fn take(&mut self, offer: Offer, cx: &mut Context<Self>) {
        match offer {
            Offer::Link(query) => {
                Wisp::global(cx)
                    .links
                    .clone()
                    .update(cx, |links, cx| links.open(&query, cx));
                self.close_panel(cx);
            }
            // 与原版一致：点联想条目就是拿它执行一次搜索，并把它填进输入框。
            // `search_for` 会挡住随之而来的聚焦，不把面板又弹开。
            Offer::Keyword(keyword) => {
                self.search_for(&keyword, cx);
            }
        }
    }

    /// Escape on the page: it puts the panel away first and lets go of the field second. An
    /// escape with neither goes on to the workspace.
    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel_open(window, cx) {
            self.close_panel(cx);
            return;
        }
        if self.typing(window, cx) {
            window.focus(&self.focus, cx);
            return;
        }
        cx.propagate();
    }

    /// The panel's row for one offer: a plain keyword line, the way Wisp lists suggestions.
    /// Clicking one runs a full search for it.
    fn offer_row(
        &self,
        offer: &Offer,
        at: usize,
        total: usize,
        me: &WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
        let chosen = self.pick == Some(at);
        let theme = *cx.theme();
        let key = SharedString::from(offer.key());
        let label = match offer {
            Offer::Link(keyword) | Offer::Keyword(keyword) => keyword.clone(),
        };
        let taken = offer.clone();
        let me = me.clone();
        div()
            .id(key)
            .flex()
            .items_center()
            .px(theme.metrics.pad)
            .py(px(7.))
            .rounded_sm()
            .cursor_pointer()
            .when(chosen, |this| this.bg(theme.table_active))
            .hover(move |style| style.bg(theme.table_hover))
            .when(chosen, |this| this.aria_selected(true))
            .aria_position_in_set(at + 1)
            .aria_size_of_set(total)
            .when(chosen, |this| this.aria_active_descendant())
            // 用按下而不是松开：按在这一行时输入框在捕获阶段就放掉了焦点，面板随之收起，
            // 若等到松开再执行，行已经不在这一帧里，点击就落空了。
            //
            // 这里只能用不带窗口的 `update`：鼠标事件派发期间窗口正被借用，`update_in` 会
            // 再借一次同一个窗口而失败。要还给输入框的焦点，从实体里取句柄，再用回调给到
            // 的窗口来设。
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                let taken = taken.clone();
                let field = me
                    .update(cx, |this, cx| {
                        this.take(taken, cx);
                        this.input.read(cx).focus_handle(cx)
                    })
                    .ok();
                if let Some(field) = field {
                    window.focus(&field, cx);
                }
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme.text(Text::Body))
                    .text_color(theme.foreground)
                    .child(label),
            )
            .into_any_element()
    }

    /// The panel's rows, with placeholder rows while the first suggestions load. The search
    /// The panel's rows, with placeholder rows while the first suggestions load.
    fn panel_rows(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let offers = self.offers(cx);
        let me = cx.entity().downgrade();
        let waiting = self.keywords_loading.then(|| {
            (0..SUGGESTED_WAITING).map(|at| {
                Card::skeleton(("search-offer-waiting", at))
                    .tucked()
                    .into_any_element()
            })
        });

        // New rows surface one after another and the panel stretches to fit them, the way the
        // quick search fills.
        self.cascade
            .show(offers.iter().map(|offer| SharedString::from(offer.key())));
        let list = div()
            .id("search-offers")
            .role(Role::ListBox)
            .aria_label(t!("search-results"))
            .flex()
            .flex_col()
            .gap_px()
            .occlude()
            // A click inside the panel has landed somewhere in it, so the panel only goes
            // away after the click's own handler has run.
            .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx)))
            .children(waiting.into_iter().flatten())
            .children(offers.iter().enumerate().map(|(at, offer)| {
                let level = self.cascade.level(&offer.key(), window, cx);
                Cascade::surface(self.offer_row(offer, at, offers.len(), &me, cx), level)
            }));
        let body = div().flex().flex_col().child(list);
        self.unfold.wrap(body, false, window, cx).into_any_element()
    }

    /// The search field with its panel and the button that runs the full search. Once the page
    /// is too narrow for two columns of categories, the panel spans the button too, so the
    /// suggestions get the whole width.
    fn field(&self, gutter: Pixels, window: &Window, cx: &Context<Self>) -> AnyElement {
        let open = self.panel_open(window, cx);
        // A closed panel forgets its rows and height, so the next one unfolds from nothing.
        if !open {
            self.cascade.clear();
            self.unfold.reset();
        }
        let me = cx.entity().downgrade();
        let input = div()
            .flex_1()
            .min_w_0()
            // A click in the field asks for the panel. It listens in the capture phase because
            // the field stops the press.
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if event.button != MouseButton::Left {
                    return;
                }
                if this.dismissed {
                    this.dismissed = false;
                    cx.notify();
                }
            }))
            .child(self.input.clone());
        let button = Button::new("search-submit")
            .icon("icons/search.svg")
            .label(t!("search-submit"))
            .primary()
            .h(cx.theme().metrics.field)
            .disabled(self.typed.trim().is_empty())
            .on_click(cx.listener(|this, _, _, cx| {
                let query = this.typed.clone();
                this.run(&query, cx);
            }));
        let row = || div().flex().items_center().gap_2();
        let dropdown = |anchor: AnyElement| {
            Dropdown::new("search-field", anchor)
                .flex_1()
                .min_w_0()
                .open(open)
                .on_close(move |_, cx| {
                    me.update(cx, |this, cx| this.close_panel(cx)).ok();
                })
                .when(open, |this| this.child(self.panel_rows(window, cx)))
        };

        let width = cells::content_width(window, cx.theme().metrics.inset * 2., cx);
        let field = match shelves::lanes(width) == 1 {
            true => row().child(dropdown(
                row().child(input).child(button).into_any_element(),
            )),
            false => row()
                .child(dropdown(input.into_any_element()))
                .child(button),
        };
        field.flex_none().px(gutter).into_any_element()
    }

    fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel_open(window, cx) {
            if let Some(last) = self.offers(cx).len().checked_sub(1) {
                self.choose(Some(self.pick.map_or(0, |at| (at + 1).min(last))), cx);
                cx.notify();
            }
            return;
        }
        if self.search.read(cx).query().trim().is_empty() {
            // With no results to step into, Down asks for the panel the page opened without.
            if self.typing(window, cx) && self.dismissed {
                self.dismissed = false;
                cx.notify();
            }
            return;
        }
        let stacked = stacked(window, cx);
        let best = self.search.read(cx).best().is_some();
        match self.cursor {
            None if best && !self.on_best => self.on_best = true,
            None => {
                let Some(column) = self.first_filled(0, stacked, cx) else {
                    return;
                };
                self.on_best = false;
                self.place(column, 0);
            }
            Some((column, row)) => {
                let last = self
                    .seats(kinds(column, stacked), cx)
                    .len()
                    .saturating_sub(1);
                if row < last {
                    self.place(column, row + 1);
                }
            }
        }
        window.focus(&self.focus, cx);
        self.reveal(window, cx);
        cx.notify();
    }

    fn select_previous(&mut self, _: &SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel_open(window, cx) {
            if let Some(at) = self.pick {
                self.choose(at.checked_sub(1), cx);
                cx.notify();
            }
            return;
        }
        let Some((column, row)) = self.cursor else {
            if std::mem::take(&mut self.on_best) {
                self.return_to_field(window, cx);
                cx.notify();
            }
            return;
        };
        if row == 0 {
            self.cursor = None;
            match self.search.read(cx).best().is_some() {
                true => {
                    self.on_best = true;
                    self.reveal(window, cx);
                }
                false => self.return_to_field(window, cx),
            }
            cx.notify();
            return;
        }
        self.place(column, row - 1);
        window.focus(&self.focus, cx);
        self.reveal(window, cx);
        cx.notify();
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        let stacked = stacked(window, cx);
        let Some((column, _)) = self.cursor else {
            return;
        };
        let Some(next) = self.prev_filled(column, stacked, cx) else {
            return;
        };
        self.hop(next, stacked, cx);
        window.focus(&self.focus, cx);
        self.reveal(window, cx);
        cx.notify();
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        let stacked = stacked(window, cx);
        let Some((column, _)) = self.cursor else {
            return;
        };
        let Some(next) = self.next_filled(column, stacked, cx) else {
            return;
        };
        self.hop(next, stacked, cx);
        window.focus(&self.focus, cx);
        self.reveal(window, cx);
        cx.notify();
    }

    fn activate_hit(&mut self, _: &Activate, window: &mut Window, cx: &mut Context<Self>) {
        if self.on_best {
            if let Some(best) = self.search.read(cx).best().cloned() {
                self.open_hit(best, cx);
            }
            return;
        }
        let Some((column, row)) = self.cursor else {
            return;
        };
        let stacked = stacked(window, cx);
        let seats = self.seats(kinds(column, stacked), cx);
        let Some(&(at, _)) = seats.get(row) else {
            return;
        };
        let Some(hit) = self.search.read(cx).hits().get(at).cloned() else {
            return;
        };
        self.open_hit(hit, cx);
    }

    /// What choosing a hit does: a song plays its radio, or pauses and resumes when it is the one
    /// playing, and anything else opens its page. Either way the hit moves to the top of the
    /// search history.
    fn open_hit(&mut self, hit: Hit, cx: &mut Context<Self>) {
        hit.remember(cx);
        match hit {
            Hit::Song(track) => {
                let current = track.id.is_some() && track.id == self.playback_status.0;
                self.playback.update(cx, |playback, cx| match current {
                    true => playback.toggle_play(cx),
                    false => playback.play_radio(&track, cx),
                });
            }
            Hit::Artist(artist) => {
                if let Some(id) = artist.id {
                    navigate(Destination::Artist(id.into()), cx);
                }
            }
            Hit::Album(album) => navigate(Destination::Album(album.id.into()), cx),
            Hit::Playlist(list) => navigate(Destination::Playlist(list.id.into()), cx),
        }
    }

    fn deselect_hit(&mut self, _: &Deselect, window: &mut Window, cx: &mut Context<Self>) {
        let on_best = std::mem::take(&mut self.on_best);
        if self.cursor.take().is_none() && !on_best {
            return;
        }
        self.return_to_field(window, cx);
        cx.notify();
    }

    fn place(&mut self, column: usize, row: usize) {
        if let Some(slot) = self.rows.get_mut(column) {
            *slot = row;
        }
        self.cursor = Some((column, row));
    }

    fn hop(&mut self, column: usize, stacked: bool, cx: &App) {
        let last = self
            .seats(kinds(column, stacked), cx)
            .len()
            .saturating_sub(1);
        let row = self.rows.get(column).copied().unwrap_or(0).min(last);
        self.place(column, row);
    }

    fn first_filled(&self, start: usize, stacked: bool, cx: &App) -> Option<usize> {
        (start..columns(stacked)).find(|&column| !self.seats(kinds(column, stacked), cx).is_empty())
    }

    fn next_filled(&self, column: usize, stacked: bool, cx: &App) -> Option<usize> {
        self.first_filled(column + 1, stacked, cx)
    }

    fn prev_filled(&self, column: usize, stacked: bool, cx: &App) -> Option<usize> {
        (0..column)
            .rev()
            .find(|&at| !self.seats(kinds(at, stacked), cx).is_empty())
    }

    /// Scrolls the list the arrow keys are in so their row shows. The best match heads the
    /// mixed list, so standing on it scrolls that list to the top.
    fn reveal(&self, window: &Window, cx: &App) {
        let stacked = stacked(window, cx);
        if self.on_best && stacked {
            let scroll = self.mixed.read(cx).scroll().clone();
            scroll.set_offset(gpui::point(scroll.offset().x, Pixels::ZERO));
            return;
        }
        let Some((column, row)) = self.cursor else {
            return;
        };
        let scroll = self.bar(column, stacked).read(cx).scroll().clone();
        let theme = *cx.theme();
        let height = snapped(theme.metrics.list_row, window);
        let gap = theme.font_size * ROW_GAP;
        let above = match stacked {
            true => self.lead.get(),
            false => Pixels::ZERO,
        };
        let top = (height + gap) * row as f32 + above;
        let visible = scroll.bounds().size.height;
        if visible <= Pixels::ZERO {
            return;
        }
        let offset = scroll.offset();
        let shown = -offset.y;
        let delta = if top < shown {
            top - shown
        } else if top + height > shown + visible {
            top + height - (shown + visible)
        } else {
            return;
        };
        scroll.set_offset(gpui::point(offset.x, offset.y - delta));
    }

    fn bar(&self, column: usize, stacked: bool) -> &Entity<Scrollbar> {
        match (stacked, column) {
            (true, _) => &self.mixed,
            (false, 0) => &self.songs,
            (false, 1) => &self.artists,
            _ => &self.albums,
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the row reports its seat in the list, so position and total ride along"
    )]
    fn row(
        &self,
        hit: &Hit,
        place: usize,
        index: usize,
        total: usize,
        compact: bool,
        chosen: bool,
        me: &WeakEntity<Self>,
        cx: &App,
    ) -> Card {
        let current = match hit {
            Hit::Song(track) => track.id.is_some() && track.id == self.playback_status.0,
            _ => false,
        };

        let refused = matches!(hit, Hit::Song(track) if !track.playable);
        hit_card(hit, place, compact, current, cx.theme())
            // 面板容器自己也有 on_click（关面板），GPUI 的 click 自顶向下分派——
            // 行内先拦住冒泡，否则容器先把面板关了，行的打开动作就被吞掉。
            .when(opens(hit) && !refused, |card| {
                let me = me.clone();
                let hit = hit.clone();
                card.press(move |event, window, cx| {
                    cx.stop_propagation();
                    pressed(&hit, &me)(event, window, cx)
                })
            })
            .when_some(
                self.transport(hit, me, cx)
                    .filter(|_| !matches!(hit, Hit::Song(track) if !track.playable)),
                |card, (playing, play)| card.play(playing, play),
            )
            .when_some(HitMenu::of(hit), |card, target| card.menu(menu(target, me)))
            .chosen(chosen)
            // The keyboard cursor stays on the container, so the chosen card claims
            // the active descendant while every card reports its seat in the list.
            .aria_position_in_set(index + 1)
            .aria_size_of_set(total)
            .when(chosen, |card| {
                card.aria_selected(true).aria_active_descendant()
            })
    }

    /// The play button of a hit. Playing from it files the hit in the search history, as opening
    /// it would.
    fn transport(&self, hit: &Hit, me: &WeakEntity<Self>, cx: &App) -> Option<(bool, Play)> {
        let me = me.clone();
        let chosen = hit.clone();
        let origin = match hit {
            Hit::Song(track) => {
                let current = track.id.is_some() && track.id == self.playback_status.0;
                let playing = current && self.playback_status.1 == Some(true);
                let track = track.clone();
                let play: Play = Box::new(move |_, _, cx| {
                    chosen.remember(cx);
                    me.update(cx, |this, cx| {
                        this.playback.update(cx, |playback, cx| match current {
                            true => playback.toggle_play(cx),
                            false => playback.play_radio(&track, cx),
                        });
                    })
                    .ok();
                });
                return Some((playing, play));
            }
            Hit::Artist(artist) => {
                state::Origin::artist(artist.id.clone()?).named(artist.name.clone())
            }
            Hit::Album(album) => state::Origin::album(album.id.clone()).named(album.name.clone()),
            Hit::Playlist(list) => {
                state::Origin::playlist(list.id.clone()).named(list.name.clone())
            }
        };
        let playing = self.playback.read(cx).engaged(&origin, cx);
        let play: Play = Box::new(move |_, _, cx| {
            chosen.remember(cx);
            me.update(cx, |this, cx| {
                this.playback
                    .update(cx, |playback, cx| playback.toggle_origin(&origin, cx));
            })
            .ok();
        });

        Some((playing, play))
    }

    fn best(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let theme = *cx.theme();
        let hit = self.search.read(cx).best()?;
        let (kind, title, artists) = match hit {
            Hit::Song(track) => (Kind::Song, track.name.clone(), track.artist_refs.clone()),
            Hit::Artist(artist) => (Kind::Artist, artist.name.clone(), Vec::new()),
            Hit::Album(album) => (Kind::Album, album.name.clone(), album.artist_refs.clone()),
            Hit::Playlist(list) => (Kind::Playlist, list.name.clone(), Vec::new()),
        };

        let me = cx.entity().downgrade();
        let card = Card::new("best", title)
            .art(theme.metrics.cover * 0.45)
            .cover(cover(hit))
            .when(matches!(hit, Hit::Song(track) if track.paid), |card| card.paid())
            .when(matches!(hit, Hit::Song(track) if !track.playable), |card| {
                card.opacity(0.55)
            })
            .when(matches!(kind, Kind::Artist), Card::circle)
            .eyebrow(noun(kind))
            .size(Text::Title)
            .weight(FontWeight::BOLD)
            .bare_meta(
                cells::artist_links(
                    "best-artist-link",
                    artists,
                    meta(hit, false),
                    theme.muted_foreground,
                )
                .text_size(theme.text(Text::Small))
                .truncate(),
            )
            .flat()
            .gap_4()
            .p_3()
            .bg(match self.on_best {
                true => theme.table_active,
                false => theme.secondary,
            })
            .when(self.on_best, |card| {
                card.aria_selected(true).aria_active_descendant()
            })
            .when_some(self.transport(hit, &me, cx), |card, (playing, play)| {
                card.play(playing, play)
            })
            .when_some(hit.pin(), Pinnable::pin)
            .when(opens(hit), |card| card.press(pressed(hit, &me)))
            .when_some(HitMenu::of(hit), |card, target| {
                card.on_mouse_down(MouseButton::Right, menu(target, &me))
            })
            .aria_description(noun(kind))
            .into_any_element();

        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap_2()
                .child({
                    let label = t!("search-best-match");
                    eyebrow(label.clone(), cx)
                        .id("search-best-heading")
                        .role(Role::Heading)
                        .aria_level(2)
                        .aria_label(label)
                        .pb_1()
                })
                .child(card)
                .into_any_element(),
        )
    }

    /// The page search shows in place of its results. Nothing here works without the network,
    /// so the No connection state covers the whole screen the moment it is gone, and a failed
    /// search or a failed browse shows its own reason otherwise. Asking again is the retry,
    /// since neither loader counts a failure as served.
    fn failure(&self, asked: bool, gutter: Pixels, cx: &Context<Self>) -> Option<AnyElement> {
        let offline = Network::lost(cx);
        let (id, reason, retry) = match asked {
            true => {
                let reason = match offline {
                    true => None,
                    false => Some(self.search.read(cx).error()?.to_owned()),
                };
                let search = self.search.clone();
                let query = search.read(cx).query().to_owned();
                let retry: Box<dyn Fn(&mut App)> = Box::new(move |cx| {
                    search.update(cx, |search, cx| search.ask(&query, cx));
                });
                ("search-lost", reason, retry)
            }
            false => {
                let reason = match offline {
                    true => None,
                    false => Some(self.genres.read(cx).error()?.to_owned()),
                };
                let genres = self.genres.clone();
                let retry: Box<dyn Fn(&mut App)> = Box::new(move |cx| {
                    genres.update(cx, |genres, cx| genres.load(cx));
                });
                ("search-browse-lost", reason, retry)
            }
        };

        Some(
            trouble::lost(
                id,
                t!("trouble-not-loaded"),
                reason.as_deref(),
                move |_, _, cx| retry(cx),
                cx,
            )
            .flex_1()
            .min_h_0()
            .px(gutter)
            .into_any_element(),
        )
    }

    fn shell(
        &self,
        id: &'static str,
        title: SharedString,
        body: AnyElement,
        gutter: Pixels,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .child(
                div()
                    .id(id)
                    .role(Role::Heading)
                    .aria_level(2)
                    .aria_label(title.clone())
                    .pl(gutter)
                    .child(eyebrow(title, cx).pb_1()),
            )
            .child(body)
            .into_any_element()
    }

    fn column(&self, kind: Kind, window: &Window, cx: &Context<Self>) -> AnyElement {
        let (id, title, only) = match kind {
            Kind::Song => ("search-songs-heading", t!("search-songs"), SONGS),
            Kind::Artist => ("search-artists-heading", t!("search-artists"), ARTISTS),
            Kind::Album | Kind::Playlist => (
                "search-releases-heading",
                t!("search-albums-playlists"),
                RELEASES,
            ),
        };
        let body = self.deck(only, RAIL, None, title.clone(), window, cx);

        self.shell(id, title, body, RAIL, cx)
    }

    fn browse(&self, gutter: Pixels, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let found = self.genres.read(cx).genres();
        let theme = *cx.theme();
        let pad = theme.metrics.inset;
        let width = cells::content_width(window, pad * 2., cx);
        let loader = self.genres.clone();
        let needs_covers = move |row: &[music::Genre], cx: &mut App| {
            let ids = row.iter().map(|genre| genre.id.clone()).collect::<Vec<_>>();
            loader.update(cx, |genres, cx| genres.ensure_covers(ids, cx));
        };
        let plates = shelves::grid("genre", found, width, window, cx, needs_covers);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(div().px(gutter).child({
                let label = t!("search-browse");
                div()
                    .id("search-browse-heading")
                    .role(Role::Heading)
                    .aria_level(2)
                    .aria_label(label.clone())
                    .child(eyebrow(label, cx))
            }))
            .child(
                Scroller::new("search-browse", &self.browsing)
                    .hold()
                    .bottom_inset(crate::chrome::Chrome::bottom(cx))
                    .fade_top(FADE)
                    .px(gutter)
                    .pb(pad)
                    .child(plates),
            )
            .into_any_element()
    }

    fn everything(&self, gutter: Pixels, window: &Window, cx: &Context<Self>) -> AnyElement {
        let lead = div()
            .flex()
            .flex_col()
            .gap_6()
            .children(self.best(cx))
            .child({
                let label = t!("search-results");
                eyebrow(label.clone(), cx)
                    .id("search-results-heading")
                    .role(Role::Heading)
                    .aria_level(2)
                    .aria_label(label)
                    .pb_1()
            })
            .into_any_element();

        self.deck(
            &Kind::ALL,
            gutter,
            Some(lead),
            t!("search-results"),
            window,
            cx,
        )
    }

    /// Where each hit of the `only` kinds sits: its index among all hits and its place among
    /// the hits of its kind. The first hit is the best match drawn above the lists, so the
    /// lists leave it out.
    fn seats(&self, only: &[Kind], cx: &App) -> Vec<(usize, usize)> {
        let mut taken = [0; Kind::ALL.len()];

        self.search
            .read(cx)
            .hits()
            .iter()
            .enumerate()
            .skip(1)
            .filter_map(|(at, hit)| {
                let kind = hit.kind();
                let place = taken[seat(kind)];
                taken[seat(kind)] += 1;
                only.contains(&kind).then_some((at, place))
            })
            .collect()
    }

    fn deck(
        &self,
        only: &[Kind],
        gutter: Pixels,
        lead: Option<AnyElement>,
        label: SharedString,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (id, bar) = match only {
            [Kind::Song] => ("search-songs", &self.songs),
            [Kind::Artist] => ("search-artists", &self.artists),
            [Kind::Album, Kind::Playlist] => ("search-albums", &self.albums),
            _ => ("search-all", &self.mixed),
        };
        let seats = self.seats(only, cx);
        if seats.is_empty() {
            let title = t!("search-no-matches");
            let empty = div()
                .id(SharedString::from(format!("{id}-empty")))
                .role(Role::Status)
                .aria_label(title.clone())
                .flex_none()
                .child(vacant(title, cx));
            return match lead {
                Some(lead) => div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px(gutter)
                    .child(lead)
                    .child(empty)
                    .into_any_element(),
                None => empty.into_any_element(),
            };
        }

        let leading = lead.is_some();
        let compact = only.len() > 1;
        let column = column_of(only);
        let theme = *cx.theme();
        let row = snapped(theme.metrics.list_row, window);
        let scroll = bar.read(cx).scroll().clone();
        let me = cx.entity().downgrade();
        let measured = self.lead.clone();
        let deck = Deck::new(format!("{id}-deck"))
            .rows((0..seats.len()).map(|_| row))
            .gap(theme.font_size * ROW_GAP)
            .when(lead.is_some(), |deck| {
                deck.on_measure(move |top, _, _| {
                    let content = scroll.bounds().origin.y - scrolled(&scroll);
                    measured.set(top - content);
                })
            })
            .draw(move |index, _, cx| {
                let Some(view) = me.upgrade() else {
                    return div().into_any_element();
                };
                let Some(&(at, place)) = seats.get(index) else {
                    return div().into_any_element();
                };
                let this = view.read(cx);
                let Some(hit) = this.search.read(cx).hits().get(at) else {
                    return div().into_any_element();
                };

                this.row(
                    hit,
                    place,
                    index,
                    seats.len(),
                    compact,
                    this.cursor == Some((column, index)),
                    &view.downgrade(),
                    cx,
                )
                .into_any_element()
            });

        div()
            .id(SharedString::from(format!("{id}-results")))
            .role(Role::ListBox)
            .aria_label(label)
            .flex_1()
            .min_h_0()
            // The mixed list sits right under the field, so its fade spends the gap above it.
            .when(leading, |this| this.mt(-FADE))
            .child(
                Scroller::new(id, bar)
                    .bottom_inset(crate::chrome::Chrome::bottom(cx))
                    .fade_top(FADE)
                    .px(gutter)
                    .pb(theme.metrics.inset)
                    .child(div().flex().flex_col().gap_1().children(lead).child(deck)),
            )
            .into_any_element()
    }
}

fn stacked(window: &Window, cx: &App) -> bool {
    !Chrome::room(window, cx).fits(Room::Wide)
}

fn columns(stacked: bool) -> usize {
    match stacked {
        true => 1,
        false => 3,
    }
}

fn kinds(column: usize, stacked: bool) -> &'static [Kind] {
    match (stacked, column) {
        (true, _) => &Kind::ALL,
        (false, 0) => SONGS,
        (false, 1) => ARTISTS,
        _ => RELEASES,
    }
}

fn column_of(only: &[Kind]) -> usize {
    match only {
        [Kind::Song] => 0,
        [Kind::Artist] => 1,
        [Kind::Album, Kind::Playlist] => 2,
        _ => 0,
    }
}

fn seat(kind: Kind) -> usize {
    match kind {
        Kind::Song => 0,
        Kind::Artist => 1,
        Kind::Album => 2,
        Kind::Playlist => 3,
    }
}

/// Whether pressing the card of `hit` does anything. Only an artist the provider gave no id
/// has no page to open.
fn opens(hit: &Hit) -> bool {
    !matches!(hit, Hit::Artist(artist) if artist.id.is_none())
}

/// What a press on the card of `hit` does: a song starts its radio and anything else opens its
/// page. The hit moves to the top of the search history.
fn pressed(
    hit: &Hit,
    me: &WeakEntity<SearchView>,
) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    let me = me.clone();
    let hit = hit.clone();

    move |_, _, cx| {
        hit.remember(cx);
        me.update(cx, |this, cx| match &hit {
            Hit::Song(track) => this
                .playback
                .update(cx, |playback, cx| playback.play_radio(track, cx)),
            Hit::Artist(artist) => {
                if let Some(id) = &artist.id {
                    navigate(Destination::Artist(id.clone().into()), cx);
                }
            }
            Hit::Album(album) => navigate(Destination::Album(album.id.clone().into()), cx),
            Hit::Playlist(list) => navigate(Destination::Playlist(list.id.clone().into()), cx),
        })
        .ok();
    }
}

fn menu(
    target: HitMenu,
    me: &WeakEntity<SearchView>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    let me = me.clone();

    move |event: &MouseDownEvent, window, cx| {
        window.prevent_default();
        me.update(cx, |this, cx| {
            this.track_menu.reset(cx);
            this.context_menu = Some((target.clone(), event.position));
            cx.notify();
        })
        .ok();
    }
}

fn album_of(hit: &AlbumHit, cx: &App) -> Album {
    Wisp::global(cx)
        .library
        .read(cx)
        .album(&hit.id)
        .cloned()
        .unwrap_or_else(|| Album {
            id: hit.id.clone(),
            name: hit.name.clone(),
            artists: hit.artists.clone(),
            artist_refs: hit.artist_refs.clone(),
            cover: hit.cover.clone(),
            cover_large: hit.cover.clone(),
            ..Default::default()
        })
}

fn playlist_of(hit: &PlaylistHit, cx: &App) -> Playlist {
    Wisp::global(cx)
        .library
        .read(cx)
        .playlist(&hit.id)
        .cloned()
        .unwrap_or_else(|| Playlist {
            id: hit.id.clone(),
            name: hit.name.clone(),
            owner: hit.owner.clone(),
            cover: hit.cover.clone(),
            ..Default::default()
        })
}

fn artist_of(hit: &ArtistHit, cx: &App) -> SavedArtist {
    let id = hit.id.clone().unwrap_or_default();
    Wisp::global(cx)
        .library
        .read(cx)
        .artist(&id)
        .cloned()
        .unwrap_or_else(|| SavedArtist {
            id,
            name: hit.name.clone(),
            cover: hit.cover.clone(),
            ..Default::default()
        })
}

fn cover(hit: &Hit) -> Option<String> {
    match hit {
        Hit::Song(track) => track.cover.clone(),
        Hit::Artist(artist) => artist.cover.clone(),
        Hit::Album(album) => album.cover.clone(),
        Hit::Playlist(list) => list.cover.clone(),
    }
}

fn meta(hit: &Hit, compact: bool) -> SharedString {
    match hit {
        Hit::Song(track) => SharedString::from(track.artists.clone()),
        Hit::Album(album) => SharedString::from(album.artists.clone()),
        Hit::Artist(artist) => match compact {
            true => noun(Kind::Artist),
            false => held(artist.saved),
        },
        Hit::Playlist(list) => match list.owner.is_empty() {
            true => noun(Kind::Playlist),
            false => SharedString::from(list.owner.clone()),
        },
    }
}

/// The card for one hit, shared by the search page and the quick search so a hit looks and
/// links the same in both. `compact` is the mixed list, where the subtitle leads with the kind
/// and a song names its album. The caller decides what a press and the play button do.
pub(crate) fn hit_card(
    hit: &Hit,
    place: usize,
    compact: bool,
    current: bool,
    theme: &Theme,
) -> Card {
    let meta = subtitle(hit, place, compact, theme);
    let card = match hit {
        Hit::Song(track) => Card::new(("song", place), track.name.clone())
            .cover(track.cover.clone())
            .fallback("icons/music.svg")
            .when(current, |card| card.tint(theme.primary))
            .when(track.paid, |card| card.paid())
            .when(!track.playable, |card| {
                card.opacity(0.55).tint(theme.muted_foreground)
            })
            .when(track.explicit, |card| card.explicit())
            .trailing(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_size(theme.text(Text::Small))
                    .text_color(theme.muted_foreground)
                    .font_features(tabular())
                    .child(clock(track.duration)),
            ),
        Hit::Artist(artist) => Card::new(("artist", place), artist.name.clone())
            .cover(artist.cover.clone())
            .fallback("icons/user.svg")
            .circle()
            .underline(),
        Hit::Album(album) => Card::new(("album", place), album.name.clone())
            .cover(album.cover.clone())
            .fallback("icons/disc-3.svg")
            .underline(),
        Hit::Playlist(list) => Card::new(("playlist", place), list.name.clone())
            .cover(list.cover.clone())
            .fallback("icons/list.svg")
            .underline(),
    };

    card.meta(meta)
        .when_some(hit.pin(), Pinnable::pin)
        .when(compact, Card::tucked)
        // The mixed list tags each row with its kind, which the card's name leaves out.
        .when(compact, |card| card.aria_description(noun(hit.kind())))
}

/// The line under a hit's name, where every artist, owner and album links to its page.
fn subtitle(hit: &Hit, place: usize, compact: bool, theme: &Theme) -> AnyElement {
    if let (Hit::Album(album), false) = (hit, compact) {
        return cards::released(
            format!("album-artist-{place}"),
            album.year,
            None,
            &album.artist_refs,
            album.artists.clone(),
            theme,
        )
        .into_any_element();
    }

    let muted = theme.muted_foreground;
    let (kind, id, artists, fallback) = match hit {
        Hit::Song(track) => (
            Kind::Song,
            format!("song-artist-{place}"),
            track.artist_refs.clone(),
            track.artists.clone(),
        ),
        Hit::Album(album) => (
            Kind::Album,
            format!("album-artist-{place}"),
            album.artist_refs.clone(),
            album.artists.clone(),
        ),
        Hit::Playlist(list) if !list.owner.is_empty() => (
            Kind::Playlist,
            format!("playlist-owner-{place}"),
            Vec::new(),
            list.owner.clone(),
        ),
        Hit::Artist(_) | Hit::Playlist(_) => {
            return meta(hit, compact).into_any_element();
        }
    };

    let links = cells::artist_links(id, artists, fallback, muted).truncate();
    if !compact {
        return links.into_any_element();
    }

    let album = match hit {
        Hit::Song(track) if !track.album.is_empty() => Some(
            InlineLinks::new(
                format!("song-album-{place}"),
                [InlineLink::new(
                    track.album.clone(),
                    track.album_id.clone().map(Into::into),
                )],
                track.album.clone(),
                muted,
            )
            .truncate()
            .on_click(|id, cx| navigate(Destination::Album(id), cx)),
        ),
        _ => None,
    };

    div()
        .flex()
        .min_w_0()
        .gap_1()
        .child(div().flex_none().child(tag(kind)))
        .child(links)
        .when_some(album, |this, album| {
            this.child(div().flex_none().child(BULLET)).child(album)
        })
        .into_any_element()
}

pub(crate) fn tag(kind: Kind) -> SharedString {
    let noun = noun(kind);
    t!("search-tag", kind = &noun)
}

fn held(saved: usize) -> SharedString {
    match saved {
        0 => noun(Kind::Artist),
        count => t!("search-saved", count = count),
    }
}

pub(crate) fn noun(kind: Kind) -> SharedString {
    match kind {
        Kind::Song => t!("kind-song"),
        Kind::Artist => t!("kind-artist"),
        Kind::Album => t!("kind-album"),
        Kind::Playlist => t!("kind-playlist"),
    }
}

impl Focusable for SearchView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SearchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let pad = theme.metrics.inset;
        let width = cells::content_width(window, pad * 2., cx);
        let stacked = !Chrome::room(window, cx).fits(Room::Wide);
        let inset = match width > VAST {
            true => (width - VAST) / 2.,
            false => Pixels::ZERO,
        };
        let asked = !self.search.read(cx).query().trim().is_empty();
        let context_menu = self.context_menu.clone().map(|(target, position)| {
            let menu = match target {
                HitMenu::Song(track) => self.track_menu.for_track(&track, cx),
                HitMenu::Album(hit) => album_menu(
                    album_of(&hit, cx),
                    self.playback.clone(),
                    &self.track_menu,
                    cx,
                ),
                HitMenu::Playlist(hit) => {
                    playlist_menu(playlist_of(&hit, cx), self.playback.clone(), cx)
                }
                HitMenu::Artist(hit) => {
                    artist_menu(artist_of(&hit, cx), self.playback.clone(), false, cx)
                }
            };

            Popup::new(position, menu).on_close(cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                cx.notify();
            }))
        });

        let gutter = pad + inset + Chrome::margin(window, cx);
        let results = match self.failure(asked, gutter, cx) {
            Some(failure) => failure,
            None => match (asked, stacked) {
                (false, _) => self.browse(gutter, window, cx),
                (true, true) => self.everything(gutter, window, cx),
                (true, false) => div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .px(gutter)
                    .child(self.column(Kind::Song, window, cx))
                    .child(Separator::vertical())
                    .child(self.column(Kind::Artist, window, cx))
                    .child(Separator::vertical())
                    .child(self.column(Kind::Album, window, cx))
                    .into_any_element(),
            },
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_6()
            .pt(pad)
            .key_context(SEARCH_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::activate_hit))
            .on_action(cx.listener(Self::deselect_hit))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::dismiss))
            .child(self.field(gutter, window, cx))
            .when(!stacked, |this| {
                this.children(self.best(cx).map(|best| div().px(gutter).child(best)))
            })
            .child(results)
            .when_some(context_menu, |this, menu| this.child(menu))
    }
}
