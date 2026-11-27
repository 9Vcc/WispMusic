// [wisp] vendored Sonora 引擎代码，provider 裁剪后留有不少未调用的工具函数，暂不逐个删除。
#![allow(dead_code)]
pub mod artwork;
mod calendar;
pub mod convert;
pub mod cover;
pub mod credentials;
pub mod escape;
mod japanese;
pub mod link;
mod models;
pub mod order;
pub mod progress;
mod redact;
pub mod romanize;
pub mod smart;
pub mod trouble;
pub mod vault;

pub use sound::dns::{bind_dns_runtime, dns as dns_resolver};
pub use sound::{Analyzer, Level, Listening, Spectrum, Watch};
pub use sound::{Meter, SoundServer, missing_bridge};
pub use sound::{audio, dns, equalizer, output, ranged, spectrum, speed, stream};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;
use jiff::fmt::temporal::DateTimeParser;

pub use equalizer::Equalizer;
pub use models::{
    Album, AlbumCatalogue, AlbumDetail, Arrival, Artist, ArtistCatalogue, ArtistProfile, ArtistRef,
    ClimaxMark, Contributor, CoverSize, Credit, Found, Genre, GenreDetail, GenreItem, GenreSection,
    HomeFeed, ImportedPlaylist, Lyrics, LyricsHit, LyricsLane, LyricsLine, LyricsQuery, LyricsWord,
    PinOutcome, PinTarget, PinTargetKind, Playlist, PlaylistDetail, ReleaseType, Repeat,
    RomanizedText, SavedArtist, Track, TrackKey, TrackTags, UserDetail, UserProfile, Voice,
    WritingSystem,
};
pub use redact::redact;
pub use smart::{
    Condition, FieldKind, Operand, OperandShape, Operator, Problem, Rule, RuleGroup, RuleProblem,
    SmartPlaylist, SmartPlaylistUpdate, SmartRules, field_kind, parse_number,
};

pub const LOCAL_TRACK_PREFIX: &str = "local:";
pub const LOCAL_ALBUM_PREFIX: &str = "local-album:";
pub const LOCAL_ARTIST_PREFIX: &str = "local-artist:";
pub const LOCAL_PLAYLIST_PREFIX: &str = "local-playlist:";

/// The most recommendations a provider hands one list of an album or artist page, so a
/// rail never asks for or draws more than this many releases or artists.
pub const SUGGESTIONS: usize = 10;

/// The duration tolerance for recognizing the same recording across releases.
const SAME_SONG: Duration = Duration::from_secs(5);

pub fn is_local_id(id: &str) -> bool {
    id.starts_with(LOCAL_TRACK_PREFIX)
        || id.starts_with(LOCAL_ALBUM_PREFIX)
        || id.starts_with(LOCAL_ARTIST_PREFIX)
        || id.starts_with(LOCAL_PLAYLIST_PREFIX)
}

pub fn distinct_covers(tracks: &[Track], wanted: usize) -> Vec<String> {
    let mut covers: Vec<String> = Vec::with_capacity(wanted);
    for cover in tracks.iter().filter_map(|track| track.cover.as_deref()) {
        if covers.len() == wanted {
            break;
        }
        if !covers.iter().any(|kept| kept == cover) {
            covers.push(cover.to_owned());
        }
    }

    covers
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Track,
    Album,
    Artist,
    Playlist,
}

/// What `MusicApi::report` tells the provider's server about the current track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    Playing,
    Paused,
    Stopped,
}

#[async_trait]
pub trait MusicApi: Send + Sync {
    fn alive(&self) -> bool {
        true
    }

