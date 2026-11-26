//! A provider backed entirely by a flat `.wisp` plugin.
//!
//! Every operation is a wasm op, and an op the plugin does not answer fails. The host no longer
//! keeps a native provider fallback; this adapter is also the path a third-party provider uses.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use rodio::Source as _;

use super::runtime::{PluginHealth, WasmPlugin};
use crate::engine::{self, Fetch};
use crate::ranged::{self, Address, Reach};
use crate::speed::{SpeedControl, SpeedSource};
use crate::stream::{Plain, Reader, Stream};
use crate::vault::{Recording, Shelf};
use crate::{
    Album, AlbumCatalogue, AlbumDetail, Artist, ArtistCatalogue, ArtistProfile, Capabilities,
    ClimaxMark, Encoding, Feed, InputSource, MediaKind, MusicApi, MusicProvider, Offer,
    PlaybackConfig, PlaybackEvents, PlaybackFactory, Player, PromptSink, ProviderSession, Quality,
    Shape, SignIn, SignInPrompt, SmartPlaylist, SmartPlaylistUpdate, SmartRules, Track, UserProfile,
    at_once,
};

#[derive(serde::Serialize, serde::Deserialize)]
struct Facts {
    duration_s: Option<u64>,
    level: String,
}

/// How many ordinary wasm calls may wait behind the one running before the queue is full.
const WASM_QUEUE_LIMIT: usize = 64;

type Reply = Box<dyn FnOnce(Result<String>) + Send + 'static>;

enum Work {
    Call {
        op: u32,
        arg: String,
        high: bool,
        reply: Reply,
    },
    SignIn {
        flow: u32,
        input: String,
        reply: Reply,
    },
}

#[derive(Default)]
struct WorkQueue {
    high: VecDeque<Work>,
    normal: VecDeque<Work>,
    closed: bool,
}

/// One wasm call at a time on a thread that stays up, instead of one OS thread per call. High
/// calls jump ahead of queued ordinary calls but never interrupt the call already running.
struct WasmWorker {
    queue: Arc<(Mutex<WorkQueue>, Condvar)>,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl WasmWorker {
    fn new(plugin: Arc<WasmPlugin>, slug: &'static str) -> Self {
        let queue = Arc::new((Mutex::new(WorkQueue::default()), Condvar::new()));
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let shared = Arc::clone(&queue);
        let started = std::thread::Builder::new()
            .name(format!("{slug}-wasm"))
            .spawn(move || {
                loop {
                    let work = {
                        let (lock, ready) = &*shared;
                        let mut queue = lock.lock().unwrap_or_else(|poison| poison.into_inner());
                        loop {
                            if queue.closed {
                                return;
                            }
                            if let Some(work) = queue.high.pop_front() {
                                break work;
                            }
                            if let Some(work) = queue.normal.pop_front() {
                                break work;
                            }
                            queue = ready
                                .wait(queue)
                                .unwrap_or_else(|poison| poison.into_inner());
                        }
                    };
                    match work {
                        Work::Call {
                            op,
                            arg,
                            high,
                            reply,
                        } => {
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                || match high {
                                    true => plugin.call_high(op, &arg),
                                    false => plugin.call(op, &arg),
                                },
                            ))
                            .unwrap_or_else(|_| {
                                Err(anyhow::anyhow!("the provider wasm call panicked"))
                            });
                            reply(result);
                        }
                        Work::SignIn { flow, input, reply } => {
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    plugin.sign_in_step(flow, &input)
                                }))
                                .unwrap_or_else(|_| {
                                    Err(anyhow::anyhow!("the provider sign-in call panicked"))
                                });
                            reply(result);
                        }
                    }
                }
            });
        if let Err(error) = started {
            log::error!("plugins: cannot start the wasm worker for {slug}: {error}");
            running.store(false, Ordering::Relaxed);
        }
        Self { queue, running }
    }

    fn submit(&self, high: bool, work: Work) -> Result<()> {
        if !self.running.load(Ordering::Relaxed) {
            anyhow::bail!("the provider wasm worker stopped");
        }
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|poison| poison.into_inner());
        if !high && queue.high.len() + queue.normal.len() >= WASM_QUEUE_LIMIT {
            anyhow::bail!("the provider wasm queue is full");
        }
        match high {
            true => queue.high.push_back(work),
            false => queue.normal.push_back(work),
        }
        ready.notify_one();
        Ok(())
    }

    fn call_blocking(&self, op: u32, arg: &str, high: bool) -> Option<Result<String>> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.submit(
            high,
            Work::Call {
                op,
                arg: arg.to_owned(),
                high,
                reply: Box::new(move |result| {
                    let _ = sender.send(result);
                }),
            },
        )
        .ok()?;
        receiver.recv().ok()
    }
}

impl Drop for WasmWorker {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|poison| poison.into_inner());
        queue.closed = true;
        ready.notify_all();
    }
}

/// A wasm-backed client. It owns the HTTP stack used for the bytes the plugin only describes;
/// plugin HTTP itself goes through the slot's shared host.
#[derive(Clone)]
pub struct WasmClient {
    plugin: Arc<WasmPlugin>,
    worker: Arc<WasmWorker>,
    http: reqwest::Client,
    slug: &'static str,
    me: Arc<AtomicI64>,
}

impl WasmClient {
    pub fn new(plugin: Arc<WasmPlugin>, slug: &'static str) -> Self {
        let worker = Arc::new(WasmWorker::new(Arc::clone(&plugin), slug));
        Self {
            plugin,
            worker,
            http: reqwest::Client::builder()
                .dns_resolver(crate::dns::dns())
                .user_agent("wisp-provider/0.51")
                .build()
                .unwrap_or_default(),
            slug,
            me: Arc::new(AtomicI64::new(0)),
        }
    }

    pub fn set_cookie(&self, cookie: &str) {
        self.plugin.set_cookie(cookie);
    }

    fn set_user_id(&self, id: &str) {
        self.me.store(id.parse().unwrap_or(0), Ordering::Relaxed);
    }

    pub(crate) async fn stream_plan(
        &self,
        track_id: &str,
        quality: &str,
    ) -> Result<wisp_plugin_api::StreamPlan> {
        let args = serde_json::json!({ "track_id": track_id, "quality": quality });
        let reply = self
            .call_high(
                wisp_plugin_api::ops::ext::STREAM_RESOLVE,
                &serde_json::to_string(&args)?,
            )
            .await?;
        parse_reply(&reply)
    }

    pub(crate) async fn open_stream(
        &self,
        url: &str,
        headers: &[wisp_plugin_api::Header],
    ) -> Result<reqwest::Response> {
        let request = self.http.get(url).headers(stream_headers(headers));
        let response = request
            .send()
            .await
            .context("cannot reach the provider stream")?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("the provider stream answered with status {status}");
        }
        Ok(response)
    }

    async fn call(&self, op: u32, arg: &str) -> Result<String> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.worker.submit(
            false,
            Work::Call {
                op,
                arg: arg.to_owned(),
                high: false,
                reply: Box::new(move |result| {
                    let _ = sender.send(result);
                }),
            },
        )?;
        receiver.await.context("the provider wasm worker stopped")?
    }

    async fn call_high(&self, op: u32, arg: &str) -> Result<String> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.worker.submit(
            true,
            Work::Call {
                op,
                arg: arg.to_owned(),
                high: true,
                reply: Box::new(move |result| {
                    let _ = sender.send(result);
                }),
            },
        )?;
        receiver.await.context("the provider wasm worker stopped")?
    }

    async fn sign_in_step(&self, flow: u32, input: &str) -> Result<wisp_plugin_api::SignInStep> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.worker.submit(
            false,
            Work::SignIn {
                flow,
                input: input.to_owned(),
                reply: Box::new(move |result| {
                    let _ = sender.send(result);
                }),
            },
        )?;
        let reply = receiver
            .await
            .context("the provider wasm worker stopped")??;
        parse_reply(&reply)
    }

    fn library_args(&self) -> String {
        serde_json::json!({ "me": self.me.load(Ordering::Relaxed) }).to_string()
    }

    async fn artist_call(&self, op: u32, artist_id: &str) -> Result<String> {
        let args = serde_json::to_string(&wisp_plugin_api::ArtistArgs {
            artist_id: artist_id.to_owned(),
        })?;
        self.call(op, &args).await
    }

    async fn ack(&self, op: u32, args: &str) -> Result<()> {
        let reply = self.call(op, args).await?;
        parse_reply::<wisp_plugin_api::AckReply>(&reply)?;
        Ok(())
    }
}

