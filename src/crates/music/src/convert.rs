//! `wisp_plugin_api` DTO → host model conversions.
//!
//! These impls are shared by the native client and the WASM provider; they do not provide
//! any provider data themselves.

use std::sync::Arc;
use std::time::Duration;

impl From<wisp_plugin_api::TrackDto> for crate::Track {
    fn from(dto: wisp_plugin_api::TrackDto) -> Self {
        Self {
            id: dto.id,
            name: dto.name,
            playable: dto.playable,
            paid: dto.paid,
            artists: dto.artists,
            artist_refs: artist_refs(dto.artist_refs),
            album: dto.album,
            album_artist: dto.album_artist,
            album_id: dto.album_id,
            cover: dto.cover,
            duration: Duration::from_millis(dto.duration_ms),
            added_at: dto.added_at,
            added_by: dto.added_by.map(|contributor| {
                Arc::new(crate::Contributor {
                    id: contributor.id,
                    name: contributor.name,
                    avatar: contributor.avatar,
                })
            }),
            playcount: dto.playcount,
            popularity: dto.popularity,
            explicit: dto.explicit,
            track_number: dto.track_number,
            disc_number: dto.disc_number,
            tags: dto.tags,
            languages: dto.languages,
            credits: dto
                .credits
                .into_iter()
                .map(|credit| crate::Credit {
                    name: credit.name,
                    role: credit.role,
                    id: credit.id,
                })
                .collect(),
        }
    }
}

impl From<wisp_plugin_api::ReleaseTypeDto> for crate::ReleaseType {
    fn from(value: wisp_plugin_api::ReleaseTypeDto) -> Self {
        match value {
            wisp_plugin_api::ReleaseTypeDto::Album => Self::Album,
            wisp_plugin_api::ReleaseTypeDto::Single => Self::Single,
            wisp_plugin_api::ReleaseTypeDto::Compilation => Self::Compilation,
            wisp_plugin_api::ReleaseTypeDto::Ep => Self::Ep,
            wisp_plugin_api::ReleaseTypeDto::Audiobook => Self::Audiobook,
            wisp_plugin_api::ReleaseTypeDto::Podcast => Self::Podcast,
        }
    }
}

impl From<wisp_plugin_api::AlbumDto> for crate::Album {
    fn from(dto: wisp_plugin_api::AlbumDto) -> Self {
        Self {
            id: dto.id,
            name: dto.name,
            artists: dto.artists,
            artist_refs: artist_refs(dto.artist_refs),
            cover: dto.cover,
            cover_large: dto.cover_large,
            release_type: dto.release_type.into(),
            year: dto.year,
            track_count: dto.track_count,
            release_date: dto.release_date,
            label: dto.label,
            copyrights: dto.copyrights,
            added_at: dto.added_at,
        }
    }
}

impl From<wisp_plugin_api::AlbumDetailDto> for crate::AlbumDetail {
    fn from(dto: wisp_plugin_api::AlbumDetailDto) -> Self {
        Self {
            album: dto.album.into(),
            tracks: dto.tracks.into_iter().map(crate::Track::from).collect(),
        }
    }
}

impl From<wisp_plugin_api::PlaylistDto> for crate::Playlist {
    fn from(dto: wisp_plugin_api::PlaylistDto) -> Self {
        Self {
            id: dto.id,
            name: dto.name,
            owner: dto.owner,
            owner_id: dto.owner_id,
            owned: dto.owned,
            collaborative: dto.collaborative,
            blend: dto.blend,
            public: dto.public,
            cover: dto.cover,
            track_count: dto.track_count,
            modified_at: dto.modified_at,
            read_only: dto.read_only,
        }
    }
}

impl From<wisp_plugin_api::PlaylistDetailDto> for crate::PlaylistDetail {
    fn from(dto: wisp_plugin_api::PlaylistDetailDto) -> Self {
        Self {
            playlist: dto.playlist.into(),
            tracks: dto.tracks.into_iter().map(crate::Track::from).collect(),
            continuation: dto.continuation,
        }
    }
}

impl From<wisp_plugin_api::SavedArtistDto> for crate::SavedArtist {
    fn from(dto: wisp_plugin_api::SavedArtistDto) -> Self {
        Self {
            id: dto.id,
            name: dto.name,
            cover: dto.cover,
            added_at: dto.added_at,
        }
    }
}

