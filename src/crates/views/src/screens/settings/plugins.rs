use std::path::{Path, PathBuf};

use gpui::prelude::*;
use gpui::{AnyElement, Context, ExternalPaths, PathPromptOptions, SharedString, div, px, svg};
use i18n::{localized, t};
use music::plugins::{PluginHealth, PluginInfo};
use state::{Outcome, Toasts, Wisp};
use ui::{ActiveTheme as _, Button, Modal, Switch, Text};

use super::SettingsView;
use crate::shared::confirm::{Confirm, Kind};
use crate::shared::{provider_brand, provider_logo};

impl SettingsView {
    /// The installed provider plugins, with the built-in fallback marked as such. Installing
    /// needs a trusted signing key and is not offered here yet; rollback and uninstall act on
    /// the version directories directly and refresh this list.
    pub(super) fn plugins_rows(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let entries: Vec<PluginInfo> = Wisp::global(cx)
            .plugins
            .read(cx)
            .catalog()
            .all()
            .cloned()
            .collect();
        let mut rows = Vec::new();
        for plugin in entries {
            let name = localized(plugin.name, &plugin.name_i18n.others);
            let mark = provider_logo(&plugin.slug, cx);
            let brand = provider_brand(&plugin.slug, cx);
            let mut meta = match Wisp::global(cx).plugins.read(cx).health(&plugin.slug) {
                Some(PluginHealth::Quarantined(_)) => format!(
                    "v{} · ABI {} · {}",
                    plugin.version,
                    plugin.abi,
                    t!("settings-plugin-quarantined")
                ),
                Some(PluginHealth::RolledBack) => format!(
                    "v{} · ABI {} · {}",
                    plugin.version,
                    plugin.abi,
                    t!("settings-plugin-rolled-back")
                ),
                _ => format!(
                    "v{} · ABI {} · {}",
                    plugin.version, plugin.abi, plugin.publisher
                ),
            };
            if Wisp::global(cx)
                .plugins
                .read(cx)
                .sign_in_required(&plugin.slug)
            {
                meta.push_str(&format!(" · {}", t!("settings-plugin-needs-sign-in")));
            }
            let actions = {
                let mut actions = div().flex().flex_none().items_center().gap_1();
                let enabled = plugin.enabled;
                actions = actions.child(
                    Switch::new(
                        SharedString::from(format!("plugin-enabled-{}", plugin.slug)),
                        enabled,
                    )
                    .name(t!("settings-plugin-toggle", name = name))
                    .on_click({
                        let plugins = Wisp::global(cx).plugins.clone();
                        let slug = plugin.slug.clone();
                        move |_, _, cx| {
                            let plugins = plugins.clone();
                            let slug = slug.clone();
                            plugins.update(cx, |plugins, cx| {
                                match plugins.set_enabled(&slug, !enabled, cx) {
                                    Ok(()) => Toasts::show(
                                        Outcome::Done,
                                        match enabled {
                                            true => "toast-plugin-disabled",
                                            false => "toast-plugin-enabled",
                                        },
                                        cx,
                                    ),
                                    Err(error) => {
                                        log::warn!(
                                            "plugins: cannot toggle {slug}: {error:#}"
                                        );
                                        Toasts::show(
                                            Outcome::Failed,
                                            "toast-plugin-failed",
                                            cx,
                                        );
                                    }
                                }
                                cx.notify();
                            });
                        }
                    }),
                );
                if let Some(previous) = plugin.previous.clone() {
                    actions = actions.child(
                        Button::new(SharedString::from(format!(
                            "plugin-rollback-{}",
                            plugin.slug
                        )))
                        .label(t!("settings-plugin-rollback"))
                        .small()
                        .outline()
                        .on_click({
                            let plugins = Wisp::global(cx).plugins.clone();
                            let slug = plugin.slug.clone();
                            let name = SharedString::from(name);
                            move |_, _, cx| {
                                let plugins = plugins.clone();
                                let slug = slug.clone();
                                let name = name.clone();
                                let previous = previous.clone();
                                Confirm::ask(
                                    Kind::PluginRollback(name, previous.into()),
                                    move |cx| {
                                        plugins.update(cx, |plugins, cx| {
                                            match plugins.rollback(&slug, cx) {
                                                Ok(_) => Toasts::show(
                                                    Outcome::Done,
                                                    "toast-plugin-rolled-back",
                                                    cx,
                                                ),
                                                Err(error) => {
                                                    log::warn!(
                                                        "plugins: cannot roll {slug} back: {error:#}"
                                                    );
                                                    Toasts::show(
                                                        Outcome::Failed,
                                                        "toast-plugin-failed",
                                                        cx,
                                                    );
                                                }
                                            }
                                            cx.notify();
                                        });
                                    },
                                    cx,
                                );
                            }
                        }),
                    );
                }
                actions.child(
                    Button::new(SharedString::from(format!(
                        "plugin-uninstall-{}",
                        plugin.slug
                    )))
                    .ghost()
                    .icon("icons/trash-2.svg")
                    .size(theme.metrics.control_small)
                    .px_0()
                    .tooltip("settings-plugin-uninstall")
                    .tint(theme.muted_foreground)
                    .on_click({
                        let plugins = Wisp::global(cx).plugins.clone();
                        let slug = plugin.slug.clone();
                        let name = SharedString::from(name);
                        move |_, _, cx| {
                            let plugins = plugins.clone();
                            let slug = slug.clone();
                            let name = name.clone();
                            Confirm::ask(
                                Kind::PluginUninstall(name),
                                move |cx| {
                                    plugins.update(cx, |plugins, cx| {
                                        match plugins.uninstall(&slug, cx) {
                                            Ok(()) => Toasts::show(
                                                Outcome::Done,
                                                "toast-plugin-uninstalled",
                                                cx,
                                            ),
                                            Err(error) => {
                                                log::warn!(
                                                    "plugins: cannot uninstall {slug}: {error:#}"
                                                );
                                                Toasts::show(
                                                    Outcome::Failed,
                                                    "toast-plugin-failed",
                                                    cx,
                                                );
                                            }
                                        }
                                        cx.notify();
                                    });
                                },
                                cx,
                            );
                        }
                    }),
                )
                .into_any_element()
            };
            rows.push(
                div()
                    .id(SharedString::from(format!("plugin-row-{}", plugin.slug)))
                    .flex()
                    .items_center()
                    .gap_3()
                    .min_h(theme.metrics.list_row)
                    .child(
                        div().flex_none().size(px(22.)).child(
                            svg()
                                .path(icons::path(mark))
                                .size(px(22.))
                                .text_color(brand),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().child(SharedString::from(name)))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(theme.text(Text::Small))
                                    .text_color(theme.muted_foreground)
                                    .child(meta),
                            ),
                    )
                    .child(actions)
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .text_size(theme.text(Text::Small))
                    .text_color(theme.muted_foreground)
                    .child(t!("settings-plugins-empty"))
                    .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(rows)
            .on_drop(|paths: &ExternalPaths, _, cx| {
                let packages: Vec<PathBuf> = paths
                    .paths()
                    .iter()
                    .filter(|path| is_wisp(path))
                    .cloned()
                    .collect();
                if packages.is_empty() {
                    Toasts::show(Outcome::Failed, "toast-plugin-wisp-only", cx);
                    return;
                }
                install_packages(packages, cx);
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("plugin-install")
                            .label(t!("settings-plugin-install"))
                            .icon("icons/plus.svg")
                            .small()
                            .outline()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.plugin_install_open = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_size(theme.text(Text::Small))
                            .text_color(theme.muted_foreground)
                            .child(t!("settings-plugin-install-detail")),
                    ),
            )
            .into_any_element()
    }

    /// The install dialog: a drop target for a `.wisp` package, with a plus that opens the
    /// file chooser, so installing does not begin with a system dialog the user did not ask
    /// for.
    pub(super) fn plugin_install_modal(&self, cx: &mut Context<Self>) -> Option<Modal> {
        if !self.plugin_install_open {
            return None;
        }
        let theme = *cx.theme();
        Some(
            Modal::new(
                "settings-plugin-install",
                t!("settings-plugin-install-title"),
            )
            .w(px(520.))
            .detail(t!("settings-plugin-install-note"))
            .child(
                div()
                    .id("plugin-install-drop")
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .h(px(190.))
                    .rounded(theme.radius)
                    .border_1()
                    .border_dashed()
                    .border_color(theme.border)
                    .bg(theme.secondary)
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        this.land_plugin(paths, cx)
                    }))
                    .child(
                        Button::new("plugin-install-pick")
                            .icon("icons/plus.svg")
                            .name(t!("settings-plugin-install-pick"))
                            .tooltip("settings-plugin-install-pick")
                            .size(px(44.))
                            .rounded_full()
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| this.pick_plugin(cx))),
                    )
                    .child(
                        div()
                            .text_color(theme.foreground)
                            .child(t!("settings-plugin-install-drop")),
                    )
                    .child(
                        div()
                            .text_size(theme.text(Text::Small))
                            .text_color(theme.muted_foreground)
                            .child(t!("settings-plugin-install-pick")),
                    ),
            )
            .action(
                Button::new("plugin-install-cancel")
                    .ghost()
                    .label(t!("common-cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_plugin_install(cx))),
            )
            .on_dismiss(cx.listener(|this, _, _, cx| this.close_plugin_install(cx))),
        )
    }

    /// Opens the system file chooser from the dialog's plus. A picked package installs at
    /// once, as a dropped one does.
    fn pick_plugin(&mut self, cx: &mut Context<Self>) {
        let package_receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t!("settings-plugin-open-package")),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = package_receiver.await else {
                return;
            };
            let packages: Vec<PathBuf> = paths.into_iter().filter(|path| is_wisp(path)).collect();
            this.update(cx, |this, cx| {
                install_packages(packages, cx);
                this.plugin_install_open = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Installs every `.wisp` among the dropped paths and closes the dialog; a drop with no
    /// package keeps it open and says what it wants.
    fn land_plugin(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let packages: Vec<PathBuf> = paths
            .paths()
            .iter()
            .filter(|path| is_wisp(path))
            .cloned()
            .collect();
        if packages.is_empty() {
            Toasts::show(Outcome::Failed, "toast-plugin-wisp-only", cx);
            return;
        }
        log::info!("plugins: installing {} dropped packages", packages.len());
        install_packages(packages, cx);
        self.plugin_install_open = false;
        cx.notify();
    }

    fn close_plugin_install(&mut self, cx: &mut Context<Self>) {
        self.plugin_install_open = false;
        cx.notify();
    }
}

fn install_packages(packages: Vec<PathBuf>, cx: &mut gpui::App) {
    if packages.is_empty() {
        Toasts::show(Outcome::Failed, "toast-plugin-wisp-only", cx);
        return;
    }
    let plugins = Wisp::global(cx).plugins.clone();
    plugins.update(cx, |plugins, cx| {
        plugins.install_many_async(packages, cx);
    });
}

fn is_wisp(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wisp"))
}
