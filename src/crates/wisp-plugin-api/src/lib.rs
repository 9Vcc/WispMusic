//! The versioned contract between WispMusic and its streaming-service provider plugins.
//!
//! This crate is deliberately small: `serde` only. It must never depend on `music`, `state`,
//! `gpui`, tokio or a network stack, so a plugin can build against it for a WASM target without
//! pulling the host in.

#![forbid(unsafe_code)]

pub mod dto;
pub mod error;
pub mod host;
pub mod manifest;
pub mod ops;
pub mod sign_in;
pub mod stream;

pub use dto::{
    AckReply, AddTrackToPlaylistArgs, AlbumArgs, AlbumCatalogueArgs, AlbumCatalogueDto,
    AlbumCatalogueReply, AlbumDetailDto, AlbumDto, ArtistArgs, ArtistCatalogueArgs,
    ArtistCatalogueDto, ArtistCatalogueReply, ArtistDto, ArtistImagesArgs, ArtistImagesReply,
    ArtistProfileDto, ArtistProfileReply, ArtistRefDto, ArtistReply, ArtistTracksReply, Channel,
    CheckinArgs, CheckinReply, ClimaxMarkDto, ClimaxReply, ContributorDto, CoverAtArgs, CoverAtReply, CoverSizeDto,
    CreatePlaylistArgs, CreatePlaylistReply, CreateSmartPlaylistArgs, CreditDto,
    DeletePlaylistArgs, DislikeArgs, GenreArgs, GenreCoverReply, GenreDetailDto, GenreDto,
    GenreItemDto, GenreReply, GenreSectionDto, GenresReply, HomeArgs, HomeFeedDto, HomeReply,
    LibraryArgs, MediaKindDto, Page, PlaylistArgs, PlaylistContinuationArgs,
    PlaylistContinuationReply, PlaylistCoversArgs, PlaylistCoversReply, PlaylistDetailDto,
    PlaylistDto, PlaylistIdArgs, PlaylistsReply, ProfileArgs, ProfileDto,
    ProfileReply, ReleaseTypeDto, RemoveTrackFromPlaylistArgs, RenamePlaylistArgs,
    ReorderPlaylistArgs, SavedAlbumsReply, SavedArtistDto, SavedArtistsReply, SavedTracksReply,
    SearchAlbumsReply, SearchArgs, SearchArtistsReply, SearchKeywordsArgs, SearchKeywordsReply,
    SearchPlaylistsArgs, SearchPlaylistsReply, SearchReply, SetAlbumSavedArgs, SetArtistSavedArgs,
    SetPlaylistCoverArgs, SetPlaylistPublicArgs, SetTrackSavedArgs, ShareUrlArgs, ShareUrlReply,
    SmartPlaylistDto, SmartPlaylistReply, TrackArgs, TrackDto, TrackPlaycountReply, TrackRadioArgs,
    TrackRadioReply, TrackReply, UpdateSmartPlaylistArgs,
};
pub use error::{ErrorCode, PluginError};
pub use host::{Host, HttpMethod, HttpRequest, HttpResponse, LogLevel};
pub use manifest::{
    AbiVersion, Assets, CapabilitiesDto, Declares, EncodingDto, I18n, LocalizedText, Manifest,
    NetworkPolicy, OfferDto, Publisher, StoragePolicy,
};
pub use sign_in::{
    FLOW_PASSWORD, FLOW_PHONE, FLOW_QQ, FLOW_QR, FLOW_SECRET, FLOW_WECHAT, Prompt, SignInStep,
    Verify, VerifyKind,
};
pub use stream::{Header, StreamPlan};

/// ABI version negotiated through the manifest. A host accepts `major == 1` and refuses other
/// majors; minor versions are additive.
pub const ABI_MAJOR: u16 = 1;
pub const ABI_MINOR: u16 = 0;

/// The largest payload a single `wisp_call` may return. Larger replies fail with
/// [`error::ErrorCode::TooLarge`] so the host can fall back to paging.
pub const MAX_REPLY_BYTES: u64 = 4 * 1024 * 1024;

/// Bytes at the start of every plugin buffer: a little-endian `u32` payload length followed by
/// that many UTF-8 JSON bytes.
pub const REPLY_HEADER_BYTES: u32 = 4;