    /// The tracks and exact file moves from the last local scan; empty for a streaming source.
    fn local_tracks(&self) -> Vec<Track> {
        Vec::new()
    }
    fn local_moves(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// Folders and files the last local scan could not read; empty for a streaming source.
    fn local_issues(&self) -> Vec<String> {
        Vec::new()
    }

    /// A cover url this provider handed out, at `size`. It is worked out from the url alone,
    /// so `None` means the provider cannot tell and the caller loads the album instead.
    fn cover_at(&self, _cover: &str, _size: CoverSize) -> Option<String> {
        None
    }

    /// The provider's original-resolution url for a cover it handed out, when the url alone
    /// names one. Only the explicit full-size cover view asks for this, so a provider that
    /// cannot name an original keeps showing the large cover.
    fn cover_original(&self, _cover: &str) -> Option<String> {
        None
    }

    fn share_url(&self, kind: MediaKind, id: &str) -> Option<String>;
    async fn profile(&self) -> Result<UserProfile>;

    async fn user(&self, _user_id: &str) -> Result<UserDetail> {
        anyhow::bail!("user profiles are not supported")
    }

    async fn artist(&self, artist_id: &str) -> Result<Artist>;

    /// The rest of an artist page, fetched once `artist` has put the overview up: the whole
    /// discography and the popular tracks that only the discography can rank. `known` is the
    /// top tracks already on the page, so the provider can rank around them. A provider whose
    /// `artist` already answers with everything leaves the default, which is nothing more to
    /// fetch.
    async fn artist_catalogue(
        &self,
        _artist_id: &str,
        _known: &[Track],
    ) -> Result<ArtistCatalogue> {
        Ok(ArtistCatalogue::default())
    }

    /// The artist's songs for playback, with popular tracks first when available.
    /// Providers extend the overview's tracks with their ranked catalogue or release order.
    /// [wisp] 搜索框的关键词联想；默认无（各 provider 自行实现）。
    async fn search_keywords(&self, _keywords: &str) -> Result<Vec<String>> {
        Ok(vec![])
    }

    /// Claims the provider's daily membership reward. A provider whose capabilities do not
    /// include `checkin` leaves this unsupported.
    async fn checkin(&self, _day: Option<&str>) -> Result<CheckinReport> {
        anyhow::bail!("this provider has no daily check-in")
    }

    async fn artist_tracks(&self, artist_id: &str) -> Result<Vec<Track>> {
        Ok(self.artist(artist_id).await?.top_tracks)
    }

    async fn artist_profile(&self, artist_id: &str) -> Result<ArtistProfile>;
    async fn artist_images(&self, ids: Vec<String>) -> Result<HashMap<String, String>>;

    /// The tracks the user starred. On a `Shape::Saved` provider this is the whole songs
    /// library; on a `Shape::Catalog` one it only feeds the hearts and the favorites filter.
    async fn saved_tracks(&self) -> Result<Vec<Track>>;

    /// The favorites a page at a time, for a library page that shows its first rows while the
    /// rest arrive. A provider that lists in one go leaves the default, which is one page.
    async fn saved_tracks_paged(&self) -> Result<Pages<Track>> {
        Ok(whole(self.saved_tracks().await?))
    }

    /// Every track the provider has. Only a `Shape::Catalog` provider answers.
    async fn all_tracks(&self) -> Result<Vec<Track>> {
        Ok(Vec::new())
    }

    async fn all_tracks_paged(&self) -> Result<Pages<Track>> {
        Ok(whole(self.all_tracks().await?))
    }

    async fn set_track_saved(&self, track_id: &str, saved: bool) -> Result<()>;

    /// Reads what the file itself says, for a provider whose tracks are files.
    async fn track_tags(&self, _track_id: &str) -> Result<TrackTags> {
        anyhow::bail!("this provider cannot edit tags")
    }

    async fn set_track_tags(&self, _track_id: &str, _tags: TrackTags) -> Result<()> {
        anyhow::bail!("this provider cannot edit tags")
    }
    async fn track(&self, track_id: &str) -> Result<Track>;

    /// Delete a track file from disk (only for local provider)
    async fn delete_track_file(&self, _track_id: &str) -> Result<()> {
        anyhow::bail!("this provider does not support file deletion")
    }
    async fn track_playcount(&self, track_id: &str) -> Result<Option<u64>>;

    /// Tells the provider's own server whether a track is playing and where it is. It is sent on
    /// every start, pause, seek and stop, and never counts as a listen. A provider that keeps no
    /// listening record keeps the default and makes no request.
    async fn report(&self, _track_id: &str, _report: Report, _position: Duration) -> Result<()> {
        Ok(())
    }

    /// Records a finished listen that started at `at` on the provider's own server. It is sent
    /// at the same moment and under the same rules as a scrobble. A provider that keeps no
    /// listening record keeps the default and makes no request.
    async fn played(&self, _track_id: &str, _at: SystemTime) -> Result<()> {
        Ok(())
    }

    /// Tells the provider a track has started playing, so the play counts on the provider's
    /// own side. A provider that keeps no history keeps the default and makes no request.
    async fn report_play(&self, _track_id: &str) -> Result<()> {
        Ok(())
    }

    /// [wisp] Tells the provider the listener does not want a track suggested again, as the
    /// private radio's dislike does. A provider with no such control keeps the default.
    async fn dislike(&self, _track_id: &str) -> Result<()> {
        Ok(())
    }

    /// The tracks the account has recently played, newest first, across every device. A provider
    /// that keeps no cross-device history keeps the default and answers with nothing.
    async fn recently_played(&self) -> Result<Vec<Track>> {
        Ok(Vec::new())
    }

    async fn playlists(&self) -> Result<Vec<Playlist>>;
    /// Changes the provider's own pin for `uri`, one of the uris `pin_targets` lists or
    /// `pin_uri` builds.
    async fn set_pinned(&self, _uri: &str, _pinned: bool) -> Result<PinOutcome> {
        anyhow::bail!("pinning is not supported")
    }

    /// What the provider can pin, each saying whether it is pinned now. A provider may list
    /// only its pinned items and answer `pin_uri` for the rest. `None` means the provider
    /// keeps no pins of its own.
    async fn pin_targets(&self) -> Result<Option<Vec<PinTarget>>> {
        Ok(None)
    }

    /// The uri `set_pinned` takes for an item `pin_targets` does not list. `None` means only
    /// a listed item can be pinned on the provider's side.
    fn pin_uri(&self, _kind: PinTargetKind, _id: &str) -> Option<String> {
        None
    }
    async fn create_playlist(&self, name: &str) -> Result<String>;
    async fn rename_playlist(&self, playlist_id: &str, name: &str) -> Result<()>;
    async fn delete_playlist(&self, playlist_id: &str) -> Result<()>;
    async fn remove_playlist_from_library(&self, playlist_id: &str) -> Result<()>;
    async fn add_playlist_to_library(&self, playlist_id: &str) -> Result<()>;

    /// Puts a track or an album into the listener's library, or takes it out, on a provider
    /// whose library is apart from its favorites. Only a `Catalog::Editable` provider
    /// answers. An artist is never added: a library artist is one whose music is there.
    async fn set_in_library(&self, _kind: MediaKind, _id: &str, _present: bool) -> Result<()> {
        anyhow::bail!("this provider has no library apart from its favorites")
    }
    async fn set_playlist_public(&self, playlist_id: &str, public: bool) -> Result<()>;

    /// Replaces a playlist's cover with `image`, the bytes of a picture in any format
    /// [`cover::square_jpeg`] reads, or puts the provider's own cover back when it is `None`.
    /// Only a provider whose capabilities list `playlist_covers` answers.
    async fn set_playlist_cover(&self, _playlist_id: &str, _image: Option<&[u8]>) -> Result<()> {
        anyhow::bail!("this provider cannot change a playlist's cover")
    }

    /// Reads one playlist's smart rules and edit bookkeeping. Only a provider whose
    /// capabilities list `smart_playlists` answers.
    async fn smart_playlist(&self, _playlist_id: &str) -> Result<SmartPlaylist> {
        anyhow::bail!("this provider has no smart playlists")
    }

    /// Creates a smart playlist called `name` from `rules` and answers its id. Only a
    /// provider whose capabilities list `smart_playlists` answers.
    async fn create_smart_playlist(&self, _name: &str, _rules: &SmartRules) -> Result<String> {
        anyhow::bail!("this provider has no smart playlists")
    }

    /// Writes the fields `update` names to the smart playlist and answers how it reads after
    /// the write; fields the update leaves as `None` keep their values. Only a provider whose
    /// capabilities list `smart_playlists` answers.
    async fn update_smart_playlist(
        &self,
        _playlist_id: &str,
        _update: SmartPlaylistUpdate,
    ) -> Result<SmartPlaylist> {
        anyhow::bail!("this provider has no smart playlists")
    }
    async fn add_track_to_playlist(&self, playlist_id: &str, track_id: &str) -> Result<()>;

    /// Adds tracks to the end of a playlist in order, and a track named twice goes in twice. A
    /// provider that takes many at once does it in one write. The default adds them one by one
    /// and stops at the first that fails, leaving the ones before it in.
    async fn add_tracks_to_playlist(&self, playlist_id: &str, track_ids: &[String]) -> Result<()> {
        for track_id in track_ids {
            self.add_track_to_playlist(playlist_id, track_id).await?;
        }
        Ok(())
    }
    async fn remove_track_from_playlist(&self, playlist_id: &str, track_id: &str) -> Result<()>;

    /// Puts the playlist's tracks in the order `wanted`, the tracks `playlist` answers in a new
    /// order. `seen` is the id of every track the listener saw before they moved any, in order,
    /// and a provider whose listing is no longer exactly that refuses, rather than land a track
    /// somewhere the listener did not drop it. Only a provider whose capabilities list
    /// `playlist_order` answers.
    async fn reorder_playlist(
        &self,
        _playlist_id: &str,
        _seen: &[String],
        _wanted: &[String],
    ) -> Result<()> {
        anyhow::bail!("this provider cannot reorder a playlist")
    }

    /// Reads the M3U or M3U8 playlist at `path` into a new playlist named after the file, for a
    /// provider whose tracks are files. Entries naming a file outside the library are left out
    /// and counted.
    async fn import_playlist(&self, _path: &Path) -> Result<ImportedPlaylist> {
        anyhow::bail!("this provider cannot import playlist files")
    }

    /// Writes a playlist to `path` as M3U8, for a provider whose tracks are files.
    async fn export_playlist(&self, _playlist_id: &str, _path: &Path) -> Result<()> {
        anyhow::bail!("this provider cannot export playlist files")
    }
    async fn saved_albums(&self) -> Result<Vec<Album>>;

    /// Every album the provider has. Only a `Shape::Catalog` provider answers.
    async fn all_albums(&self) -> Result<Vec<Album>> {
        Ok(Vec::new())
    }

    async fn saved_albums_paged(&self) -> Result<Pages<Album>> {
        Ok(whole(self.saved_albums().await?))
    }

    async fn all_albums_paged(&self) -> Result<Pages<Album>> {
        Ok(whole(self.all_albums().await?))
    }

    async fn set_album_saved(&self, album_id: &str, saved: bool) -> Result<()>;
    async fn saved_artists(&self) -> Result<Vec<SavedArtist>>;

    /// Every artist the provider has. Only a `Shape::Catalog` provider answers.
    async fn all_artists(&self) -> Result<Vec<SavedArtist>> {
        Ok(Vec::new())
    }

    async fn saved_artists_paged(&self) -> Result<Pages<SavedArtist>> {
        Ok(whole(self.saved_artists().await?))
    }

    async fn all_artists_paged(&self) -> Result<Pages<SavedArtist>> {
        Ok(whole(self.all_artists().await?))
    }

    async fn set_artist_saved(&self, artist_id: &str, saved: bool) -> Result<()>;
    async fn album(&self, album_id: &str) -> Result<AlbumDetail>;
    async fn album_tracks(&self, album_id: &str) -> Result<Vec<Track>>;

    /// The rest of an album page, fetched once `album` has put the tracks up: the releases
    /// the provider lists as related, with more from the same artist first and similar
    /// artists' releases topping the rail up. `artist_id` is
    /// the page's artist when the album names one the app can follow. A provider whose
    /// `album` already answers with everything leaves the default, which is nothing more
    /// to fetch.
    async fn album_catalogue(
        &self,
        _album_id: &str,
        _artist_id: Option<&str>,
    ) -> Result<AlbumCatalogue> {
        Ok(AlbumCatalogue::default())
    }
    async fn playlist(&self, playlist_id: &str) -> Result<PlaylistDetail>;
    async fn playlist_continuation(
        &self,
        _continuation: &str,
    ) -> Result<(Vec<Track>, Option<String>)> {
        anyhow::bail!("playlist pagination is not supported")
    }

    async fn playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Track>>;
    async fn playlist_covers(&self, playlist_id: &str, wanted: usize) -> Result<Vec<String>>;
    /// The station seeded by `track_id`, from its start or from `from`, a continuation an
    /// earlier call answered with. The continuation that comes back fetches the next stretch,
    /// and `None` means the provider has no more. A provider that serves a station in one go
    /// ignores `from` and answers `None`, which it is then never handed.
    async fn track_radio(
        &self,
        track_id: &str,
        from: Option<&str>,
    ) -> Result<(Vec<Track>, Option<String>)>;

    /// The same station with the provider's optional FM mode and song pool. Providers without
    /// those switches ignore them; the host only sends them for a provider that advertises
    /// `radio_modes`.
    async fn track_radio_with(
        &self,
        track_id: &str,
        from: Option<&str>,
        _mode: Option<&str>,
        _song_pool_id: Option<u32>,
    ) -> Result<(Vec<Track>, Option<String>)> {
        self.track_radio(track_id, from).await
    }

    async fn search(&self, query: &str) -> Result<Vec<Track>>;

    async fn search_albums(&self, _query: &str) -> Result<Vec<Album>> {
        Ok(Vec::new())
    }

    async fn search_playlists(&self, _query: &str) -> Result<Vec<Playlist>> {
        Ok(Vec::new())
    }

    /// The artists the provider finds for `query`, best match first. A provider without an
    /// artist search keeps the default of none, and search then names only the artists behind
    /// the songs and albums it found.
    async fn search_artists(&self, _query: &str) -> Result<Vec<SavedArtist>> {
        Ok(Vec::new())
    }

    /// Songs, albums, playlists and artists for `query`, which is what the search page and the
    /// command line show. The default runs the four searches at once. A provider that can
    /// answer every kind in one request overrides it, so a query costs one round trip.
    async fn search_all(&self, query: &str) -> Found {
        let (tracks, albums, playlists, artists) = futures::join!(
            self.search(query),
            self.search_albums(query),
            self.search_playlists(query),
            self.search_artists(query),
        );
        Found {
            tracks,
            albums,
            playlists,
            artists,
        }
    }

    async fn home(&self) -> Result<HomeFeed> {
        Ok(HomeFeed::default())
    }

    /// The home feed as it fills, so a page draws its first shelves before its last have
    /// arrived. Defaults to `home` delivered at once, so a provider that has the feed in one go
    /// writes nothing.
    async fn home_paged(&self) -> Result<Feed> {
        Ok(at_once(self.home().await?))
    }

    async fn name_home_playlists(&self, sections: Vec<GenreSection>) -> Vec<GenreSection> {
        sections
    }

    async fn genres(&self) -> Result<Vec<Genre>> {
        Ok(Vec::new())
    }

    async fn genre(&self, _genre_id: &str) -> Result<GenreDetail> {
        Ok(GenreDetail::default())
    }

    /// One more page of a genre, starting at `offset` and asking for at most `limit` cards.
    /// Providers that list a genre in one go ignore both and return the whole page again.
    async fn genre_page(&self, genre_id: &str, offset: usize, limit: usize) -> Result<GenreDetail> {
        let _ = (offset, limit);
        self.genre(genre_id).await
    }

    /// A picture for a genre that `genres` listed without one, found with as few requests as
    /// the provider allows. Defaults to none, for a provider whose genres come with covers.
    async fn genre_cover(&self, _genre_id: &str) -> Result<Option<String>> {
        Ok(None)
    }

    /// The song's climax segments in milliseconds, when the provider knows them. Defaults to
    /// none, so the seek bar shows no marks.
    async fn climax(&self, _track_id: &str) -> Result<Vec<ClimaxMark>> {
        Ok(Vec::new())
    }
}

#[async_trait]
pub trait LyricsProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn search(&self, query: &LyricsQuery) -> Result<Vec<LyricsHit>>;
}