impl From<wisp_plugin_api::ProfileDto> for crate::UserProfile {
    fn from(dto: wisp_plugin_api::ProfileDto) -> Self {
        Self {
            id: dto.id,
            display_name: dto.display_name,
            avatar: dto.avatar,
        }
    }
}

impl From<wisp_plugin_api::ArtistDto> for crate::Artist {
    fn from(dto: wisp_plugin_api::ArtistDto) -> Self {
        Self {
            name: dto.name,
            cover_large: dto.cover_large,
            banner: dto.banner,
            biography: dto.biography,
            monthly_listeners: dto.monthly_listeners,
            followers: dto.followers,
            top_tracks: dto.top_tracks.into_iter().map(crate::Track::from).collect(),
            albums: dto.albums.into_iter().map(crate::Album::from).collect(),
        }
    }
}

impl From<wisp_plugin_api::ArtistProfileDto> for crate::ArtistProfile {
    fn from(dto: wisp_plugin_api::ArtistProfileDto) -> Self {
        Self {
            name: dto.name,
            cover_large: dto.cover_large,
            biography: dto.biography,
        }
    }
}

impl From<wisp_plugin_api::GenreDto> for crate::Genre {
    fn from(dto: wisp_plugin_api::GenreDto) -> Self {
        Self {
            id: dto.id,
            name: dto.name,
            cover: dto.cover,
        }
    }
}

impl From<wisp_plugin_api::GenreItemDto> for crate::GenreItem {
    fn from(dto: wisp_plugin_api::GenreItemDto) -> Self {
        match dto {
            wisp_plugin_api::GenreItemDto::Playlist(playlist) => Self::Playlist(playlist.into()),
            wisp_plugin_api::GenreItemDto::Album(album) => Self::Album(album.into()),
            wisp_plugin_api::GenreItemDto::Genre(genre) => Self::Genre(genre.into()),
            wisp_plugin_api::GenreItemDto::Track(track) => Self::Track(track.into()),
            wisp_plugin_api::GenreItemDto::Artist(artist) => Self::Artist(artist.into()),
        }
    }
}

impl From<wisp_plugin_api::GenreSectionDto> for crate::GenreSection {
    fn from(dto: wisp_plugin_api::GenreSectionDto) -> Self {
        Self {
            title: dto.title,
            items: dto.items.into_iter().map(crate::GenreItem::from).collect(),
            more: dto.more,
        }
    }
}

impl From<wisp_plugin_api::GenreDetailDto> for crate::GenreDetail {
    fn from(dto: wisp_plugin_api::GenreDetailDto) -> Self {
        Self {
            name: dto.name,
            next_offset: dto.next_offset,
            total: dto.total,
            sections: dto
                .sections
                .into_iter()
                .map(crate::GenreSection::from)
                .collect(),
        }
    }
}

impl From<wisp_plugin_api::AlbumCatalogueDto> for crate::AlbumCatalogue {
    fn from(dto: wisp_plugin_api::AlbumCatalogueDto) -> Self {
        Self {
            also_like: dto.also_like.into_iter().map(crate::Album::from).collect(),
            similar: dto
                .similar
                .into_iter()
                .map(crate::SavedArtist::from)
                .collect(),
        }
    }
}

impl From<wisp_plugin_api::ArtistCatalogueDto> for crate::ArtistCatalogue {
    fn from(dto: wisp_plugin_api::ArtistCatalogueDto) -> Self {
        Self {
            albums: dto.albums.into_iter().map(crate::Album::from).collect(),
            top_tracks: dto.top_tracks.into_iter().map(crate::Track::from).collect(),
            appears_on: dto.appears_on.into_iter().map(crate::Album::from).collect(),
        }
    }
}

impl From<wisp_plugin_api::HomeFeedDto> for crate::HomeFeed {
    fn from(dto: wisp_plugin_api::HomeFeedDto) -> Self {
        Self {
            listen_again: dto
                .listen_again
                .into_iter()
                .map(crate::GenreItem::from)
                .collect(),
            quick_picks: dto
                .quick_picks
                .map(|tracks| tracks.into_iter().map(crate::Track::from).collect()),
            sections: dto
                .sections
                .into_iter()
                .map(crate::GenreSection::from)
                .collect(),
        }
    }
}

fn artist_refs(items: Vec<wisp_plugin_api::ArtistRefDto>) -> Vec<crate::ArtistRef> {
    items
        .into_iter()
        .map(|artist| crate::ArtistRef {
            name: artist.name,
            id: artist.id,
            channel: artist.channel,
        })
        .collect()
}