fn parse_reply<T: serde::de::DeserializeOwned>(reply: &str) -> Result<T> {
    let value: serde_json::Value = serde_json::from_str(reply).context("invalid wasm reply")?;
    if value.get("code").is_some() {
        let error: wisp_plugin_api::PluginError =
            serde_json::from_value(value).context("invalid wasm error")?;
        anyhow::bail!("{}", error.message);
    }
    serde_json::from_value(value).context("invalid wasm payload")
}

impl std::fmt::Debug for WasmClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmClient")
            .field("slug", &self.slug)
            .finish()
    }
}

#[async_trait]
impl MusicApi for WasmClient {
    fn share_url(&self, kind: MediaKind, id: &str) -> Option<String> {
        let kind = match kind {
            MediaKind::Track => wisp_plugin_api::MediaKindDto::Track,
            MediaKind::Album => wisp_plugin_api::MediaKindDto::Album,
            MediaKind::Artist => wisp_plugin_api::MediaKindDto::Artist,
            MediaKind::Playlist => wisp_plugin_api::MediaKindDto::Playlist,
        };
        let args = serde_json::to_string(&wisp_plugin_api::ShareUrlArgs {
            kind,
            id: id.to_owned(),
        })
        .ok()?;
        let reply = self
            .worker
            .call_blocking(wisp_plugin_api::ops::music_api::SHARE_URL, &args, false)?
            .ok()?;
        parse_reply::<wisp_plugin_api::ShareUrlReply>(&reply)
            .ok()?
            .url
    }

    fn cover_at(&self, cover: &str, size: crate::CoverSize) -> Option<String> {
        let size = match size {
            crate::CoverSize::Large => wisp_plugin_api::CoverSizeDto::Large,
            crate::CoverSize::Full => wisp_plugin_api::CoverSizeDto::Full,
        };
        let args = serde_json::to_string(&wisp_plugin_api::CoverAtArgs {
            cover: cover.to_owned(),
            size,
        })
        .ok()?;
        let reply = self
            .worker
            .call_blocking(wisp_plugin_api::ops::music_api::COVER_AT, &args, false)?
            .ok()?;
        parse_reply::<wisp_plugin_api::CoverAtReply>(&reply)
            .ok()?
            .cover
    }

    async fn profile(&self) -> Result<UserProfile> {
        let reply = self
            .call(wisp_plugin_api::ops::music_api::PROFILE, "{}")
            .await?;
        Ok(parse_reply::<wisp_plugin_api::ProfileReply>(&reply)?
            .profile
            .into())
    }