/// What an engine is started with. `equalizer` is shared rather than copied: the engine keeps
/// reading it, so a change reaches the output without a restart.
#[derive(Clone, Debug)]
pub struct PlaybackConfig {
    pub normalisation: bool,
    pub gapless: bool,
    pub position_interval: Duration,
    pub gain: f32,
    pub equalizer: Equalizer,
    pub transcode: Transcode,
    pub quality: Quality,
    /// 播放速度（1.0 = 正常）。provider 在打开解码器时套一层 Speed。
    pub speed: f32,
    /// 跨线程速度控制：UI 改速度时写这里，音频线程就地生效，不重建引擎。
    pub speed_control: Arc<sound::speed::SpeedControl>,
    /// Shared like `equalizer`: every output the engine opens reports its loudest sample here.
    pub meter: Meter,
    /// Shared like `equalizer`: every output the engine opens lets it hear the music ahead of the
    /// equalizer while it is watched.
    pub analyzer: Analyzer,
    /// Where played tracks are cached, while caching is on.
    pub vault: Option<Arc<vault::Vault>>,
}

/// How good a stream to ask a streaming service for. Each provider lists the levels it serves in
/// [`Capabilities::qualities`], plays any other level as the nearest of those through
/// [`Quality::within`], and steps further down by itself when the account or the track has
/// nothing at that level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Quality {
    Low,
    Normal,
    High,
    Lossless,
    #[default]
    HiRes,
    /// Kugou's concept-VIP levels above Hi-Res, lowest first. Other providers leave them out
    /// of their offers, and playback falls back to the nearest level they do serve.
    ViperClear,
    ViperTape,
    ViperAtmos,
}

