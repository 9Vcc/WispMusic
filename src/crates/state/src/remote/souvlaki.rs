use std::ffi::c_void;
use std::time::Duration;

use anyhow::{Result, anyhow};
use gpui::App;
use music::Track;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use tokio::sync::mpsc;

use super::{BUS_NAME, Command, DISPLAY_NAME};
use crate::{PlaybackState, Repeat};

const SEEK_STEP: Duration = Duration::from_secs(5);

/// The macOS Now Playing center or the Windows transport controls, through `souvlaki`, which
/// has no shuffle, repeat or volume there.
pub struct Controls {
    #[cfg(target_os = "windows")]
    sender: std::sync::mpsc::Sender<PlatformCommand>,
    #[cfg(not(target_os = "windows"))]
    controls: MediaControls,
}

#[cfg(target_os = "windows")]
enum PlatformCommand {
    Describe {
        title: Option<String>,
        artist: Option<String>,
        album: Option<String>,
        duration: Option<Duration>,
        cover: Option<String>,
    },
    Playback {
        state: PlaybackState,
        at: Duration,
    },
}

impl Controls {
    pub fn new(
        hwnd: Option<*mut c_void>,
        commands: mpsc::UnboundedSender<Command>,
        _cx: &mut App,
    ) -> Result<Self> {
        #[cfg(target_os = "windows")]
        {
            // The Windows media-control calls synchronously deliver shell input to the window.
            // Calling them while GPUI is updating an entity re-enters the window procedure and
            // trips `RefCell already borrowed`; keep the COM object on its own thread instead.
            let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
            let (sender, receiver) = std::sync::mpsc::channel();
            let commands = commands;
            let hwnd = hwnd.map(|handle| handle as usize);
            std::thread::Builder::new()
                .name("wisp-media-controls".to_owned())
                .spawn(move || {
                    let config = PlatformConfig {
                        dbus_name: BUS_NAME,
                        display_name: DISPLAY_NAME,
                        hwnd: hwnd.map(|handle| handle as *mut c_void),
                    };
                    let mut controls = match MediaControls::new(config) {
                        Ok(controls) => controls,
                        Err(error) => {
                            let _ = ready_tx.send(Err(anyhow!("{error:?}")));
                            return;
                        }
                    };
                    if let Err(error) = controls.attach(move |event| {
                        if let Some(command) = command(event) {
                            commands.send(command).ok();
                        }
                    }) {
                        log::warn!("remote: cannot listen for media keys: {error:?}");
                    }
                    let _ = ready_tx.send(Ok(()));
                    while let Ok(command) = receiver.recv() {
                        match command {
                            PlatformCommand::Describe {
                                title,
                                artist,
                                album,
                                duration,
                                cover,
                            } => {
                                let metadata = MediaMetadata {
                                    title: title.as_deref(),
                                    artist: artist.as_deref(),
                                    album: album.as_deref(),
                                    duration,
                                    cover_url: cover.as_deref(),
                                };
                                if let Err(error) = controls.set_metadata(metadata) {
                                    log::warn!(
                                        "remote: cannot publish the current track: {error:?}"
                                    );
                                }
                            }
                            PlatformCommand::Playback { state, at } => {
                                let progress = Some(MediaPosition(at));
                                let playback = match &state {
                                    PlaybackState::Playing | PlaybackState::Loading => {
                                        MediaPlayback::Playing { progress }
                                    }
                                    PlaybackState::Paused => MediaPlayback::Paused { progress },
                                    PlaybackState::Idle | PlaybackState::Failed(_) => {
                                        MediaPlayback::Stopped
                                    }
                                };
                                if let Err(error) = controls.set_playback(playback) {
                                    log::warn!("remote: cannot publish playback state: {error:?}");
                                }
                            }
                        }
                    }
                })?;
            ready_rx.recv()??;
            Ok(Self { sender })
        }

        #[cfg(not(target_os = "windows"))]
        {
            let config = PlatformConfig {
                dbus_name: BUS_NAME,
                display_name: DISPLAY_NAME,
                hwnd,
            };
            let mut controls = MediaControls::new(config).map_err(|error| anyhow!("{error:?}"))?;
            if let Err(error) = controls.attach(move |event| {
                if let Some(command) = command(event) {
                    commands.send(command).ok();
                }
            }) {
                log::warn!("remote: cannot listen for media keys: {error:?}");
            }
            Ok(Self { controls })
        }
    }

    pub fn describe(&mut self, track: Option<&Track>, cover: Option<&str>, _cx: &App) {
        #[cfg(target_os = "windows")]
        {
            let command = PlatformCommand::Describe {
                title: track.map(|track| track.name.clone()),
                artist: track.map(|track| track.artists.clone()),
                album: track.map(|track| track.album.clone()),
                duration: track.map(|track| track.duration),
                cover: cover.map(str::to_owned),
            };
            if self.sender.send(command).is_err() {
                log::warn!("remote: media controls thread stopped");
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let metadata = match track {
                Some(track) => MediaMetadata {
                    title: Some(&track.name),
                    artist: Some(&track.artists),
                    album: Some(&track.album),
                    duration: Some(track.duration),
                    cover_url: cover,
                },
                None => MediaMetadata::default(),
            };
            if let Err(error) = self.controls.set_metadata(metadata) {
                log::warn!("remote: cannot publish the current track: {error:?}");
            }
        }
    }

    pub fn set_playback(&mut self, state: &PlaybackState, at: Duration) {
        #[cfg(target_os = "windows")]
        {
            let command = PlatformCommand::Playback {
                state: state.clone(),
                at,
            };
            if self.sender.send(command).is_err() {
                log::warn!("remote: media controls thread stopped");
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let progress = Some(MediaPosition(at));
            let playback = match state {
                PlaybackState::Playing | PlaybackState::Loading => {
                    MediaPlayback::Playing { progress }
                }
                PlaybackState::Paused => MediaPlayback::Paused { progress },
                PlaybackState::Idle | PlaybackState::Failed(_) => MediaPlayback::Stopped,
            };
            if let Err(error) = self.controls.set_playback(playback) {
                log::warn!("remote: cannot publish playback state: {error:?}");
            }
        }
    }

    pub fn seeked(&mut self, _at: Duration) {}

    pub fn set_volume(&mut self, _level: f64) {}

    pub fn set_shuffle(&mut self, _on: bool) {}

    pub fn set_repeat(&mut self, _repeat: Repeat) {}
}

fn command(event: MediaControlEvent) -> Option<Command> {
    Some(match event {
        MediaControlEvent::Play => Command::Play,
        MediaControlEvent::Pause | MediaControlEvent::Stop => Command::Pause,
        MediaControlEvent::Toggle => Command::Toggle,
        MediaControlEvent::Next => Command::Next,
        MediaControlEvent::Previous => Command::Previous,
        MediaControlEvent::SetPosition(MediaPosition(at)) => Command::Seek(at),
        MediaControlEvent::Seek(direction) => step(direction, SEEK_STEP),
        MediaControlEvent::SeekBy(direction, by) => step(direction, by),
        MediaControlEvent::SetVolume(level) => Command::Volume(level),
        _ => return None,
    })
}

fn step(direction: SeekDirection, by: Duration) -> Command {
    match direction {
        SeekDirection::Forward => Command::Forward(by),
        SeekDirection::Backward => Command::Back(by),
    }
}