    async fn artist(&self, artist_id: &str) -> Result<Artist> {
        let reply = self
            .artist_call(wisp_plugin_api::ops::music_api::ARTIST, artist_id)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::ArtistReply>(&reply)?
            .artist
            .into())
    }

    async fn artist_profile(&self, artist_id: &str) -> Result<ArtistProfile> {
        let reply = self
            .artist_call(wisp_plugin_api::ops::music_api::ARTIST_PROFILE, artist_id)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::ArtistProfileReply>(&reply)?
            .profile
            .into())
    }

    async fn artist_images(&self, ids: Vec<String>) -> Result<HashMap<String, String>> {
        let args = serde_json::to_string(&wisp_plugin_api::ArtistImagesArgs { ids })?;
        let reply = self
            .call(wisp_plugin_api::ops::music_api::ARTIST_IMAGES, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::ArtistImagesReply>(&reply)?
            .images
            .into_iter()
            .collect())
    }

    async fn saved_tracks(&self) -> Result<Vec<Track>> {
        let reply = self
            .call(
                wisp_plugin_api::ops::music_api::SAVED_TRACKS,
                &self.library_args(),
            )
            .await?;
        let tracks: Vec<Track> = parse_reply::<wisp_plugin_api::SavedTracksReply>(&reply)?
            .tracks
            .into_iter()
            .map(Track::from)
            .collect();
        log::debug!(
            "provider {}: saved_tracks returned {} tracks",
            self.slug,
            tracks.len()
        );
        Ok(tracks)
    }

    async fn recently_played(&self) -> Result<Vec<Track>> {
        let reply = self
            .call(
                wisp_plugin_api::ops::music_api::RECENTLY_PLAYED,
                &self.library_args(),
            )
            .await?;
        Ok(parse_reply::<wisp_plugin_api::SavedTracksReply>(&reply)?
            .tracks
            .into_iter()
            .map(Track::from)
            .collect())
    }

    async fn set_track_saved(&self, track_id: &str, saved: bool) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::SetTrackSavedArgs {
            track_id: track_id.to_owned(),
            saved,
        })?;
        self.ack(wisp_plugin_api::ops::music_api::SET_TRACK_SAVED, &args)
            .await
    }

    async fn track(&self, track_id: &str) -> Result<Track> {
        let args = serde_json::to_string(&wisp_plugin_api::TrackArgs {
            track_id: track_id.to_owned(),
        })?;
        let reply = self
            .call_high(wisp_plugin_api::ops::music_api::TRACK, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::TrackReply>(&reply)?
            .track
            .into())
    }

    async fn track_playcount(&self, track_id: &str) -> Result<Option<u64>> {
        let args = serde_json::to_string(&wisp_plugin_api::TrackArgs {
            track_id: track_id.to_owned(),
        })?;
        let reply = self
            .call(wisp_plugin_api::ops::music_api::TRACK_PLAYCOUNT, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::TrackPlaycountReply>(&reply)?.playcount)
    }

    async fn playlists(&self) -> Result<Vec<crate::Playlist>> {
        let reply = self
            .call(
                wisp_plugin_api::ops::music_api::PLAYLISTS,
                &self.library_args(),
            )
            .await?;
        Ok(parse_reply::<wisp_plugin_api::PlaylistsReply>(&reply)?
            .playlists
            .into_iter()
            .map(crate::Playlist::from)
            .collect())
    }

    async fn smart_playlist(&self, playlist_id: &str) -> Result<SmartPlaylist> {
        let args = serde_json::to_string(&wisp_plugin_api::PlaylistIdArgs {
            playlist_id: playlist_id.to_owned(),
        })?;
        let reply = self
            .call(wisp_plugin_api::ops::music_api::SMART_PLAYLIST, &args)
            .await?;
        smart_playlist(parse_reply::<wisp_plugin_api::SmartPlaylistReply>(&reply)?.playlist)
    }

    async fn create_smart_playlist(&self, name: &str, rules: &SmartRules) -> Result<String> {
        let args = serde_json::to_string(&wisp_plugin_api::CreateSmartPlaylistArgs {
            name: name.to_owned(),
            rules: rules.to_wire().to_string(),
        })?;
        let reply = self
            .call(
                wisp_plugin_api::ops::music_api::CREATE_SMART_PLAYLIST,
                &args,
            )
            .await?;
        Ok(parse_reply::<wisp_plugin_api::CreatePlaylistReply>(&reply)?.playlist_id)
    }

    async fn update_smart_playlist(
        &self,
        playlist_id: &str,
        update: SmartPlaylistUpdate,
    ) -> Result<SmartPlaylist> {
        let args = serde_json::to_string(&wisp_plugin_api::UpdateSmartPlaylistArgs {
            playlist_id: playlist_id.to_owned(),
            name: update.name,
            comment: update.comment,
            public: update.public,
            rules: update.rules.map(|rules| rules.to_wire().to_string()),
        })?;
        let reply = self
            .call(
                wisp_plugin_api::ops::music_api::UPDATE_SMART_PLAYLIST,
                &args,
            )
            .await?;
        smart_playlist(parse_reply::<wisp_plugin_api::SmartPlaylistReply>(&reply)?.playlist)
    }

    async fn create_playlist(&self, name: &str) -> Result<String> {
        let args = serde_json::to_string(&wisp_plugin_api::CreatePlaylistArgs {
            name: name.to_owned(),
        })?;
        let reply = self
            .call(wisp_plugin_api::ops::music_api::CREATE_PLAYLIST, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::CreatePlaylistReply>(&reply)?.playlist_id)
    }

    async fn rename_playlist(&self, playlist_id: &str, name: &str) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::RenamePlaylistArgs {
            playlist_id: playlist_id.to_owned(),
            name: name.to_owned(),
        })?;
        self.ack(wisp_plugin_api::ops::music_api::RENAME_PLAYLIST, &args)
            .await
    }

    async fn delete_playlist(&self, playlist_id: &str) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::DeletePlaylistArgs {
            playlist_id: playlist_id.to_owned(),
        })?;
        self.ack(wisp_plugin_api::ops::music_api::DELETE_PLAYLIST, &args)
            .await
    }

    async fn remove_playlist_from_library(&self, playlist_id: &str) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::PlaylistIdArgs {
            playlist_id: playlist_id.to_owned(),
        })?;
        self.ack(
            wisp_plugin_api::ops::music_api::REMOVE_PLAYLIST_FROM_LIBRARY,
            &args,
        )
        .await
    }

    async fn add_playlist_to_library(&self, playlist_id: &str) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::PlaylistIdArgs {
            playlist_id: playlist_id.to_owned(),
        })?;
        self.ack(
            wisp_plugin_api::ops::music_api::ADD_PLAYLIST_TO_LIBRARY,
            &args,
        )
        .await
    }

    async fn set_playlist_public(&self, playlist_id: &str, public: bool) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::SetPlaylistPublicArgs {
            playlist_id: playlist_id.to_owned(),
            public,
        })?;
        self.ack(wisp_plugin_api::ops::music_api::SET_PLAYLIST_PUBLIC, &args)
            .await
    }

    async fn add_track_to_playlist(&self, playlist_id: &str, track_id: &str) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::AddTrackToPlaylistArgs {
            playlist_id: playlist_id.to_owned(),
            track_id: track_id.to_owned(),
        })?;
        self.ack(
            wisp_plugin_api::ops::music_api::ADD_TRACK_TO_PLAYLIST,
            &args,
        )
        .await
    }

    async fn remove_track_from_playlist(&self, playlist_id: &str, track_id: &str) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::RemoveTrackFromPlaylistArgs {
            playlist_id: playlist_id.to_owned(),
            track_id: track_id.to_owned(),
        })?;
        self.ack(
            wisp_plugin_api::ops::music_api::REMOVE_TRACK_FROM_PLAYLIST,
            &args,
        )
        .await
    }

    async fn reorder_playlist(
        &self,
        playlist_id: &str,
        seen: &[String],
        wanted: &[String],
    ) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::ReorderPlaylistArgs {
            playlist_id: playlist_id.to_owned(),
            seen: seen.to_vec(),
            wanted: wanted.to_vec(),
        })?;
        self.ack(wisp_plugin_api::ops::music_api::REORDER_PLAYLIST, &args)
            .await
    }

    async fn set_playlist_cover(&self, playlist_id: &str, image: Option<&[u8]>) -> Result<()> {
        use base64::Engine as _;
        let args = serde_json::to_string(&wisp_plugin_api::SetPlaylistCoverArgs {
            playlist_id: playlist_id.to_owned(),
            image_base64: image
                .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)),
        })?;
        self.ack(wisp_plugin_api::ops::music_api::SET_PLAYLIST_COVER, &args)
            .await
    }

    async fn report_play(&self, track_id: &str) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::TrackArgs {
            track_id: track_id.to_owned(),
        })?;
        self.ack(wisp_plugin_api::ops::music_api::REPORT_PLAY, &args)
            .await
    }

    async fn dislike(&self, track_id: &str) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::DislikeArgs {
            track_id: track_id.to_owned(),
        })?;
        self.ack(wisp_plugin_api::ops::music_api::DISLIKE, &args)
            .await
    }

    async fn saved_albums(&self) -> Result<Vec<Album>> {
        let reply = self
            .call(
                wisp_plugin_api::ops::music_api::SAVED_ALBUMS,
                &self.library_args(),
            )
            .await?;
        Ok(parse_reply::<wisp_plugin_api::SavedAlbumsReply>(&reply)?
            .albums
            .into_iter()
            .map(Album::from)
            .collect())
    }

    async fn set_album_saved(&self, album_id: &str, saved: bool) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::SetAlbumSavedArgs {
            album_id: album_id.to_owned(),
            saved,
        })?;
        self.ack(wisp_plugin_api::ops::music_api::SET_ALBUM_SAVED, &args)
            .await
    }

    async fn saved_artists(&self) -> Result<Vec<crate::SavedArtist>> {
        let reply = self
            .call(
                wisp_plugin_api::ops::music_api::SAVED_ARTISTS,
                &self.library_args(),
            )
            .await?;
        Ok(parse_reply::<wisp_plugin_api::SavedArtistsReply>(&reply)?
            .artists
            .into_iter()
            .map(crate::SavedArtist::from)
            .collect())
    }

    async fn set_artist_saved(&self, artist_id: &str, saved: bool) -> Result<()> {
        let args = serde_json::to_string(&wisp_plugin_api::SetArtistSavedArgs {
            artist_id: artist_id.to_owned(),
            saved,
        })?;
        self.ack(wisp_plugin_api::ops::music_api::SET_ARTIST_SAVED, &args)
            .await
    }

    async fn album(&self, album_id: &str) -> Result<AlbumDetail> {
        let args = serde_json::to_string(&wisp_plugin_api::AlbumArgs {
            album_id: album_id.to_owned(),
        })?;
        let reply = self
            .call(wisp_plugin_api::ops::music_api::ALBUM, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::AlbumDetailDto>(&reply)?.into())
    }

    async fn album_tracks(&self, album_id: &str) -> Result<Vec<Track>> {
        Ok(self.album(album_id).await?.tracks)
    }

    async fn playlist(&self, playlist_id: &str) -> Result<crate::PlaylistDetail> {
        let args = serde_json::to_string(&wisp_plugin_api::PlaylistArgs {
            playlist_id: playlist_id.to_owned(),
            me: 0,
        })?;
        let reply = self
            .call_high(wisp_plugin_api::ops::music_api::PLAYLIST, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::PlaylistDetailDto>(&reply)?.into())
    }

    async fn playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Track>> {
        let detail = self.playlist(playlist_id).await?;
        let mut tracks = detail.tracks;
        let mut continuation = detail.continuation;
        while let Some(token) = continuation {
            let (more, next) = self.playlist_continuation(&token).await?;
            if more.is_empty() && next.is_none() {
                break;
            }
            tracks.extend(more);
            continuation = next;
        }
        Ok(tracks)
    }

    /// The rest of a playlist whose first answer carried a continuation. A large playlist
    /// refills in provider pages, so an interactive call can cut in between them instead of
    /// waiting out the whole list.
    async fn playlist_continuation(
        &self,
        continuation: &str,
    ) -> Result<(Vec<Track>, Option<String>)> {
        let args = serde_json::to_string(&wisp_plugin_api::PlaylistContinuationArgs {
            continuation: continuation.to_owned(),
        })?;
        let reply = self
            .call(
                wisp_plugin_api::ops::music_api::PLAYLIST_CONTINUATION,
                &args,
            )
            .await?;
        let page = parse_reply::<wisp_plugin_api::PlaylistContinuationReply>(&reply)?;
        Ok((
            page.tracks.into_iter().map(Track::from).collect(),
            page.continuation,
        ))
    }

    async fn playlist_covers(&self, playlist_id: &str, wanted: usize) -> Result<Vec<String>> {
        let args = serde_json::to_string(&wisp_plugin_api::PlaylistCoversArgs {
            playlist_id: playlist_id.to_owned(),
            wanted,
        })?;
        let reply = self
            .call(wisp_plugin_api::ops::music_api::PLAYLIST_COVERS, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::PlaylistCoversReply>(&reply)?.covers)
    }

    async fn track_radio(
        &self,
        track_id: &str,
        from: Option<&str>,
    ) -> Result<(Vec<Track>, Option<String>)> {
        self.track_radio_with(track_id, from, None, None).await
    }

    async fn track_radio_with(
        &self,
        track_id: &str,
        from: Option<&str>,
        mode: Option<&str>,
        song_pool_id: Option<u32>,
    ) -> Result<(Vec<Track>, Option<String>)> {
        let args = serde_json::to_string(&wisp_plugin_api::TrackRadioArgs {
            track_id: track_id.to_owned(),
            from: from.map(str::to_owned),
            mode: mode.map(str::to_owned),
            song_pool_id,
        })?;
        let reply = self
            .call(wisp_plugin_api::ops::music_api::TRACK_RADIO, &args)
            .await?;
        let reply = parse_reply::<wisp_plugin_api::TrackRadioReply>(&reply)?;
        Ok((
            reply.tracks.into_iter().map(Track::from).collect(),
            reply.continuation,
        ))
    }

    async fn search(&self, query: &str) -> Result<Vec<Track>> {
        let args = serde_json::to_string(&wisp_plugin_api::SearchArgs {
            query: query.to_owned(),
        })?;
        let reply = self
            .call_high(wisp_plugin_api::ops::music_api::SEARCH, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::SearchReply>(&reply)?
            .tracks
            .into_iter()
            .map(Track::from)
            .collect())
    }

    async fn search_albums(&self, query: &str) -> Result<Vec<Album>> {
        let args = serde_json::to_string(&wisp_plugin_api::SearchArgs {
            query: query.to_owned(),
        })?;
        let reply = self
            .call_high(wisp_plugin_api::ops::music_api::SEARCH_ALBUMS, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::SearchAlbumsReply>(&reply)?
            .albums
            .into_iter()
            .map(Album::from)
            .collect())
    }

    async fn search_playlists(&self, query: &str) -> Result<Vec<crate::Playlist>> {
        let args = serde_json::to_string(&wisp_plugin_api::SearchPlaylistsArgs {
            query: query.to_owned(),
            me: 0,
        })?;
        let reply = self
            .call_high(wisp_plugin_api::ops::music_api::SEARCH_PLAYLISTS, &args)
            .await?;
        Ok(
            parse_reply::<wisp_plugin_api::SearchPlaylistsReply>(&reply)?
                .playlists
                .into_iter()
                .map(crate::Playlist::from)
                .collect(),
        )
    }

    async fn search_artists(&self, query: &str) -> Result<Vec<crate::SavedArtist>> {
        let args = serde_json::to_string(&wisp_plugin_api::SearchArgs {
            query: query.to_owned(),
        })?;
        let reply = self
            .call_high(wisp_plugin_api::ops::music_api::SEARCH_ARTISTS, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::SearchArtistsReply>(&reply)?
            .artists
            .into_iter()
            .map(crate::SavedArtist::from)
            .collect())
    }

    async fn artist_tracks(&self, artist_id: &str) -> Result<Vec<Track>> {
        let reply = self
            .artist_call(wisp_plugin_api::ops::music_api::ARTIST_TRACKS, artist_id)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::ArtistTracksReply>(&reply)?
            .tracks
            .into_iter()
            .map(Track::from)
            .collect())
    }

    async fn artist_catalogue(&self, artist_id: &str, _known: &[Track]) -> Result<ArtistCatalogue> {
        let reply = self
            .artist_call(wisp_plugin_api::ops::music_api::ARTIST_CATALOGUE, artist_id)
            .await?;
        Ok(
            parse_reply::<wisp_plugin_api::ArtistCatalogueReply>(&reply)?
                .catalogue
                .into(),
        )
    }

    async fn album_catalogue(
        &self,
        album_id: &str,
        artist_id: Option<&str>,
    ) -> Result<AlbumCatalogue> {
        let args = serde_json::to_string(&wisp_plugin_api::AlbumCatalogueArgs {
            album_id: album_id.to_owned(),
            artist_id: artist_id.map(str::to_owned),
        })?;
        let reply = self
            .call(wisp_plugin_api::ops::music_api::ALBUM_CATALOGUE, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::AlbumCatalogueReply>(&reply)?
            .catalogue
            .into())
    }

    async fn home(&self) -> Result<crate::HomeFeed> {
        let reply = self
            .call(wisp_plugin_api::ops::music_api::HOME, "{}")
            .await?;
        Ok(parse_reply::<wisp_plugin_api::HomeReply>(&reply)?
            .feed
            .into())
    }

    async fn home_paged(&self) -> Result<Feed> {
        if !self.plugin.manifest().declares.capabilities.home_parts {
            return Ok(at_once(self.home().await?));
        }
        // The page draws as each stage lands: the quick picks and first shelf, then the
        // playlists and albums, then the rows whose responses are the heaviest. Every stage
        // carries the whole feed so far, which is what the state page replaces the last with.
        const STAGES: [&[&str]; 3] = [
            &["picks", "ranks"],
            &["collections", "albums"],
            &["news", "themes"],
        ];
        let (sender, receiver) = tokio::sync::mpsc::channel(STAGES.len());
        let client = self.clone();
        tokio::spawn(async move {
            let mut feed = crate::HomeFeed::default();
            for parts in STAGES {
                let args = serde_json::to_string(&wisp_plugin_api::HomeArgs {
                    parts: parts.iter().map(|part| (*part).to_owned()).collect(),
                })
                .unwrap_or_default();
                let loaded = async {
                    let reply = client
                        .call(wisp_plugin_api::ops::music_api::HOME, &args)
                        .await?;
                    let lot = parse_reply::<wisp_plugin_api::HomeReply>(&reply)?.feed;
                    Ok::<crate::HomeFeed, anyhow::Error>(lot.into())
                }
                .await;
                match loaded {
                    Ok(lot) => {
                        if feed.quick_picks.is_none() {
                            feed.quick_picks = lot.quick_picks;
                        }
                        feed.sections.extend(lot.sections);
                        if sender.send(Ok(feed.clone())).await.is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        sender.send(Err(error)).await.ok();
                        return;
                    }
                }
            }
        });
        Ok(receiver)
    }

    async fn genres(&self) -> Result<Vec<crate::Genre>> {
        let reply = self
            .call(wisp_plugin_api::ops::music_api::GENRES, "{}")
            .await?;
        Ok(parse_reply::<wisp_plugin_api::GenresReply>(&reply)?
            .genres
            .into_iter()
            .map(crate::Genre::from)
            .collect())
    }

    async fn genre(&self, genre_id: &str) -> Result<crate::GenreDetail> {
        self.genre_page(genre_id, 0, 0).await
    }

    async fn genre_page(
        &self,
        genre_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<crate::GenreDetail> {
        let args = serde_json::to_string(&wisp_plugin_api::GenreArgs {
            genre_id: genre_id.to_owned(),
            offset,
            limit,
        })?;
        let reply = self
            .call(wisp_plugin_api::ops::music_api::GENRE, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::GenreReply>(&reply)?
            .detail
            .into())
    }

    async fn genre_cover(&self, genre_id: &str) -> Result<Option<String>> {
        let args = serde_json::to_string(&wisp_plugin_api::GenreArgs {
            genre_id: genre_id.to_owned(),
            offset: 0,
            limit: 0,
        })?;
        let reply = self
            .call(wisp_plugin_api::ops::music_api::GENRE_COVER, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::GenreCoverReply>(&reply)?.cover)
    }

    async fn climax(&self, track_id: &str) -> Result<Vec<ClimaxMark>> {
        let args = serde_json::to_string(&wisp_plugin_api::TrackArgs {
            track_id: track_id.to_owned(),
        })?;
        let reply = self
            .call(wisp_plugin_api::ops::music_api::CLIMAX, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::ClimaxReply>(&reply)?
            .marks
            .into_iter()
            .map(|mark| ClimaxMark {
                start_ms: mark.start_ms,
                end_ms: mark.end_ms,
            })
            .collect())
    }

    async fn search_keywords(&self, keywords: &str) -> Result<Vec<String>> {
        let args = serde_json::to_string(&wisp_plugin_api::SearchKeywordsArgs {
            keywords: keywords.to_owned(),
        })?;
        let reply = self
            .call_high(wisp_plugin_api::ops::music_api::SEARCH_KEYWORDS, &args)
            .await?;
        Ok(parse_reply::<wisp_plugin_api::SearchKeywordsReply>(&reply)?.keywords)
    }

    async fn checkin(&self, day: Option<&str>) -> Result<crate::CheckinReport> {
        let args = serde_json::to_string(&wisp_plugin_api::CheckinArgs {
            day: day.map(str::to_owned),
        })?;
        let reply = self.call(wisp_plugin_api::ops::ext::CHECKIN, &args).await?;
        let reply = parse_reply::<wisp_plugin_api::CheckinReply>(&reply)?;
        Ok(crate::CheckinReport {
            date: reply.date,
            claimed: reply.claimed,
            upgraded: reply.upgraded,
            claim_code: reply.claim_code,
            upgrade_code: reply.upgrade_code,
        })
    }
}