impl Quality {
    /// Every level, best first.
    pub const ALL: [Self; 8] = [
        Self::ViperAtmos,
        Self::ViperTape,
        Self::ViperClear,
        Self::HiRes,
        Self::Lossless,
        Self::High,
        Self::Normal,
        Self::Low,
    ];

    /// The name settings store the level by: the level's own name on the provider's wire, so
    /// one word carries a choice from the dialog to the request.
    pub fn id(self) -> &'static str {
        match self {
            Self::ViperAtmos => "viper_atmos",
            Self::ViperTape => "viper_tape",
            Self::ViperClear => "viper_clear",
            Self::HiRes => "hires",
            Self::Lossless => "lossless",
            Self::High => "exhigh",
            Self::Normal => "higher",
            Self::Low => "standard",
        }
    }

    /// The level a stored name stands for, accepting the names the settings used to be saved
    /// under before they were the provider's own, so a saved choice survives the change.
    pub fn from_id(id: &str) -> Option<Self> {
        let id = match id {
            "hi-res" => "hires",
            "high" => "exhigh",
            "normal" => "higher",
            "low" => "standard",
            other => other,
        };
        Self::ALL.into_iter().find(|quality| quality.id() == id)
    }

    /// The level out of `offers` that stands in for this one: the best at or below it, or the
    /// lowest offered when every one is above it. It is this level itself when `offers` is
    /// empty.
    pub fn within(self, offers: &[Offer]) -> Self {
        let levels = || offers.iter().map(|offer| offer.quality);
        let below = levels().filter(|level| *level <= self).max();
        below.or_else(|| levels().min()).unwrap_or(self)
    }
}

/// One level a service serves and what it streams at that level, so the listener can choose by
/// what they will hear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub quality: Quality,
    pub encoding: Encoding,
    /// The figures are the most a track comes at. Many come lower, such as an album that was
    /// never mastered in hi-res, or a plan that does not reach that far.
    pub up_to: bool,
}

/// What a stream or a file is encoded as: a lossless encoding with its bit depth, a lossy one
/// with its bit rate in kilobits a second.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Encoding {
    /// FLAC at this bit depth and sample rate in hertz.
    Flac {
        depth: u8,
        rate: u32,
    },
    /// Apple Lossless at this bit depth.
    Alac {
        depth: u8,
    },
    /// Uncompressed, as in WAV or AIFF, at this bit depth.
    Pcm {
        depth: u8,
    },
    Aac {
        kbps: u32,
    },
    Vorbis {
        kbps: u32,
    },
    Mp3 {
        kbps: u32,
    },
    Opus {
        kbps: u32,
    },
    /// A codec Wisp has no name for.
    Other,
}

/// What an engine knows about one track, reported as `PlaybackEvent::Signal` as it starts
/// decoding it.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackSignal {
    /// How the provider encodes it, when it says.
    pub encoding: Option<Encoding>,
    /// The rate and channel count it decodes to, before Wisp resamples it to the device's.
    pub rate: u32,
    pub channels: u16,
    pub normalisation: Normalisation,
}

/// What loudness normalisation does to a track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Normalisation {
    Off,
    /// A fixed gain in decibels, zero when nothing says how loud the track is.
    Gain(f32),
    /// A player that normalises inside itself, with a limiter, needs none of it, so
    /// the gain is not known and changes with the music.
    Limited,
}

/// What a server that converts audio on request is asked to send. Only such a server honours it,
/// the default leaves both choices to the server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Transcode {
    pub codec: Option<Codec>,
    /// The highest bitrate to send, in kilobits per second.
    pub bitrate: Option<u32>,
}

