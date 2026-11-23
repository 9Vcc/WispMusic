use gpui::prelude::*;
use gpui::{AnyElement, App, Context, Entity, PathPromptOptions, SharedString, div};
use i18n::t;
use state::{Outcome, Playback, Toasts};
use ui::{ActiveTheme as _, Button};

use super::Row;
use super::{Group, SettingsView, Slot};

/// The bytes in a gigabyte, the unit the cache limit is set in.
const GIGABYTE: u64 = 1 << 30;
const MEGABYTE: u64 = 1 << 20;

impl SettingsView {
    /// The storage section: the cache of played tracks with what it takes and a way to empty it.
    /// The cache settings show only for a provider whose played tracks are kept; the space it
    /// takes always shows, since files stay on disk across providers.
    pub(super) fn storage_groups(&self, cx: &App) -> Vec<Group> {
        let capabilities = self.session.read(cx).capabilities();
        let mut cache = Vec::new();
        if capabilities.keeps {
            cache.push(Slot::KeepTracks);
            if self.playback.read(cx).keep_tracks() {
                cache.push(Slot::CacheSize);
                cache.push(Slot::CacheDir);
            }
        }
        cache.push(Slot::CacheUsage);
        vec![Group::new("settings-group-cache", "icons/rotate-ccw-clock.svg").rows(cache)]
    }

    /// [wisp] 缓存位置：当前选择（或默认位置），可以换一个文件夹或恢复默认。换好后
    /// 新缓存写入新位置，旧目录里的文件留在原处，并由 toast 提示手动清理。
    pub(super) fn cache_dir_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let chosen = self.settings.read(cx).cache_dir().map(str::to_owned);
        let shown: SharedString = match &chosen {
            Some(dir) => SharedString::from(dir.clone()),
            None => t!("settings-cache-dir-default"),
        };
        let playback = self.playback.clone();
        let control = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .max_w(gpui::px(260.))
                    .truncate()
                    .child(ui::spoken("cache-dir-path", shown).text_color(theme.muted_foreground)),
            )
            .child(
                Button::new("cache-dir-choose")
                    .label(t!("settings-choose-folder"))
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| choose_cache_dir(playback.clone(), cx)),
            )
            .when(chosen.is_some(), |row| {
                row.child(
                    Button::new("cache-dir-reset")
                        .label(t!("settings-cache-dir-reset"))
                        .small()
                        .outline()
                        .on_click(cx.listener(|this, _, _, cx| {
                            let old = this.playback.update(cx, |playback, cx| {
                                playback.set_cache_dir(None, cx)
                            });
                            if let Some(old) = old {
                                Toasts::about(
                                    Outcome::Info,
                                    "toast-cache-dir-moved",
                                    old.display().to_string(),
                                    cx,
                                );
                            }
                        })),
                )
            });

        Row::new(t!("settings-cache-dir"), control.into_any_element())
            .detail(t!("settings-cache-dir-detail"))
            .yielding()
            .into_any_element()
    }

    /// What the cached tracks take on disk, with a button that removes them all.
    pub(super) fn cache_usage_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let used = self.playback.read(cx).usage();
        let control = div()
            .flex()
            .items_center()
            .gap_3()
            .child(ui::spoken("cache-used", size(used)).text_color(theme.muted_foreground))
            .child(
                Button::new("clear-cache")
                    .label(t!("settings-cache-clear"))
                    .small()
                    .outline()
                    .disabled(used.is_none_or(|used| used == 0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.playback
                            .update(cx, |playback, cx| playback.clear_cache(cx));
                    })),
            );
        self.row(t!("settings-cache-used"), control.into_any_element())
    }
}

/// [wisp] 选一个缓存目录，交给 playback 立即重建 vault。
fn choose_cache_dir(playback: Entity<Playback>, cx: &mut App) {
    let receiver = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = receiver.await else {
            return;
        };
        let Some(dir) = paths.into_iter().next() else {
            return;
        };
        let old = playback.update(cx, |playback, cx| playback.set_cache_dir(Some(dir), cx));
        if let Some(old) = old {
            cx.update(|cx| {
                Toasts::about(
                    Outcome::Info,
                    "toast-cache-dir-moved",
                    old.display().to_string(),
                    cx,
                )
            });
        }
    })
    .detach();
}

/// A size in bytes as a short label: gigabytes to one decimal from a gigabyte up, whole
/// megabytes below. Nothing measured yet reads as measuring.
fn size(bytes: Option<u64>) -> SharedString {
    let Some(bytes) = bytes else {
        return t!("settings-size-measuring");
    };
    let mut args = i18n::FluentArgs::new();
    match bytes >= GIGABYTE {
        true => {
            args.set("size", format!("{:.1}", bytes as f64 / GIGABYTE as f64));
            i18n::lookup("settings-size-gb", Some(&args))
        }
        false => {
            args.set("size", (bytes / MEGABYTE) as i64);
            i18n::lookup("settings-size-mb", Some(&args))
        }
    }
}