/// A provider backed by a flat wasm plugin. Its identity comes from the catalog row's manifest.
pub struct WasmProvider {
    plugin: Arc<WasmPlugin>,
    slug: &'static str,
    name: &'static str,
    manifest: wisp_plugin_api::Manifest,
    client: WasmClient,
    /// Bumped when a sign-in starts or is abandoned, so an abandoned poll loop stops instead
    /// of racing the next method against the plugin's shared QR state.
    sign_in_generation: AtomicU64,
}

impl WasmProvider {
    pub fn from_package(
        package: &std::path::Path,
        slug: &'static str,
        name: &'static str,
    ) -> Result<Arc<dyn MusicProvider>> {
        let package = std::fs::canonicalize(package)
            .with_context(|| format!("cannot resolve {}", package.display()))?;
        let loaded = wisp_plugin_pack::load(
            &package,
            &wisp_plugin_pack::VerifyOptions {
                public_key: None,
                allow_unsigned: true,
                allow_zip: false,
                skip_signature: true,
            },
        )?;
        let hosts = loaded.manifest.network.hosts.clone();
        let host = crate::dns::try_handle().map(|handle| {
            let http = reqwest::Client::builder()
                .dns_resolver(crate::dns::dns())
                .build()
                .unwrap_or_default();
            let host = super::runtime::SessionHost::new(http, handle, hosts);
            host.set_storage_dir(super::plugins_dir().join(slug).join("data"));
            Arc::new(host)
        });
        let plugin = super::runtime::shared(&package, host)?;
        Ok(Arc::new(Self::with_manifest(
            plugin,
            loaded.manifest,
            slug,
            name,
        )))
    }