impl Encoding {
    /// The bits per sample of a lossless encoding, which a lossy one has none of.
    pub fn depth(self) -> Option<u8> {
        match self {
            Self::Flac { depth, .. } | Self::Alac { depth } | Self::Pcm { depth } => Some(depth),
            _ => None,
        }
    }
}

impl Transcode {
    /// The bitrates offered as a ceiling, highest first.
    pub const BITRATES: [u32; 7] = [320, 256, 192, 160, 128, 96, 64];
}

/// A lossy format a streaming server can convert a track into before sending it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Mp3,
    Opus,
    Aac,
}

impl Codec {
    pub const ALL: [Self; 3] = [Self::Mp3, Self::Opus, Self::Aac];

    /// The name the format is known by on the wire, which is also how settings store it.
    pub fn id(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Opus => "opus",
            Self::Aac => "aac",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|codec| codec.id() == id)
    }
}

/// What an engine reports back. `Playing` and `Seeked` mean audio from `at` is reaching the
/// output, not that a decoder is ready, so whatever follows the sound can start on them.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackEvent {
    Loading {
        id: Option<String>,
        at: Duration,
    },
    Playing {
        id: Option<String>,
        at: Duration,
    },
    Paused {
        id: Option<String>,
        at: Duration,
    },
    /// A progress report from the decoder. It runs ahead of what is audible by whatever the
    /// output has queued, and none arrive while the engine is busy with a seek.
    Position {
        id: Option<String>,
        at: Duration,
    },
    /// Audio from the new position has reached the output after a seek.
    Seeked {
        id: Option<String>,
        at: Duration,
    },
    Length {
        id: Option<String>,
        duration: Duration,
    },
    /// The whole of the current track has arrived, so fetching the next one takes nothing from
    /// it. Engines that cannot tell never send this.
    Downloaded {
        id: Option<String>,
    },
    Ended {
        id: Option<String>,
    },
    /// Track `id` could not be loaded. `timed_out` says the host it came from never answered in
    /// time, so the listener can be pointed at their connection rather than at the track.
    Unavailable {
        id: Option<String>,
        timed_out: bool,
    },
    /// The provider turned the load down for now, as it does under a rate limit, so the same
    /// track may play after a wait.
    Throttled {
        id: Option<String>,
    },
    Refused,
    Gated,
    /// The provider refuses the network the app connects from, so no track will load until the
    /// network changes.
    Blocked,
    /// The audio output was lost or changed, so the engine is gone. `heard` is the track being
    /// heard and how far into it the output had played, when the engine knew. A restart resumes
    /// there rather than where a clock that kept running during the switch would say.
    OutputChanged {
        heard: Option<(String, Duration)>,
    },
    /// What the engine knows about the sound of track `id`, sent as it starts decoding it. A
    /// track queued behind the current one reports before it is heard.
    Signal {
        id: String,
        track: TrackSignal,
    },
}

impl PlaybackEvent {
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Loading { id, .. }
            | Self::Playing { id, .. }
            | Self::Paused { id, .. }
            | Self::Position { id, .. }
            | Self::Seeked { id, .. }
            | Self::Length { id, .. }
            | Self::Downloaded { id }
            | Self::Ended { id, .. }
            | Self::Unavailable { id, .. }
            | Self::Throttled { id } => id.as_deref(),
            Self::Signal { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// Transport control of one engine. Every call is fire-and-forget: the outcome arrives as a
/// `PlaybackEvent`, never as a return value.
pub trait Player: Send + Sync {
    /// Fetches a track and plays it from `at`. A `seamless` load is a queue segue: a gapless
    /// engine keeps what it has queued so the join has no gap, any other load drops it.
    fn load(&self, track_id: &str, at: Duration, seamless: bool) -> Result<()>;

    /// Fetches a track and leaves it paused at `at`, ready for `play`.
    fn load_paused_at(&self, track_id: &str, at: Duration) -> Result<()>;

    /// Fetches a track ahead of time so a later `load` starts at once. `segue` marks the next
    /// queue item, which a gapless engine may already line up behind the current one.
    fn preload(&self, track_id: &str, segue: bool) -> Result<()>;

    /// Takes back what `preload` lined up behind the current track, as when a change of repeat
    /// altered what follows it. An engine that cannot is left as it is.
    fn unline(&self) {}

    /// Whether the engine can honour ordered local queue placements.
    fn can_order_queue(&self) -> bool {
        true
    }

    fn play(&self);
    fn pause(&self);

    /// Moves to `position`. While loading, the track starts there instead; while playing, a
    /// `Seeked` event follows once audio from there reaches the output.
    fn seek(&self, position: Duration);
    fn set_gain(&self, gain: f32);

    /// 无缝变速：写入共享控制，由音频线程在下一个缓冲周期应用。
    fn set_speed(&self, _speed: f32) {}

    fn spectrum(&self) -> Option<Spectrum> {
        None
    }
}

#[async_trait]
pub trait PlaybackEvents: Send {
    async fn next(&mut self) -> Option<PlaybackEvent>;
}

pub trait PlaybackFactory: Send + Sync {
    fn start(&self, config: PlaybackConfig) -> (Box<dyn Player>, Box<dyn PlaybackEvents>);

    /// The stream levels the next engine serves, when they differ from the ones the provider
    /// signed in with. A provider with a second way to play answers here, since which one is in
    /// use can change while the account stays signed in.
    fn qualities(&self) -> Option<&'static [Offer]> {
        None
    }
}

/// What a provider's library is made of. It decides which `MusicApi` methods fill the library
/// pages and whether a favorites filter is offered on them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Shape {
    /// The library is what the user starred, read through the `saved_*` methods.
    Saved,
    /// The library is a catalog read through the `all_*` methods, with the `saved_*` set drawn
    /// on top as hearts and a filter.
    Catalog(Catalog),
}

/// A collection a radio can be started from, the way a track seeds one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RadioSeed {
    Album,
    Playlist,
    Artist,
}

impl Shape {
    pub fn is_catalog(self) -> bool {
        matches!(self, Self::Catalog(_))
    }

    /// Whether the listener can put things into the library and take them out, apart from
    /// favoriting them.
    pub fn editable(self) -> bool {
        self == Self::Catalog(Catalog::Editable)
    }
}

