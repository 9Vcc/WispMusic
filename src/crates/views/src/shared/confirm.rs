use gpui::prelude::*;
use gpui::{App, Context, Entity, FocusHandle, Global, Render, SharedString, Window, div};
use i18n::t;
use state::{Io, Outcome, Toasts, Wisp, equalizer_profile_label};
use ui::{Button, Dismiss, FORM_CONTEXT, Modal, Submit};

#[derive(Clone)]
pub(crate) enum Kind {
    DeleteTrackFiles(usize),
    /// An equalizer profile, by its stored name.
    EqualizerProfile(SharedString),
    /// A provider plugin, by display name, and the version `CURRENT` goes back to.
    PluginRollback(SharedString, SharedString),
    /// A provider plugin, by display name.
    PluginUninstall(SharedString),
}

impl Kind {
    fn title(&self) -> gpui::SharedString {
        match self {
            Self::DeleteTrackFiles(_) => t!("confirm-delete-track-files-title"),
            Self::EqualizerProfile(_) => t!("equalizer-delete"),
            Self::PluginRollback(..) => t!("settings-plugin-rollback"),
            Self::PluginUninstall(_) => t!("settings-plugin-uninstall"),
        }
    }

    fn detail(&self) -> gpui::SharedString {
        match *self {
            Self::DeleteTrackFiles(count) => t!("confirm-delete-track-files", count = count),
            Self::EqualizerProfile(ref name) => {
                let name = equalizer_profile_label(name);
                t!("equalizer-delete-confirm", name = &name)
            }
            Self::PluginRollback(ref name, ref version) => {
                t!(
                    "settings-plugin-rollback-confirm",
                    name = name,
                    version = version
                )
            }
            Self::PluginUninstall(ref name) => {
                t!("settings-plugin-uninstall-confirm", name = name)
            }
        }
    }

    fn action(&self) -> gpui::SharedString {
        match self {
            Self::PluginRollback(..) => t!("settings-plugin-rollback"),
            Self::PluginUninstall(_) => t!("settings-plugin-uninstall"),
            _ => t!("common-delete"),
        }
    }
}

type Apply = Box<dyn FnOnce(&mut App)>;

struct Pending {
    kind: Kind,
    apply: Apply,
}

pub(crate) struct Confirm {
    pending: Option<Pending>,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
    grab: bool,
}

struct Installed(Entity<Confirm>);

impl Global for Installed {}

impl Confirm {
    pub fn entity(cx: &mut App) -> Entity<Self> {
        if cx.try_global::<Installed>().is_none() {
            let confirm = cx.new(|cx| Self {
                pending: None,
                focus: cx.focus_handle(),
                restore: None,
                grab: false,
            });
            cx.set_global(Installed(confirm));
        }
        cx.global::<Installed>().0.clone()
    }

    pub fn ask(kind: Kind, apply: impl FnOnce(&mut App) + 'static, cx: &mut App) {
        let confirm = Self::entity(cx);
        confirm.update(cx, |this, cx| {
            this.pending = Some(Pending {
                kind,
                apply: Box::new(apply),
            });
            this.grab = true;
            cx.notify();
        });
    }

    pub fn delete_track_files(ids: Vec<String>, cx: &mut App) {
        if ids.is_empty() {
            return;
        }
        Self::ask(
            Kind::DeleteTrackFiles(ids.len()),
            move |cx| {
                let wisp = Wisp::global(cx);
                let Some(provider) = wisp.session.read(cx).local_client() else {
                    return;
                };
                let library = wisp.library.clone();
                let playback = wisp.playback.clone();
                let io = Io::global(cx);
                cx.spawn(async move |cx| {
                    let result = io
                        .spawn(async move {
                            let mut failed = 0;
                            let mut deleted = Vec::new();
                            for id in ids {
                                match provider.delete_track_file(&id).await {
                                    Ok(()) => deleted.push(id),
                                    Err(error) => {
                                        failed += 1;
                                        log::warn!(
                                            "local: cannot delete track file {id}: {error:#}"
                                        );
                                    }
                                }
                            }
                            (failed, deleted)
                        })
                        .await;

                    if let Ok((_, ref deleted)) = result {
                        library.update(cx, |library, cx| library.hide_local_tracks(deleted, cx));
                        playback.update(cx, |playback, cx| playback.remove_from_queue(deleted, cx));
                    }
                    match result {
                        Ok((failed, _)) if failed > 0 => {
                            cx.update(|cx| {
                                Toasts::show(Outcome::Failed, "toast-local-delete-failed", cx);
                            });
                        }
                        Err(error) => {
                            log::warn!("local: deletion task failed: {error}");
                            cx.update(|cx| {
                                Toasts::show(Outcome::Failed, "toast-local-delete-failed", cx);
                            });
                        }
                        Ok(_) => {}
                    }
                })
                .detach();
            },
            cx,
        );
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending = None;
        self.grab = false;
        if let Some(focus) = self.restore.take() {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.grab = false;
        if let Some(focus) = self.restore.take() {
            window.focus(&focus, cx);
        }
        (pending.apply)(cx);
        cx.notify();
    }
}

impl Render for Confirm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(pending) = self.pending.as_ref() else {
            return div().into_any_element();
        };
        if self.grab {
            self.restore = window.focused(cx);
            window.focus(&self.focus, cx);
            self.grab = false;
        }

        let title = pending.kind.title();
        let detail = pending.kind.detail();
        let action = pending.kind.action();

        div()
            .absolute()
            .inset_0()
            .key_context(FORM_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
                cx.stop_propagation();
                this.close(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Submit, window, cx| {
                cx.stop_propagation();
                this.apply(window, cx);
            }))
            .child(
                Modal::new("confirm-remove", title)
                    .detail(detail)
                    .action(
                        Button::new("cancel-confirm")
                            .ghost()
                            .label(t!("common-cancel"))
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                    )
                    .action(
                        Button::new("apply-confirm")
                            .destructive()
                            .label(action)
                            .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                    )
                    .on_dismiss(cx.listener(|this, _, window, cx| this.close(window, cx))),
            )
            .into_any_element()
    }
}