    pub fn new(plugin: Arc<WasmPlugin>, slug: &'static str, name: &'static str) -> Self {
        let manifest = plugin.manifest().clone();
        Self::with_manifest(plugin, manifest, slug, name)
    }

    /// The provider takes the manifest of the package it was built from, not the slot's first
    /// manifest: a shadow swap may not have activated yet and the new capabilities are live
    /// immediately on this provider handle.
    fn with_manifest(
        plugin: Arc<WasmPlugin>,
        manifest: wisp_plugin_api::Manifest,
        slug: &'static str,
        name: &'static str,
    ) -> Self {
        let client = WasmClient::new(Arc::clone(&plugin), slug);
        Self {
            plugin,
            slug,
            name,
            manifest,
            client,
            sign_in_generation: AtomicU64::new(0),
        }
    }

    fn session_path(&self) -> PathBuf {
        crate::credentials::dir(self.slug).join("wasm-session")
    }

    fn read_only(&self) -> bool {
        !self.manifest.declares.ops.iter().any(|op| op == "sign_in")
    }

    fn session_with(&self, profile: UserProfile, authenticated: bool) -> ProviderSession {
        self.client.set_user_id(&profile.id);
        ProviderSession {
            profile,
            api: Arc::new(self.client.clone()),
            playback: Arc::new(WasmPlaybackFactory {
                client: self.client.clone(),
                slug: self.slug,
            }),
            shape: Shape::Saved,
            authenticated,
            capabilities: capabilities_for(&self.manifest),
        }
    }

    fn session(&self, profile: UserProfile) -> ProviderSession {
        self.session_with(profile, true)
    }

    /// A read-only provider serves its data plane without an account. The host's guest path
    /// signs it in anonymously; no plugin call is involved.
    fn guest_session(&self) -> ProviderSession {
        self.session_with(
            UserProfile {
                id: "guest".to_owned(),
                display_name: self.name.to_owned(),
                avatar: None,
            },
            false,
        )
    }