/// Whether the listener can change what a catalog holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Catalog {
    /// Everything the provider has, which changes only with the files behind it, as with local
    /// files and a self-hosted server.
    Fixed,
    /// A set the listener picked out of a bigger catalog, which tracks and albums are put into
    /// and taken out of through `set_in_library`. A favorite is a mark on a
    /// library item, so taking an item out of the library takes its favorite with it.
    Editable,
}

/// What a provider can do beyond listing and playing, so a control it has no answer for is
/// never put in front of the listener.
///
/// This is about the service, not the account: something a provider simply does not have, like
/// a station the provider will not list or a play count it does not keep. A capability that
/// is off hides its button, its menu item and its column, rather than showing one that fails
/// when pressed. A provider that gains one flips a flag here and the UI follows.
/// The result of a provider's daily membership check-in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckinReport {
    pub date: String,
    pub claimed: bool,
    pub upgraded: bool,
    pub claim_code: Option<i64>,
    pub upgrade_code: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The listener can follow and unfollow an artist.
    pub follow_artists: bool,
    /// A track can seed a station, which is what fills the queue behind it.
    pub radio: bool,
    /// The provider understands the FM mode and song-pool switches.
    pub radio_modes: bool,
    /// The provider offers a daily membership check-in.
    pub checkin: bool,
    /// Tracks carry a play count worth a column of its own.
    pub playcounts: bool,
    /// The provider keeps sidebar pins of its own, listed by `pin_targets` and changed
    /// through `set_pinned`. Off, a pin lives in Wisp's settings alone.
    pub pins: bool,
    /// The server converts tracks on request, as [`Transcode`] asks.
    pub transcode: bool,
    /// The listener can give an owned playlist a cover of their own through
    /// `set_playlist_cover`, and take it off again.
    pub playlist_covers: bool,
    /// The listener can create a smart playlist and change an owned one's rules, through
    /// `create_smart_playlist` and `update_smart_playlist`. Off, the rule editor stays
    /// hidden rather than saving nowhere.
    pub smart_playlists: bool,
    /// The listener can move tracks within an owned playlist through `reorder_playlist`,
    /// and the provider keeps the new order.
    pub playlist_order: bool,
    /// An owned playlist can be switched between public and private through
    /// `set_playlist_public`.
    pub playlist_visibility: bool,
    /// Played tracks can be kept in the [`vault`], so playing one again downloads nothing.
    pub keeps: bool,
    /// The stream levels the service serves, best first. Empty when there is nothing to choose
    /// between, so the setting stays hidden.
    pub qualities: &'static [Offer],
    /// Set when the library's Songs page comes in the order its songs were added but carries
    /// no date for each, saying which end is newest. The page then sorts by Date added through
    /// that order and shows no Date added column. None when the songs have dates of their own
    /// or come in some other order.
    pub song_arrival: Option<Arrival>,
    /// The same, for the songs of a playlist the listener owns. Anyone else's playlist keeps
    /// the order its curator gave it.
    pub playlist_arrival: Option<Arrival>,
}

impl Capabilities {
    /// What a full streaming service offers. Pins of the provider's own are not among them,
    /// since few providers keep any. Converting on request is not either, since only a server
    /// that transcodes does it. Playlist covers, order and smart playlists are left for each
    /// provider to turn on.
    pub const ALL: Self = Self {
        follow_artists: true,
        radio: true,
        radio_modes: false,
        checkin: false,
        playcounts: true,
        pins: false,
        transcode: false,
        playlist_covers: false,
        smart_playlists: false,
        playlist_order: false,
        playlist_visibility: true,
        keeps: true,
        qualities: &[],
        song_arrival: None,
        playlist_arrival: None,
    };

    /// Nothing beyond listing and playing.
    pub const NONE: Self = Self {
        follow_artists: false,
        radio: false,
        radio_modes: false,
        checkin: false,
        playcounts: false,
        pins: false,
        transcode: false,
        playlist_covers: false,
        smart_playlists: false,
        playlist_order: false,
        playlist_visibility: false,
        keeps: false,
        qualities: &[],
        song_arrival: None,
        playlist_arrival: None,
    };
}

pub struct ProviderSession {
    pub profile: UserProfile,
    pub api: Arc<dyn MusicApi>,
    pub playback: Arc<dyn PlaybackFactory>,
    pub shape: Shape,
    pub authenticated: bool,
    pub capabilities: Capabilities,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignIn {
    Default,
    Anonymous,
    Secret,
    Path(Vec<PathBuf>),
    /// [wisp] 网易云：扫码登录（二维码内容由 `SignInPrompt::Qr` 给出）。
    Qr,
    /// [wisp] 网易云：手机号 + 短信验证码登录，走 `SignInPrompt::Phone`。
    Phone,
    /// [wisp] 酷狗：账号 + 密码登录，走 `SignInPrompt::Password`。
    Password,
    /// [wisp] 酷狗：QQ 扫码登录。
    Qq,
    /// [wisp] 酷狗：微信扫码登录。
    WeChat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInProblem {
    Region,
    Credentials,
    /// The server did not accept the username and password typed for a new sign-in.
    Password,
    Network,
    /// The provider refuses the network the app connects from. Another network gets through.
    Blocked,
    Cancelled,
    Refused,
}

/// A provider refusing for now because too much was asked of it too quickly. Whoever asked can
/// try again later, and finds this in the error's chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Throttled;

impl std::fmt::Display for Throttled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the provider is refusing for now")
    }
}

impl std::error::Error for Throttled {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignInFailure(pub SignInProblem);

impl std::fmt::Display for SignInFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self.0 {
            SignInProblem::Region => "the account is out of its home region",
            SignInProblem::Credentials => "the stored credentials are no longer valid",
            SignInProblem::Password => "the server did not accept that username and password",
            SignInProblem::Network => "the provider could not be reached",
            SignInProblem::Blocked => "the provider refuses requests from this network",
            SignInProblem::Cancelled => "authorization was cancelled in the browser",
            SignInProblem::Refused => "the provider refused the session",
        };
        write!(f, "{reason}")
    }
}

impl std::error::Error for SignInFailure {}

/// One account or profile a sign-in can go on as. The picked `id` comes back on the prompt's
/// input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountChoice {
    pub id: String,
    pub name: String,
    pub detail: Option<String>,
}

/// What the choices in a `SignInPrompt::Accounts` are, which decides how the picker asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountKind {
    /// Separate accounts that one browser session is signed in to at once.
    Session,
    /// Profiles inside one account, such as the members of a family plan.
    Profile,
}

