use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;

/// The signed-in account a provider hands back after a successful sign-in or restore.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileDto {
    pub id: String,
    pub display_name: String,
    pub avatar: Option<String>,
}

/// A credited artist as the provider names it. Mirrors the host's `ArtistRef`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistRefDto {
    pub name: String,
    pub id: Option<String>,
    pub channel: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CreditDto {
    pub name: String,
    pub role: String,
    pub id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContributorDto {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
}

/// A track as it crosses the ABI. Field-for-field the host's `Track`, with the duration in
/// milliseconds so no `std::time::Duration` crosses the boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackDto {
    pub id: Option<String>,
    pub name: String,
    pub playable: bool,
    /// Sold on its own (a digital single or album) and not yet unlocked by this account, as
    /// opposed to a row only a membership tier opens.
    pub paid: bool,
    pub artists: String,
    pub artist_refs: Vec<ArtistRefDto>,
    pub album: String,
    pub album_artist: String,
    pub album_id: Option<String>,
    pub cover: Option<String>,
    pub duration_ms: u64,
    pub added_at: Option<i64>,
    pub added_by: Option<ContributorDto>,
    pub playcount: Option<u64>,
    pub popularity: u32,
    pub explicit: bool,
    pub track_number: u32,
    pub disc_number: u32,
    pub tags: Vec<String>,
    pub languages: Vec<String>,
    pub credits: Vec<CreditDto>,
}

/// `search` arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchArgs {
    pub query: String,
}

/// `search` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchReply {
    pub tracks: Vec<TrackDto>,
}