    async fn sign_in_wasm(
        &self,
        method: SignIn,
        prompt: &PromptSink,
        input: &mut InputSource,
    ) -> Result<ProviderSession> {
        let flow = match method {
            SignIn::Qr | SignIn::Default => wisp_plugin_api::FLOW_QR,
            SignIn::Phone => wisp_plugin_api::FLOW_PHONE,
            SignIn::Secret => wisp_plugin_api::FLOW_SECRET,
            SignIn::Password => wisp_plugin_api::FLOW_PASSWORD,
            SignIn::Qq => wisp_plugin_api::FLOW_QQ,
            SignIn::WeChat => wisp_plugin_api::FLOW_WECHAT,
            SignIn::Anonymous | SignIn::Path(_) => {
                anyhow::bail!("this provider cannot sign in with that method")
            }
        };
        let generation = self
            .sign_in_generation
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        let probe = WasmClient::new(Arc::clone(&self.plugin), self.slug);
        let mut input_text = "start".to_owned();
        log::info!("provider {}: starting sign-in flow {flow}", self.slug);
        loop {
            if self.sign_in_generation.load(Ordering::SeqCst) != generation {
                anyhow::bail!("the sign-in was abandoned");
            }
            let step = probe.sign_in_step(flow, &input_text).await.map_err(|error| {
                log::warn!("provider {}: sign-in step failed: {error:#}", self.slug);
                error
            })?;
            match step {
                wisp_plugin_api::SignInStep::Prompt(step) => match step {
                    wisp_plugin_api::Prompt::Qr { url, hint } => {
                        prompt(SignInPrompt::Qr { url, hint });
                        input_text.clear();
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    wisp_plugin_api::Prompt::Phone { notice } => {
                        prompt(SignInPrompt::Phone { notice });
                        input_text = input.recv().await.context("sign-in was cancelled")?;
                    }
                    wisp_plugin_api::Prompt::Password { notice } => {
                        prompt(SignInPrompt::Password { notice });
                        input_text = input.recv().await.context("sign-in was cancelled")?;
                    }
                    wisp_plugin_api::Prompt::Secret => {
                        prompt(SignInPrompt::Secret);
                        input_text = input.recv().await.context("sign-in was cancelled")?;
                    }
                },
                wisp_plugin_api::SignInStep::Wait { ms } => {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    input_text.clear();
                }
                wisp_plugin_api::SignInStep::Done { profile, session } => {
                    log::info!(
                        "provider {}: sign-in completed for {}",
                        self.slug,
                        profile.display_name
                    );
                    crate::credentials::write(&self.session_path(), session.as_bytes())?;
                    self.client.set_cookie(&session);
                    self.plugin.set_sign_in_required(false);
                    return Ok(self.session(profile.into()));
                }
                wisp_plugin_api::SignInStep::Failed { error } => {
                    log::warn!("provider {}: sign-in failed: {}", self.slug, error.message);
                    anyhow::bail!("{}", error.message)
                }
                wisp_plugin_api::SignInStep::Verify(verify) => {
                    prompt(SignInPrompt::Verify {
                        kind: format!("{:?}", verify.kind).to_lowercase(),
                        context: verify.context,
                    });
                    input_text = input.recv().await.context("sign-in was cancelled")?;
                }
            }
        }
    }
}

#[async_trait]
impl MusicProvider for WasmProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    fn slug(&self) -> &'static str {
        self.slug
    }

    fn sign_in_options(&self) -> Vec<SignIn> {
        if self.read_only() {
            return vec![SignIn::Anonymous];
        }
        let options: Vec<SignIn> = self
            .manifest
            .declares
            .sign_in
            .iter()
            .filter_map(|method| match method.as_str() {
                "qr" => Some(SignIn::Qr),
                "phone" => Some(SignIn::Phone),
                "password" => Some(SignIn::Password),
                "qq" => Some(SignIn::Qq),
                "wechat" => Some(SignIn::WeChat),
                "secret" => Some(SignIn::Secret),
                _ => None,
            })
            .collect();
        match options.is_empty() {
            true => vec![SignIn::Qr, SignIn::Phone, SignIn::Secret],
            false => options,
        }
    }

    fn stored(&self) -> bool {
        !self.read_only() && self.session_path().exists()
    }

    fn stored_guest(&self) -> bool {
        self.read_only()
    }

    fn reach(&self) -> Option<String> {
        self.manifest.network.hosts.first().cloned()
    }

    fn listening_to(&self) -> &'static str {
        self.name
    }

    fn public_art(&self) -> bool {
        true
    }

    async fn restore(&self) -> Result<Option<ProviderSession>> {
        if self.read_only() {
            self.plugin.set_sign_in_required(false);
            return Ok(Some(self.guest_session()));
        }
        let Ok(session) = std::fs::read_to_string(self.session_path()) else {
            self.plugin.set_sign_in_required(false);
            return Ok(None);
        };
        self.client.set_cookie(session.trim());
        let profile = match self.client.profile().await {
            Ok(profile) => {
                self.plugin.set_sign_in_required(false);
                profile
            }
            Err(error) if crate::trouble::offline(&format!("{error:#}")) => {
                return Err(error);
            }
            Err(error) => {
                log::warn!(
                    "provider {}: stored session needs a sign-in again: {error:#}",
                    self.slug
                );
                self.plugin.set_sign_in_required(true);
                return Ok(None);
            }
        };
        Ok(Some(self.session(profile)))
    }

    async fn sign_in(
        &self,
        method: SignIn,
        prompt: PromptSink,
        mut input: InputSource,
    ) -> Result<ProviderSession> {
        if self.read_only() {
            if matches!(method, SignIn::Anonymous) {
                return Ok(self.guest_session());
            }
            anyhow::bail!("this provider does not accept that sign-in method");
        }
        self.sign_in_wasm(method, &prompt, &mut input).await
    }

    fn abandon(&self) {
        self.sign_in_generation.fetch_add(1, Ordering::SeqCst);
    }

    fn sign_out(&self) {
        crate::credentials::remove(&self.session_path());
        self.client.set_cookie("");
        self.plugin.set_sign_in_required(false);
    }

    fn playback_factory(&self) -> Option<Arc<dyn PlaybackFactory>> {
        Some(Arc::new(WasmPlaybackFactory {
            client: self.client.clone(),
            slug: self.slug,
        }))
    }
}

fn smart_playlist(dto: wisp_plugin_api::SmartPlaylistDto) -> Result<SmartPlaylist> {
    let rules = match dto.rules {
        Some(text) => {
            let value: serde_json::Value = serde_json::from_str(&text)
                .context("the provider sent malformed smart-playlist rules")?;
            Some(SmartRules::from_wire(&value).map_err(|error| anyhow::anyhow!("{error}"))?)
        }
        None => None,
    };
    Ok(SmartPlaylist {
        id: dto.id,
        name: dto.name,
        comment: dto.comment,
        public: dto.public,
        owner: dto.owner,
        owned: dto.owned,
        rules,
        synced: dto.synced,
        track_count: dto.track_count,
    })
}

fn capabilities_for(manifest: &wisp_plugin_api::Manifest) -> Capabilities {
    let qualities: Vec<Offer> = manifest
        .declares
        .capabilities
        .qualities
        .iter()
        .filter_map(|offer| {
            let quality = Quality::from_id(&offer.quality)?;
            let encoding = match &offer.encoding {
                wisp_plugin_api::EncodingDto::Flac { depth, rate } => Encoding::Flac {
                    depth: *depth as u8,
                    rate: *rate,
                },
                wisp_plugin_api::EncodingDto::Mp3 { kbps } => Encoding::Mp3 { kbps: *kbps },
                wisp_plugin_api::EncodingDto::Aac { kbps } => Encoding::Aac { kbps: *kbps },
            };
            Some(Offer {
                quality,
                encoding,
                up_to: offer.up_to,
            })
        })
        .collect();
    Capabilities {
        follow_artists: manifest.declares.capabilities.follow_artists,
        radio: manifest.declares.capabilities.radio,
        radio_modes: manifest.declares.capabilities.radio_modes,
        checkin: manifest.declares.capabilities.checkin,
        playcounts: manifest.declares.capabilities.playcounts,
        qualities: Box::leak(qualities.into_boxed_slice()),
        playlist_covers: manifest
            .declares
            .ops
            .iter()
            .any(|op| op == "set_playlist_cover"),
        playlist_order: manifest
            .declares
            .ops
            .iter()
            .any(|op| op == "reorder_playlist"),
        playlist_visibility: manifest.declares.capabilities.playlist_visibility,
        smart_playlists: manifest
            .declares
            .ops
            .iter()
            .any(|op| op == "create_smart_playlist"),
        ..Capabilities::ALL
    }
}

// The generic StreamPlan player. It deliberately has no unblock or native fallback: those are
// host/provider features, not part of a third-party plugin's data plane.
struct WasmPlaybackFactory {
    client: WasmClient,
    slug: &'static str,
}

impl PlaybackFactory for WasmPlaybackFactory {
    fn start(&self, config: PlaybackConfig) -> (Box<dyn Player>, Box<dyn PlaybackEvents>) {
        let media = ranged::media(self.slug, || self.client.http.clone());
        engine::start(
            WasmPlayback {
                client: self.client.clone(),
                slug: self.slug,
                quality: config.quality,
                control: config.speed_control.clone(),
                speed: config.speed,
                media,
                shelf: config
                    .vault
                    .as_ref()
                    .map(|vault| vault.shelf(self.slug)),
            },
            config,
        )
    }
}

struct WasmPlayback {
    client: WasmClient,
    slug: &'static str,
    quality: Quality,
    control: Arc<SpeedControl>,
    speed: f32,
    /// The client a seek's own download goes through.
    media: reqwest::Client,
    shelf: Option<Shelf>,
}