/// The accounts a sign-in asks the user to pick from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountChoices {
    pub kind: AccountKind,
    pub accounts: Vec<AccountChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInPrompt {
    Accounts(AccountChoices),
    Code {
        code: String,
        url: String,
    },
    Url(String),
    Secret,
    /// [wisp] 扫码登录：`url` 是二维码内容（手机扫码后跳转的地址），
    /// `hint` 是状态提示（已扫码待确认等），由服务端轮询更新。
    Qr {
        url: String,
        hint: Option<String>,
    },
    /// [wisp] 手机号验证码登录。`notice` 是状态提示（如「验证码已发送」）。
    /// 输入通道收到的文本约定：
    ///   `send:<手机号>`            —— 请求发送验证码
    ///   `login:<手机号>\n<验证码>` —— 提交登录
    Phone {
        notice: Option<String>,
    },
    /// [wisp] 账号密码登录。输入通道收到的文本约定：
    ///   `start`                —— 开始，显示输入框
    ///   `账号\n密码`           —— 提交登录
    Password {
        notice: Option<String>,
    },
    /// [wisp] 二次安全验证。`context` 是 provider 给的 JSON/提示，输入通道收到用户提交的验证码。
    Verify {
        kind: String,
        context: String,
    },
}

pub type PromptSink = Arc<dyn Fn(SignInPrompt) + Send + Sync>;
pub type InputSource = tokio::sync::mpsc::UnboundedReceiver<String>;

/// One page of a listing that arrives in pieces. `total` is how long the whole listing will
/// be, on the pages of a provider that knows before the last one; a page that does not know
/// carries `None`, and the count so far stands in.
pub struct Page<T> {
    pub total: Option<usize>,
    pub items: Vec<T>,
}

/// A listing arriving a page at a time, in order. The channel closes after the last page, or
/// carries the error a page broke on, after which nothing more comes. Dropping it stops the
/// provider fetching.
pub type Pages<T> = tokio::sync::mpsc::Receiver<Result<Page<T>>>;

/// The home feed arriving a lot at a time: every message is the whole feed so far, arranged
/// the way the provider wants it drawn, so each one can replace the last on the page. The
/// channel closes after the last lot, or carries the error one broke on, after which nothing
/// more comes. Dropping it stops the provider fetching.
pub type Feed = tokio::sync::mpsc::Receiver<Result<HomeFeed>>;

/// A home feed that arrived whole, as its one and only message.
pub fn at_once(feed: HomeFeed) -> Feed {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    // Room for one message was made above, so this never waits and never fails.
    sender.try_send(Ok(feed)).ok();
    receiver
}

/// A listing that arrived whole, as its one and only page. What a provider that lists in one
/// go answers the paged calls with.
pub fn whole<T: Send + 'static>(items: Vec<T>) -> Pages<T> {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let total = Some(items.len());
    // Room for one page was made above, so this never waits and never fails.
    sender.try_send(Ok(Page { total, items })).ok();
    receiver
}

/// A cookie sign-in the app runs in its own browser window. `url` opens first and `landing` scopes
/// URL-based cookie reads. The user is through once the cookies for `domain` carry one of the
/// `proof` names. The header those cookies make is what `SignInPrompt::Secret` then receives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebSignIn {
    pub url: &'static str,
    pub landing: &'static str,
    pub domain: &'static str,
    pub proof: &'static [&'static str],
    /// A user agent the window presents instead of its default. Some providers' anti-bot
    /// checks reject an agent that does not match the engine behind it (a Firefox string on a
    /// WebKit window); the string must match what the backend's engine would say. Backends
    /// whose default is already engine-consistent may ignore it.
    pub agent: Option<&'static str>,
}

/// One of the libraries a self-hosted server keeps apart, which a stored account can be limited
/// to. It is kept beside the account so a launch can name the choice before the server is asked,
/// and so a library the server no longer lists is still reported by name rather than silently
/// widened back to everything.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Library {
    pub id: i64,
    /// The server's name for it, when it has one.
    #[serde(default)]
    pub name: Option<String>,
}

#[async_trait]
pub trait MusicProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn slug(&self) -> &'static str;

    /// Whether a stored account can be limited to one library of its server. Only such a
    /// provider answers the other library calls.
    fn has_libraries(&self) -> bool {
        false
    }
    /// The library the stored account is limited to, or `None` when it reads the whole server.
    /// It reads what is stored and asks the server nothing.
    fn library(&self) -> Option<Library> {
        None
    }
    /// Every library the stored account's server offers.
    async fn libraries(&self) -> Result<Vec<Library>> {
        anyhow::bail!("this provider has no libraries")
    }
    /// Keeps the library the stored account reads, or the whole server with `None`. It only
    /// stores the choice, so a live session applies it once it is rebuilt.
    fn choose_library(&self, _library: Option<Library>) -> Result<()> {
        anyhow::bail!("this provider has no libraries")
    }

    /// Forgets whatever the provider remembers about its last scan, so the next one reads
    /// everything again. Only a provider that scans files has anything to forget, and only a
    /// rescan the user asked for should ask it to.
    fn forget_scan(&self) {}
    /// A factory for the provider's playback engine that can work without sign-in and scan,
    /// so playback can start before the library has loaded. Only for a provider whose tracks
    /// are files.
    fn playback_factory(&self) -> Option<Arc<dyn PlaybackFactory>> {
        None
    }
    /// Reads an arbitrary file on disk as a track, for a provider whose tracks are files. Used
    /// by file-association opens, which may point outside any scanned folder.
    fn track_from_path(&self, _path: &Path) -> Option<Track> {
        None
    }
    fn sign_in_options(&self) -> Vec<SignIn>;
    fn stored(&self) -> bool;
    /// Whether what is stored is an anonymous session rather than an account, so a caller
    /// can tell the two apart. A provider without an anonymous sign-in never says yes.
    fn stored_guest(&self) -> bool {
        false
    }
    fn location(&self) -> Option<String> {
        None
    }
    /// The host to open a connection to when checking whether the network is back. `None`
    /// where the provider needs no network, which is what keeps a local library from ever
    /// looking for one.
    fn reach(&self) -> Option<String> {
        None
    }
    /// What a status calls this provider after "listening to". A service answers with its own
    /// name; one that is only the user's own files says what the files are instead.
    fn listening_to(&self) -> &'static str {
        self.name()
    }
    /// Whether the artwork urls this provider hands out can be given to another service. A path
    /// on disk means nothing elsewhere, and a self-hosted url carries the credentials that fetch
    /// it, so the default is no.
    fn public_art(&self) -> bool {
        false
    }
    /// A playback factory built from the stored sign-in alone, with no network, for a provider
    /// that can play what it cached when the account cannot be reached. The session starts it
    /// when the account is unreachable, inside the app's tokio runtime.
    fn offline_playback(&self) -> Option<Arc<dyn PlaybackFactory>> {
        None
    }
    /// What `link` points at when it is one of this provider's own, on its website or in its
    /// app's scheme. It reads the text alone and fetches nothing, so a short link comes back as
    /// `Linked::Short` for the caller to follow and ask again.
    fn link(&self, _link: &str) -> Option<link::Linked> {
        None
    }
    async fn restore(&self) -> Result<Option<ProviderSession>>;
    async fn sign_in(
        &self,
        method: SignIn,
        prompt: PromptSink,
        input: InputSource,
    ) -> Result<ProviderSession>;
    fn abandon(&self) {}
    fn sign_out(&self);
    /// How to run `SignIn::Secret` in a browser window. `None` means the provider has no cookie
    /// sign-in, and the app offers no `Secret` option for it.
    fn web_sign_in(&self) -> Option<WebSignIn> {
        None
    }
}

