//! The main window's title on Windows. The shell shows that title in the taskbar hover preview,
//! the taskbar tooltip and Alt+Tab, so while a song plays it names the song and its artist.
//!
//! The title follows the track `Playback` shows, the same one the player bar and the thumbnail
//! toolbar read. It is written again only when the text changes, and it goes back to the app
//! name once playback stops.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AnyWindowHandle, App, Global, Subscription};
use music::Track;
use state::Wisp;

/// The name the window carries when nothing plays. It matches the title the window opens with,
/// which is untranslated too.
const APP_NAME: &str = "Wisp";

/// Holds the playback watch for the open window. Installing again for another window drops the
/// old watch, so only the window on screen gets retitled.
struct Installed {
    _watch: Subscription,
}

impl Global for Installed {}

/// Keeps the title of `window` in step with the track playback shows. Call it once per window
/// open, after the window exists, so a window reopened from the tray picks up the current song.
pub fn install(window: AnyWindowHandle, cx: &mut App) {
    let playback = Wisp::global(cx).playback.clone();
    let last = Rc::new(RefCell::new(Last::default()));
    let hwnd = native_handle(window, cx);
    // The window is still being built here, so the first write waits until the frame is up.
    let first = Rc::clone(&last);
    cx.defer(move |cx| publish(window, hwnd, &first, cx));
    let watch = cx.observe(&playback, move |_, cx| publish(window, hwnd, &last, cx));
    cx.set_global(Installed { _watch: watch });
}

#[cfg(target_os = "windows")]
fn native_handle(window: AnyWindowHandle, cx: &mut App) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    window
        .update(cx, |_, window, _| {
            let RawWindowHandle::Win32(handle) =
                HasWindowHandle::window_handle(window).ok()?.as_raw()
            else {
                return None;
            };
            Some(handle.hwnd.get() as *mut std::ffi::c_void)
        })
        .ok()
        .flatten()
}

#[cfg(not(target_os = "windows"))]
fn native_handle(window: AnyWindowHandle, cx: &mut App) -> Option<*mut std::ffi::c_void> {
    let _ = (window, cx);
    None
}

fn write_title(hwnd: Option<*mut std::ffi::c_void>, title: &str) -> bool {
    #[cfg(target_os = "windows")]
    if let Some(hwnd) = hwnd {
        let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        return unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(hwnd) != 0
                && windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                    hwnd,
                    wide.as_ptr(),
                ) != 0
        };
    }
    false
}

/// The title the main window carries for `track`: the song, its artist and the app name while a
/// song plays, and the app name alone otherwise. This is the one place the format lives.
pub fn title(track: Option<&Track>) -> String {
    match track {
        Some(track) if track.artists.is_empty() => format!("{} - {APP_NAME}", track.name),
        Some(track) => format!("{} - {} - {APP_NAME}", track.name, track.artists),
        None => APP_NAME.to_owned(),
    }
}

/// The title the window was last given, so a playback change that alters nothing visible costs
/// no window call.
#[derive(Default)]
struct Last(Option<String>);

impl Last {
    /// Returns the title for `track` when it differs from the one recorded, and records it.
    /// Returns `None` when the window already shows that title.
    fn change(&mut self, track: Option<&Track>) -> Option<String> {
        let title = title(track);
        if self.0.as_deref() == Some(title.as_str()) {
            return None;
        }
        self.0 = Some(title.clone());
        Some(title)
    }

    /// Forgets the recorded title, so the next change is written even when it matches.
    fn forget(&mut self) {
        self.0 = None;
    }
}

/// Writes the title for the track playback shows now, if it differs from the last one written.
/// A failed write forgets the recorded title so the next playback change tries again.
fn publish(window: AnyWindowHandle, hwnd: Option<*mut std::ffi::c_void>, last: &Rc<RefCell<Last>>, cx: &mut App) {
    let track = Wisp::global(cx).playback.read(cx).shown_track(cx);
    let Some(title) = last.borrow_mut().change(track.as_ref()) else {
        return;
    };
    // Playback observations can run while the window is mid-draw. Defer the title write one
    // turn, and on Windows go straight to the HWND so GPUI never has to lend the window out.
    let last = Rc::clone(last);
    cx.defer(move |cx| {
        if write_title(hwnd, &title) {
            return;
        }
        if let Err(error) = window.update(cx, |_, window, _| window.set_window_title(&title)) {
            log::debug!("window title: cannot reach the main window: {error:#}");
            last.borrow_mut().forget();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(name: &str, artists: &str) -> Track {
        Track {
            name: name.to_owned(),
            artists: artists.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn a_playing_song_shows_its_title_and_artist_before_the_app_name() {
        let track = track("Blue Monday", "New Order");
        assert_eq!(title(Some(&track)), "Blue Monday - New Order - Wisp");
    }

    #[test]
    fn a_song_without_an_artist_shows_its_title_and_the_app_name() {
        let track = track("Blue Monday", "");
        assert_eq!(title(Some(&track)), "Blue Monday - Wisp");
    }

    #[test]
    fn nothing_playing_shows_the_app_name() {
        assert_eq!(title(None), "Wisp");
    }

    #[test]
    fn the_title_follows_each_track_change_and_writes_only_when_it_differs() {
        let mut last = Last::default();
        let first = track("Blue Monday", "New Order");
        let second = track("Dreams", "Fleetwood Mac");

        assert_eq!(
            last.change(Some(&first)).as_deref(),
            Some("Blue Monday - New Order - Wisp"),
        );
        assert_eq!(last.change(Some(&first)), None);
        assert_eq!(
            last.change(Some(&second)).as_deref(),
            Some("Dreams - Fleetwood Mac - Wisp"),
        );
    }

    #[test]
    fn stopping_playback_drops_the_last_song_from_the_title() {
        let mut last = Last::default();
        let song = track("Blue Monday", "New Order");

        assert!(last.change(Some(&song)).is_some());
        assert_eq!(last.change(None).as_deref(), Some("Wisp"));
        assert_eq!(last.change(None), None);
    }

    #[test]
    fn a_forgotten_title_is_written_again_even_when_it_matches() {
        let mut last = Last::default();
        let song = track("Blue Monday", "New Order");

        assert!(last.change(Some(&song)).is_some());
        last.forget();
        assert_eq!(
            last.change(Some(&song)).as_deref(),
            Some("Blue Monday - New Order - Wisp"),
        );
    }
}