impl WasmPlayback {
    async fn try_unblock(&self, id: &str) -> Option<(String, &'static str)> {
        if let Some(hit) = crate::builtin::unblock_for_id(id, self.quality).await {
            return Some(hit);
        }
        let track = self.client.track(id).await.ok()?;
        crate::builtin::unblock_for(&track, self.quality).await
    }
}

/// The headers a plugin's stream is fetched with, including the host's user agent when the
/// plugin gives none.
fn stream_headers(headers: &[wisp_plugin_api::Header]) -> HeaderMap {
    let mut map = HeaderMap::new();
    let mut has_agent = false;
    for header in headers {
        if header.name.eq_ignore_ascii_case("user-agent") {
            has_agent = true;
        }
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(header.name.as_bytes()),
            HeaderValue::from_str(&header.value),
        ) else {
            continue;
        };
        map.append(name, value);
    }
    if !has_agent {
        map.insert(USER_AGENT, HeaderValue::from_static("wisp-provider/0.51"));
    }
    map
}

#[derive(Clone)]
struct Loaded {
    stream: Stream<Plain>,
    duration: Option<Duration>,
    level: String,
    /// What a seek past the download fetches the file through. Only a plain FLAC, Ogg or MP3
    /// stream whose server answers byte ranges has one.
    leap: Option<Arc<ranged::File<Plain>>>,
    /// The track's length once a lookup lands, which a seek enters the file by.
    length: Option<Arc<OnceLock<Duration>>>,
}

#[async_trait]
impl Fetch for WasmPlayback {
    type Loaded = Loaded;
    type Source = SpeedSource<rodio::Decoder<Reader<Plain>>>;