/// Leniently convert an ISO8601 timestamp to unix epoch seconds. Accepts only
/// the date portion, date and time portions, or full date and time with offset.
pub fn iso_8601_to_epoch(value: Option<&str>) -> Option<i64> {
    let value = value?;
    let parser = DateTimeParser::new();
    let timestamp = parser
        .parse_timestamp(value)
        .or_else(|_| {
            parser
                .parse_datetime(value)
                .and_then(|datetime| jiff::tz::TimeZone::UTC.to_timestamp(datetime))
        })
        .or_else(|_| {
            parser
                .parse_date(value)
                .and_then(|date| jiff::tz::TimeZone::UTC.to_timestamp(date.at(0, 0, 0, 0)))
        })
        .ok()?;
    Some(timestamp.as_second())
}

/// Keeps playable songs credited to the artist in their supplied order, collapsing repeat
/// ids and matching titles of similar duration across releases. Remaster labels do not
/// distinguish songs.
fn artist_songs(tracks: Vec<Track>, artist_id: &str) -> Vec<Track> {
    let mut kept: HashMap<String, Vec<Duration>> = HashMap::new();
    let mut ids = HashSet::new();
    tracks
        .into_iter()
        .filter(|track| {
            if !track.playable {
                return false;
            }
            let named = track
                .artist_refs
                .iter()
                .filter_map(|artist| artist.id.as_deref());
            named.clone().next().is_none() || named.into_iter().any(|id| id == artist_id)
        })
        .filter(|track| {
            if let Some(id) = &track.id
                && !ids.insert(id.clone())
            {
                return false;
            }
            let lengths = kept.entry(song_title(&track.name)).or_default();
            let repeat = lengths.iter().any(|length| {
                !length.is_zero()
                    && !track.duration.is_zero()
                    && length.abs_diff(track.duration) <= SAME_SONG
            });
            if !repeat {
                lengths.push(track.duration);
            }
            !repeat
        })
        .collect()
}

/// A track title reduced to what tells songs apart: lowercase, without a remaster note in
/// brackets or after a dash, and with its spacing collapsed.
fn song_title(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut rest = match lower.rsplit_once(" - ") {
        Some((head, tail)) if remaster_note(tail) => head,
        _ => lower.as_str(),
    };
    let mut title = String::with_capacity(rest.len());
    while let Some(open) = rest.find(['(', '[']) {
        let close = match rest.as_bytes()[open] {
            b'(' => ')',
            _ => ']',
        };
        let Some(length) = rest[open..].find(close) else {
            break;
        };
        let end = open + length + 1;
        title.push_str(&rest[..open]);
        if !remaster_note(&rest[open + 1..end - 1]) {
            title.push_str(&rest[open..end]);
        }
        rest = &rest[end..];
    }
    title.push_str(rest);
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Recognizes remaster labels without discarding version names such as live or acoustic.
fn remaster_note(note: &str) -> bool {
    note.contains("remaster")
        && note.split_whitespace().all(|word| {
            matches!(
                word,
                "remaster" | "remastered" | "digitally" | "digital" | "version"
            ) || word.chars().all(|character| character.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_8601_to_epoch_parses_correctly() {
        // None and malformed input
        assert_eq!(iso_8601_to_epoch(None), None);
        assert_eq!(iso_8601_to_epoch(Some("")), None);
        assert_eq!(iso_8601_to_epoch(Some("malformed")), None);

        // Basic epoch format
        assert_eq!(iso_8601_to_epoch(Some("1970-01-01")), Some(0));
        assert_eq!(iso_8601_to_epoch(Some("1970-01-01T00:00:00")), Some(0));
        assert_eq!(iso_8601_to_epoch(Some("1970-01-01T00:00:00Z")), Some(0));
        assert_eq!(
            iso_8601_to_epoch(Some("1970-01-01T00:00:00+00:00")),
            Some(0)
        );

        // A specific date with/without time/offset
        let date = jiff::civil::date(2021, 3, 20);
        let midnight = jiff::tz::TimeZone::UTC
            .to_timestamp(date.at(0, 0, 0, 0))
            .unwrap()
            .as_second();
        let afternoon = jiff::tz::TimeZone::UTC
            .to_timestamp(date.at(13, 45, 7, 0))
            .unwrap()
            .as_second();
        assert_eq!(iso_8601_to_epoch(Some("2021-03-20")), Some(midnight));
        assert_eq!(
            iso_8601_to_epoch(Some("2021-03-20T13:45:07")),
            Some(afternoon)
        );
        assert_eq!(
            iso_8601_to_epoch(Some("2021-03-20T13:45:07.123456")),
            Some(afternoon)
        );
        assert_eq!(
            iso_8601_to_epoch(Some("2021-03-20T13:45:07Z")),
            Some(afternoon)
        );
        assert_eq!(
            iso_8601_to_epoch(Some("2021-03-20T13:45:07+00:00")),
            Some(afternoon)
        );
        assert_eq!(
            iso_8601_to_epoch(Some("2021-03-20T10:45:07-03:00")),
            Some(afternoon)
        );
    }
}