/// An artist as the search page lists it. Mirrors the host's `SavedArtist`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedArtistDto {
    pub id: String,
    pub name: String,
    pub cover: Option<String>,
    pub added_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchAlbumsReply {
    pub albums: Vec<AlbumDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchPlaylistsReply {
    pub playlists: Vec<PlaylistDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchArtistsReply {
    pub artists: Vec<SavedArtistDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchPlaylistsArgs {
    pub query: String,
    /// The signed-in user id, so the plugin can mark a playlist as owned.
    pub me: i64,
}

/// An artist page as it crosses the ABI. Mirrors the host's `Artist`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistDto {
    pub name: String,
    pub cover_large: Option<String>,
    pub banner: Option<String>,
    pub biography: Option<String>,
    pub monthly_listeners: Option<u64>,
    pub followers: Option<u64>,
    pub top_tracks: Vec<TrackDto>,
    pub albums: Vec<AlbumDto>,
}

/// Mirrors the host's `ArtistProfile`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistProfileDto {
    pub name: String,
    pub cover_large: Option<String>,
    pub biography: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistArgs {
    pub artist_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistImagesArgs {
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistReply {
    pub artist: ArtistDto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistTracksReply {
    pub tracks: Vec<TrackDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistProfileReply {
    pub profile: ArtistProfileDto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistImagesReply {
    pub images: BTreeMap<String, String>,
}

/// Arguments for the signed-in library ops.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryArgs {
    pub me: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedTracksReply {
    pub tracks: Vec<TrackDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedAlbumsReply {
    pub albums: Vec<AlbumDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedArtistsReply {
    pub artists: Vec<SavedArtistDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistsReply {
    pub playlists: Vec<PlaylistDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackArgs {
    pub track_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackReply {
    pub track: TrackDto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackPlaycountReply {
    pub playcount: Option<u64>,
}

/// One climax segment of a song, in milliseconds from its start.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClimaxMarkDto {
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClimaxReply {
    pub marks: Vec<ClimaxMarkDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenreDto {
    pub id: String,
    pub name: String,
    pub cover: Option<String>,
}

/// One card of a home shelf. The host's `GenreItem`, tagged so the JSON stays unambiguous.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "item", rename_all = "snake_case")]
pub enum GenreItemDto {
    Playlist(PlaylistDto),
    Album(AlbumDto),
    Genre(GenreDto),
    Track(TrackDto),
    Artist(SavedArtistDto),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenreSectionDto {
    pub title: String,
    pub items: Vec<GenreItemDto>,
    /// A provider genre id for the shelf's full page. None keeps the page on the items this
    /// feed already carried.
    pub more: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HomeFeedDto {
    pub listen_again: Vec<GenreItemDto>,
    pub quick_picks: Option<Vec<TrackDto>>,
    pub sections: Vec<GenreSectionDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HomeArgs {
    /// The parts of the feed this call should carry; empty asks for the whole feed. A provider
    /// that declares `capabilities.home_parts` names its shelves here so the host can ask in
    /// stages.
    #[serde(default)]
    pub parts: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HomeReply {
    pub feed: HomeFeedDto,
}

/// Mirrors the host's `GenreDetail`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenreDetailDto {
    pub name: String,
    pub sections: Vec<GenreSectionDto>,
    /// The offset to ask for next, when the provider pages this genre. None ends the listing.
    pub next_offset: Option<usize>,
    pub total: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenreArgs {
    pub genre_id: String,
    /// How many cards the caller already holds; zero for the first page.
    pub offset: usize,
    /// The largest page the caller wants; zero lets the provider choose.
    pub limit: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenresReply {
    pub genres: Vec<GenreDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenreReply {
    pub detail: GenreDetailDto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenreCoverReply {
    pub cover: Option<String>,
}

/// Mirrors the host's `AlbumCatalogue`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlbumCatalogueDto {
    pub also_like: Vec<AlbumDto>,
    pub similar: Vec<SavedArtistDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlbumCatalogueArgs {
    pub album_id: String,
    pub artist_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlbumCatalogueReply {
    pub catalogue: AlbumCatalogueDto,
}

/// Mirrors the host's `ArtistCatalogue`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistCatalogueDto {
    pub albums: Vec<AlbumDto>,
    pub top_tracks: Vec<TrackDto>,
    pub appears_on: Vec<AlbumDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistCatalogueArgs {
    pub artist_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistCatalogueReply {
    pub catalogue: ArtistCatalogueDto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistCoversArgs {
    pub playlist_id: String,
    pub wanted: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistCoversReply {
    pub covers: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackRadioArgs {
    pub track_id: String,
    pub from: Option<String>,
    /// FM mode: `normal`, `small`, `peak` or `radio`.
    pub mode: Option<String>,
    /// FM song pool: 0 taste, 1 style, 2 explore.
    pub song_pool_id: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckinArgs {
    /// The day to claim as `YYYY-MM-DD`; `None` lets the provider read its server date.
    pub day: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckinReply {
    pub date: String,
    pub claimed: bool,
    pub upgraded: bool,
    pub claim_code: Option<i64>,
    pub upgrade_code: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackRadioReply {
    pub tracks: Vec<TrackDto>,
    pub continuation: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SetTrackSavedArgs {
    pub track_id: String,
    pub saved: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DislikeArgs {
    pub track_id: String,
}

/// The reply of a write op that only reports success or a `PluginError`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AckReply {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CreatePlaylistArgs {
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CreatePlaylistReply {
    pub playlist_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenamePlaylistArgs {
    pub playlist_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeletePlaylistArgs {
    pub playlist_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistIdArgs {
    pub playlist_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SetPlaylistPublicArgs {
    pub playlist_id: String,
    pub public: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AddTrackToPlaylistArgs {
    pub playlist_id: String,
    pub track_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoveTrackFromPlaylistArgs {
    pub playlist_id: String,
    pub track_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReorderPlaylistArgs {
    pub playlist_id: String,
    pub seen: Vec<String>,
    pub wanted: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SetPlaylistCoverArgs {
    pub playlist_id: String,
    /// The new cover as a base64 string; `None` asks to remove it.
    pub image_base64: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SetAlbumSavedArgs {
    pub album_id: String,
    pub saved: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SetArtistSavedArgs {
    pub artist_id: String,
    pub saved: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchKeywordsArgs {
    pub keywords: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchKeywordsReply {
    pub keywords: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverSizeDto {
    #[default]
    Large,
    Full,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CoverAtArgs {
    pub cover: String,
    pub size: CoverSizeDto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CoverAtReply {
    pub cover: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKindDto {
    #[default]
    Track,
    Album,
    Artist,
    Playlist,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareUrlArgs {
    pub kind: MediaKindDto,
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareUrlReply {
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileArgs {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileReply {
    pub profile: ProfileDto,
}

/// The release kind an album names. Mirrors the host's `ReleaseType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReleaseTypeDto {
    #[default]
    Album,
    Single,
    Compilation,
    Ep,
    Audiobook,
    Podcast,
}

/// An album as it crosses the ABI. Mirrors the host's `Album`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlbumDto {
    pub id: String,
    pub name: String,
    pub artists: String,
    pub artist_refs: Vec<ArtistRefDto>,
    pub cover: Option<String>,
    pub cover_large: Option<String>,
    pub release_type: ReleaseTypeDto,
    pub year: i32,
    pub track_count: u32,
    pub release_date: String,
    pub label: String,
    pub copyrights: Vec<String>,
    pub added_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlbumDetailDto {
    pub album: AlbumDto,
    pub tracks: Vec<TrackDto>,
}

/// A playlist as it crosses the ABI. Mirrors the host's `Playlist`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistDto {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub owner_id: String,
    pub owned: bool,
    pub collaborative: bool,
    pub blend: bool,
    pub public: bool,
    pub cover: Option<String>,
    pub track_count: u32,
    pub modified_at: Option<i64>,
    pub read_only: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistDetailDto {
    pub playlist: PlaylistDto,
    pub tracks: Vec<TrackDto>,
    pub continuation: Option<String>,
}

/// A provider-side smart playlist. `rules` is the criteria JSON as a string so the ABI does
/// not freeze the rule model the host's editor keeps evolving.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartPlaylistDto {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub public: bool,
    pub owner: String,
    pub owned: bool,
    pub rules: Option<String>,
    pub synced: bool,
    pub track_count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartPlaylistReply {
    pub playlist: SmartPlaylistDto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CreateSmartPlaylistArgs {
    pub name: String,
    pub rules: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSmartPlaylistArgs {
    pub playlist_id: String,
    pub name: Option<String>,
    pub comment: Option<String>,
    pub public: Option<bool>,
    pub rules: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlbumArgs {
    pub album_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistArgs {
    pub playlist_id: String,
    /// The signed-in user id, so the plugin can mark a playlist as owned.
    pub me: i64,
}

/// Asks for the next stretch of a playlist whose first call answered a `continuation`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistContinuationArgs {
    pub continuation: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistContinuationReply {
    pub tracks: Vec<TrackDto>,
    pub continuation: Option<String>,
}

/// A page of items plus the provider's total, when it knows one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub total: u32,
    pub items: Vec<T>,
}

/// One channel of a provider response: either items or the reason that channel failed. Used
/// where the host asks several searches at once and wants one failure not to sink the rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Channel<T> {
    Items { total: u32, items: Vec<T> },
    Error { code: ErrorCode, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_track_round_trips() {
        let track = TrackDto {
            id: Some("123".to_owned()),
            name: "晴天".to_owned(),
            playable: true,
            artists: "周杰伦".to_owned(),
            artist_refs: vec![ArtistRefDto {
                name: "周杰伦".to_owned(),
                id: Some("6452".to_owned()),
                channel: false,
            }],
            album: "叶惠美".to_owned(),
            album_id: Some("18905".to_owned()),
            cover: Some("https://p1.music.126.net/a.jpg".to_owned()),
            duration_ms: 269_000,
            popularity: 100,
            track_number: 3,
            disc_number: 1,
            ..TrackDto::default()
        };
        let text = serde_json::to_string(&track).expect("serialize");
        let again: TrackDto = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(again, track);

        let reply = SearchReply {
            tracks: vec![track],
        };
        let text = serde_json::to_string(&reply).expect("serialize");
        let again: SearchReply = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(again, reply);
    }
}