    fn name(&self) -> &'static str {
        self.slug
    }

    fn blocked(&self, error: &anyhow::Error) -> bool {
        crate::trouble::blocked(&format!("{error:#}"))
    }

    async fn load(&self, id: &str) -> Result<Loaded> {
        let requested = self.quality.id();
        if let Some(shelf) = &self.shelf {
            if let Some(kept) = shelf.find(id, requested).await {
                let facts = serde_json::from_slice::<Facts>(&kept.facts).ok();
                let duration = facts
                    .as_ref()
                    .and_then(|facts| facts.duration_s)
                    .map(Duration::from_secs);
                let stream = Stream::pulling(kept.source, Some(kept.length), Plain)
                    .primed()
                    .await?;
                return Ok(Loaded {
                    stream,
                    duration,
                    level: requested.to_owned(),
                    leap: None,
                    length: None,
                });
            }
        }
        let (mut url, mut headers, mut variant) = match self.client.stream_plan(id, requested).await {
            Ok(plan) => (plan.url, plan.headers, plan.variant),
            Err(error) if self.slug == "netease" => {
                log::info!("playback: {id} has no plugin stream; trying the built-in unblock chain");
                match self.try_unblock(id).await {
                    Some((url, source)) => (url, Vec::new(), format!("unm-{source}-{requested}")),
                    None => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        let response = match self.client.open_stream(&url, &headers).await {
            Ok(response) => response,
            Err(error) if self.slug == "netease" => {
                log::info!(
                    "playback: {id} official stream failed ({error:#}); trying the built-in unblock chain"
                );
                match self.try_unblock(id).await {
                    Some((fresh, source)) => {
                        variant = format!("unm-{source}-{requested}");
                        url = fresh;
                        headers.clear();
                        self.client
                            .open_stream(&url, &[])
                            .await
                            .with_context(|| format!("unblock stream failed after {error:#}"))?
                    }
                    None => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        let total = response.content_length();
        let seekable = ranged::ranges(&response);
        let level = if variant.is_empty() {
            requested.to_owned()
        } else {
            variant
        };
        let facts = Facts {
            duration_s: None,
            level: level.clone(),
        };
        let body = match &self.shelf {
            Some(shelf) => shelf.keep(
                id,
                &level,
                move || serde_json::to_vec(&facts).ok(),
                response,
            ),
            None => Recording::through(response),
        };
        let stream = Stream::pulling(body, total, Plain).primed().await?;
        let leap = match seekable {
            true => {
                let headers = stream_headers(&headers);
                let (media, url, slug) = (self.media.clone(), url.clone(), self.slug);
                let reach = Reach::new(slug, id, move |_| {
                    let address = Address::new(slug, media.clone(), url.clone());
                    (address.headers(headers.clone()), Plain)
                });
                ranged::File::detect(&stream, reach)
            }
            false => None,
        };
        let length = leap.as_ref().map(|_| {
            let length = Arc::new(OnceLock::new());
            let cell = Arc::clone(&length);
            let client = self.client.clone();
            let track_id = id.to_owned();
            tokio::spawn(async move {
                if let Ok(track) = client.track(&track_id).await {
                    let _ = cell.set(track.duration);
                }
            });
            length
        });
        Ok(Loaded {
            stream,
            duration: None,
            level,
            leap,
            length,
        })
    }

    fn length(&self, loaded: &Loaded) -> Option<Duration> {
        loaded
            .duration
            .or_else(|| {
                loaded
                    .length
                    .as_ref()
                    .and_then(|length| length.get().copied())
            })
            .filter(|length| !length.is_zero())
    }

    async fn downloaded(&self, loaded: &Loaded) {
        loaded.stream.finished().await;
    }

    fn open(&self, _id: &str, loaded: &Loaded, at: Duration) -> Option<Self::Source> {
        let mut builder = rodio::Decoder::builder()
            .with_data(loaded.stream.reader())
            .with_seekable(true);
        if let Some(total) = loaded.stream.total() {
            builder = builder.with_byte_len(total);
        }
        let mut decoder = builder.build().ok()?;
        if at > Duration::ZERO {
            decoder.try_seek(at).ok()?;
        }
        Some(SpeedSource::new(decoder, self.speed, self.control.clone()))
    }

    /// Opens the track at `at` from a download started near it when the main one is far from
    /// there and the file is a format a seek can enter. Otherwise the decoder waits for the
    /// main download to get there, as [`open`](Fetch::open) does.
    fn open_near(
        &self,
        id: &str,
        loaded: &Loaded,
        at: Duration,
    ) -> Option<(Self::Source, Duration)> {
        let length = self.length(loaded);
        let entered = loaded.leap.as_ref().zip(length).and_then(|(file, length)| {
            file.enter(&loaded.stream, length, at, |reader| {
                let decoder = ranged::forward(self.slug, id, reader)?;
                Some(SpeedSource::new(decoder, self.speed, self.control.clone()))
            })
        });
        entered.or_else(|| self.open(id, loaded, at).map(|source| (source, at)))
    }

    /// A seek opens a second decoder at its target when the file can be entered there, so one
    /// past the download starts from a new ranged download rather than wait for the main one.
    /// Without both a reach and the track's length, the running decoder moves as before.
    fn reopen_to_seek(&self, loaded: &Loaded) -> bool {
        loaded.leap.is_some() && self.length(loaded).is_some()
    }
}

fn _catalog_shape() -> Shape {
    Shape::Saved
}

/// The health of a slot, re-exported for the plugin settings page.
pub fn health(slug: &str) -> Option<PluginHealth> {
    super::runtime::health_for(slug)
}

#[cfg(test)]
mod tests {
    use crate::plugins::Catalog;
    use std::fs;

    #[tokio::test]
    async fn a_loaded_stream_survives_a_plugin_swap_and_the_next_track_loads() {
        use super::{WasmClient, WasmPlayback};
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;
        use std::sync::Arc;
        use wisp_plugin_api::{Host, HttpRequest, HttpResponse, PluginError};

        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();

        let Some(path) = std::env::var_os("WISP_NETEASE_WISP") else {
            return;
        };
        let package = std::path::Path::new(&path).to_path_buf();

        let listener = TcpListener::bind("127.0.0.1:0").expect("the local stream server");
        let address = listener.local_addr().expect("the local address");
        let payload = vec![0x5au8; 256 * 1024];
        let served = payload.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().expect("a stream request");
                let mut request = [0u8; 2048];
                let _ = socket.read(&mut request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    served.len()
                );
                socket
                    .write_all(head.as_bytes())
                    .expect("the response head");
                socket.write_all(&served).expect("the response body");
            }
        });

        struct PlanHost {
            plan: Vec<u8>,
        }
        impl Host for PlanHost {
            fn http(&self, _request: HttpRequest) -> Result<HttpResponse, PluginError> {
                Ok(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: self.plan.clone(),
                })
            }

            fn secret_get(&self, _key: &str) -> Option<String> {
                None
            }
        }

        let plan = serde_json::json!({
            "data": [{
                "url": format!("http://{address}/audio"),
                "size": payload.len() as u64,
                "code": 200,
                "level": "exhigh",
                "expi": 0
            }]
        });
        let host = Arc::new(PlanHost {
            plan: serde_json::to_vec(&plan).expect("the plan JSON"),
        });
        let plugin =
            Arc::new(super::WasmPlugin::with_host(&package, Some(host)).expect("the real plugin"));
        let client = WasmClient::new(plugin.clone(), "netease");
        let playback = WasmPlayback {
            client,
            slug: "netease",
            quality: crate::Quality::High,
            control: Arc::new(crate::speed::SpeedControl::default()),
            speed: 1.0,
            media: reqwest::Client::new(),
            shelf: None,
        };

        let loaded = crate::engine::Fetch::load(&playback, "track")
            .await
            .expect("the stream loads");
        let mut reader = loaded.stream.reader();
        let mut first = vec![0u8; 32 * 1024];
        std::io::Read::read_exact(&mut reader, &mut first).expect("the loaded stream reads");

        let copy = std::env::temp_dir().join("wisp-playback-swap.wisp");
        std::fs::copy(&package, &copy).expect("the second package");
        plugin.swap(&copy).expect("the plugin hot swap");
        assert_eq!(plugin.health(), super::PluginHealth::Active);

        let mut rest = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut rest).expect("the old stream keeps reading");
        assert!(first.len() + rest.len() >= payload.len());
        assert!(first.iter().chain(rest.iter()).all(|byte| *byte == 0x5a));

        // The swapped slot still serves the next track through the same provider handle.
        let next = crate::engine::Fetch::load(&playback, "next")
            .await
            .expect("the next stream loads");
        let mut reader = next.stream.reader();
        let mut byte = [0u8; 1];
        std::io::Read::read_exact(&mut reader, &mut byte).expect("the next stream reads");
        assert_eq!(byte, [0x5a]);

        server.join().expect("the local stream server");
        let _ = std::fs::remove_file(copy);
    }

    #[test]
    fn an_unknown_flat_package_becomes_a_provider_from_its_manifest() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let dir = tempfile::tempdir().expect("a temp dir");
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("icons")).expect("the source tree");
        let mut manifest = crate::test_support::manifest("demo");
        manifest.id = "demo".to_owned();
        manifest.slug = "demo".to_owned();
        manifest.name.default = "Demo Music".to_owned();
        manifest.version = "1.0.0".to_owned();
        manifest.entry = "lib.wasm".to_owned();
        fs::write(
            source.join("wisp.json"),
            serde_json::to_vec(&manifest).expect("the manifest"),
        )
        .expect("wisp.json");
        fs::write(source.join("lib.wasm"), b"not instantiated here").expect("the entry");
        fs::write(source.join("icons/netease.svg"), b"<svg/>").expect("the icon");
        let package = wisp_plugin_pack::pack_dir(&source, None).expect("the package");
        wisp_plugin_pack::write_wisp(&package, &dir.path().join("demo-1.0.0.wisp"))
            .expect("the flat package");

        let catalog = Catalog::scan(dir.path());
        let providers = catalog.providers();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].slug(), "demo");
        assert_eq!(providers[0].name(), "Demo Music");
        assert!(providers[0].playback_factory().is_some());
    }

    #[test]
    fn the_manifest_decides_which_sign_in_methods_are_offered() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let dir = tempfile::tempdir().expect("a temp dir");
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("icons")).expect("the source tree");
        let mut manifest = crate::test_support::manifest("demo");
        manifest.id = "methods-demo".to_owned();
        manifest.slug = "methods-demo".to_owned();
        manifest.name.default = "Methods Demo".to_owned();
        manifest.version = "1.0.0".to_owned();
        manifest.entry = "lib.wasm".to_owned();
        manifest.declares.sign_in = ["qr", "phone", "password", "qq", "wechat"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        fs::write(
            source.join("wisp.json"),
            serde_json::to_vec(&manifest).expect("the manifest"),
        )
        .expect("wisp.json");
        fs::write(source.join("lib.wasm"), b"not instantiated here").expect("the entry");
        fs::write(source.join("icons/netease.svg"), b"<svg/>").expect("the icon");
        let package = wisp_plugin_pack::pack_dir(&source, None).expect("the package");
        wisp_plugin_pack::write_wisp(&package, &dir.path().join("methods-demo-1.0.0.wisp"))
            .expect("the flat package");

        let catalog = Catalog::scan(dir.path());
        let providers = catalog.providers();
        assert_eq!(
            providers[0].sign_in_options(),
            vec![
                crate::SignIn::Qr,
                crate::SignIn::Phone,
                crate::SignIn::Password,
                crate::SignIn::Qq,
                crate::SignIn::WeChat,
            ]
        );
    }

    #[tokio::test]
    async fn a_read_only_package_gets_an_anonymous_guest_session() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let dir = tempfile::tempdir().expect("a temp dir");
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("icons")).expect("the source tree");
        let mut manifest = crate::test_support::manifest("demo");
        manifest.id = "guest-demo".to_owned();
        manifest.slug = "guest-demo".to_owned();
        manifest.name.default = "Guest Demo".to_owned();
        manifest.version = "1.0.0".to_owned();
        manifest.entry = "lib.wasm".to_owned();
        manifest.declares.ops.retain(|op| op != "sign_in");
        manifest.declares.capabilities.follow_artists = false;
        manifest.declares.capabilities.radio = false;
        manifest.declares.capabilities.playcounts = false;
        fs::write(
            source.join("wisp.json"),
            serde_json::to_vec(&manifest).expect("the manifest"),
        )
        .expect("wisp.json");
        fs::write(source.join("lib.wasm"), b"not instantiated here").expect("the entry");
        fs::write(source.join("icons/netease.svg"), b"<svg/>").expect("the icon");
        let package = wisp_plugin_pack::pack_dir(&source, None).expect("the package");
        wisp_plugin_pack::write_wisp(&package, &dir.path().join("guest-demo-1.0.0.wisp"))
            .expect("the flat package");

        let catalog = Catalog::scan(dir.path());
        let providers = catalog.providers();
        assert_eq!(providers.len(), 1);
        let provider = &providers[0];
        assert_eq!(provider.sign_in_options(), vec![crate::SignIn::Anonymous]);
        assert!(provider.stored_guest());
        assert!(!provider.stored());
        let restored = provider
            .restore()
            .await
            .expect("restore")
            .expect("a guest session");
        assert!(!restored.authenticated);

        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
        let prompt: crate::PromptSink = std::sync::Arc::new(|_| {});
        let session = provider
            .sign_in(crate::SignIn::Anonymous, prompt, receiver)
            .await
            .expect("the guest session");
        assert!(!session.authenticated);
        assert_eq!(session.profile.display_name, "Guest Demo");
        assert!(!session.capabilities.follow_artists);
        assert!(!session.capabilities.radio);
        assert!(!session.capabilities.playcounts);
    }
}
