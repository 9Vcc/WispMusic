//! Frozen `op` numbers for the single `wisp_call(op, arg)` entry point.
//!
//! Ranges are capability groups, not trait order: adding a method appends inside its group so
//! an existing number never moves. The ABI only grows; a removed op stays reserved.

/// `MusicApi` — session, covers and share links.
pub mod music_api {
    pub const ALIVE: u32 = 0x0100;
    pub const COVER_AT: u32 = 0x0101;
    pub const COVER_ORIGINAL: u32 = 0x0102;
    pub const SHARE_URL: u32 = 0x0103;
    pub const PROFILE: u32 = 0x0104;
    pub const USER: u32 = 0x0105;

    /// Artist pages and the saved-artist library.
    pub const ARTIST: u32 = 0x0110;
    pub const ARTIST_CATALOGUE: u32 = 0x0111;
    pub const ARTIST_TRACKS: u32 = 0x0112;
    pub const ARTIST_PROFILE: u32 = 0x0113;
    pub const ARTIST_IMAGES: u32 = 0x0114;
    pub const SAVED_ARTISTS: u32 = 0x0115;
    pub const ALL_ARTISTS: u32 = 0x0116;
    pub const SAVED_ARTISTS_PAGED: u32 = 0x0117;
    pub const ALL_ARTISTS_PAGED: u32 = 0x0118;
    pub const SET_ARTIST_SAVED: u32 = 0x0119;

    /// Search entry points.
    pub const SEARCH_KEYWORDS: u32 = 0x0120;
    pub const SEARCH: u32 = 0x0121;
    pub const SEARCH_ALBUMS: u32 = 0x0122;
    pub const SEARCH_PLAYLISTS: u32 = 0x0123;
    pub const SEARCH_ARTISTS: u32 = 0x0124;
    pub const SEARCH_ALL: u32 = 0x0125;

    /// Album pages and the saved-album library.
    pub const ALBUM: u32 = 0x0130;
    pub const ALBUM_TRACKS: u32 = 0x0131;
    pub const ALBUM_CATALOGUE: u32 = 0x0132;
    pub const SAVED_ALBUMS: u32 = 0x0133;
    pub const ALL_ALBUMS: u32 = 0x0134;
    pub const SAVED_ALBUMS_PAGED: u32 = 0x0135;
    pub const ALL_ALBUMS_PAGED: u32 = 0x0136;
    pub const SET_ALBUM_SAVED: u32 = 0x0137;

    /// Home feed, genres and recently played.
    pub const HOME: u32 = 0x0140;
    pub const HOME_PAGED: u32 = 0x0141;
    pub const NAME_HOME_PLAYLISTS: u32 = 0x0142;
    pub const GENRES: u32 = 0x0143;
    pub const GENRE: u32 = 0x0144;
    pub const GENRE_COVER: u32 = 0x0145;
    pub const RECENTLY_PLAYED: u32 = 0x0146;

    /// Tracks and the track library.
    pub const SAVED_TRACKS: u32 = 0x0150;
    pub const SAVED_TRACKS_PAGED: u32 = 0x0151;
    pub const ALL_TRACKS: u32 = 0x0152;
    pub const ALL_TRACKS_PAGED: u32 = 0x0153;
    pub const SET_TRACK_SAVED: u32 = 0x0154;
    pub const TRACK: u32 = 0x0155;
    pub const TRACK_TAGS: u32 = 0x0156;
    pub const SET_TRACK_TAGS: u32 = 0x0157;
    pub const DELETE_TRACK_FILE: u32 = 0x0158;
    pub const TRACK_PLAYCOUNT: u32 = 0x0159;
    pub const TRACK_RADIO: u32 = 0x015A;
    pub const REPORT: u32 = 0x015B;
    pub const PLAYED: u32 = 0x015C;
    pub const REPORT_PLAY: u32 = 0x015D;
    pub const DISLIKE: u32 = 0x015E;
    /// The song's climax segments, for the marks over the seek bar.
    pub const CLIMAX: u32 = 0x015F;

    /// Playlists, pins and playlist mutations.
    pub const PLAYLISTS: u32 = 0x0160;
    pub const SET_PINNED: u32 = 0x0161;
    pub const PIN_TARGETS: u32 = 0x0162;
    pub const PIN_URI: u32 = 0x0163;
    pub const PLAYLIST: u32 = 0x0164;
    pub const PLAYLIST_CONTINUATION: u32 = 0x0165;
    pub const PLAYLIST_TRACKS: u32 = 0x0166;
    pub const PLAYLIST_COVERS: u32 = 0x0167;
    pub const CREATE_PLAYLIST: u32 = 0x0168;
    pub const RENAME_PLAYLIST: u32 = 0x0169;
    pub const DELETE_PLAYLIST: u32 = 0x016A;
    pub const REMOVE_PLAYLIST_FROM_LIBRARY: u32 = 0x016B;
    pub const ADD_PLAYLIST_TO_LIBRARY: u32 = 0x016C;
    pub const SET_IN_LIBRARY: u32 = 0x016D;
    pub const SET_PLAYLIST_PUBLIC: u32 = 0x016E;
    pub const SET_PLAYLIST_COVER: u32 = 0x016F;

    /// Smart playlists, playlist track editing and import/export.
    pub const SMART_PLAYLIST: u32 = 0x0170;
    pub const CREATE_SMART_PLAYLIST: u32 = 0x0171;
    pub const UPDATE_SMART_PLAYLIST: u32 = 0x0172;
    pub const ADD_TRACK_TO_PLAYLIST: u32 = 0x0173;
    pub const ADD_TRACKS_TO_PLAYLIST: u32 = 0x0174;
    pub const REMOVE_TRACK_FROM_PLAYLIST: u32 = 0x0175;
    pub const REORDER_PLAYLIST: u32 = 0x0176;
    pub const IMPORT_PLAYLIST: u32 = 0x0177;
    pub const EXPORT_PLAYLIST: u32 = 0x0178;

    pub const ALL: [u32; 78] = [
        ALIVE,
        COVER_AT,
        COVER_ORIGINAL,
        SHARE_URL,
        PROFILE,
        USER,
        ARTIST,
        ARTIST_CATALOGUE,
        ARTIST_TRACKS,
        ARTIST_PROFILE,
        ARTIST_IMAGES,
        SAVED_ARTISTS,
        ALL_ARTISTS,
        SAVED_ARTISTS_PAGED,
        ALL_ARTISTS_PAGED,
        SET_ARTIST_SAVED,
        SEARCH_KEYWORDS,
        SEARCH,
        SEARCH_ALBUMS,
        SEARCH_PLAYLISTS,
        SEARCH_ARTISTS,
        SEARCH_ALL,
        ALBUM,
        ALBUM_TRACKS,
        ALBUM_CATALOGUE,
        SAVED_ALBUMS,
        ALL_ALBUMS,
        SAVED_ALBUMS_PAGED,
        ALL_ALBUMS_PAGED,
        SET_ALBUM_SAVED,
        HOME,
        HOME_PAGED,
        NAME_HOME_PLAYLISTS,
        GENRES,
        GENRE,
        GENRE_COVER,
        RECENTLY_PLAYED,
        SAVED_TRACKS,
        SAVED_TRACKS_PAGED,
        ALL_TRACKS,
        ALL_TRACKS_PAGED,
        SET_TRACK_SAVED,
        TRACK,
        TRACK_TAGS,
        SET_TRACK_TAGS,
        DELETE_TRACK_FILE,
        TRACK_PLAYCOUNT,
        TRACK_RADIO,
        REPORT,
        PLAYED,
        REPORT_PLAY,
        DISLIKE,
        CLIMAX,
        PLAYLISTS,
        SET_PINNED,
        PIN_TARGETS,
        PIN_URI,
        PLAYLIST,
        PLAYLIST_CONTINUATION,
        PLAYLIST_TRACKS,
        PLAYLIST_COVERS,
        CREATE_PLAYLIST,
        RENAME_PLAYLIST,
        DELETE_PLAYLIST,
        REMOVE_PLAYLIST_FROM_LIBRARY,
        ADD_PLAYLIST_TO_LIBRARY,
        SET_IN_LIBRARY,
        SET_PLAYLIST_PUBLIC,
        SET_PLAYLIST_COVER,
        SMART_PLAYLIST,
        CREATE_SMART_PLAYLIST,
        UPDATE_SMART_PLAYLIST,
        ADD_TRACK_TO_PLAYLIST,
        ADD_TRACKS_TO_PLAYLIST,
        REMOVE_TRACK_FROM_PLAYLIST,
        REORDER_PLAYLIST,
        IMPORT_PLAYLIST,
        EXPORT_PLAYLIST,
    ];
}

/// `MusicProvider` — identity, libraries and the sign-in lifecycle.
pub mod provider {
    pub const NAME: u32 = 0x0200;
    pub const SLUG: u32 = 0x0201;
    pub const HAS_LIBRARIES: u32 = 0x0202;
    pub const LIBRARY: u32 = 0x0203;
    pub const LIBRARIES: u32 = 0x0204;
    pub const CHOOSE_LIBRARY: u32 = 0x0205;
    pub const FORGET_SCAN: u32 = 0x0206;
    pub const PLAYBACK_FACTORY: u32 = 0x0207;
    pub const TRACK_FROM_PATH: u32 = 0x0208;
    pub const SIGN_IN_OPTIONS: u32 = 0x0209;
    pub const STORED: u32 = 0x020A;
    pub const STORED_GUEST: u32 = 0x020B;
    pub const LOCATION: u32 = 0x020C;
    pub const REACH: u32 = 0x020D;
    pub const LISTENING_TO: u32 = 0x020E;
    pub const PUBLIC_ART: u32 = 0x020F;
    pub const OFFLINE_PLAYBACK: u32 = 0x0210;
    pub const LINK: u32 = 0x0211;
    pub const RESTORE: u32 = 0x0212;
    pub const SIGN_IN: u32 = 0x0213;
    pub const ABANDON: u32 = 0x0214;
    pub const SIGN_OUT: u32 = 0x0215;
    pub const WEB_SIGN_IN: u32 = 0x0216;

    pub const ALL: [u32; 23] = [
        NAME,
        SLUG,
        HAS_LIBRARIES,
        LIBRARY,
        LIBRARIES,
        CHOOSE_LIBRARY,
        FORGET_SCAN,
        PLAYBACK_FACTORY,
        TRACK_FROM_PATH,
        SIGN_IN_OPTIONS,
        STORED,
        STORED_GUEST,
        LOCATION,
        REACH,
        LISTENING_TO,
        PUBLIC_ART,
        OFFLINE_PLAYBACK,
        LINK,
        RESTORE,
        SIGN_IN,
        ABANDON,
        SIGN_OUT,
        WEB_SIGN_IN,
    ];
}

/// `LyricsProvider`.
pub mod lyrics {
    pub const NAME: u32 = 0x0300;
    pub const SEARCH: u32 = 0x0301;

    pub const ALL: [u32; 2] = [NAME, SEARCH];
}

/// Extension ops that have no `MusicApi` equivalent.
pub mod ext {
    /// Resolve a track to a [`crate::StreamPlan`]; bytes stay in the host.
    pub const STREAM_RESOLVE: u32 = 0x0400;
    /// Claim the provider's daily membership reward.
    pub const CHECKIN: u32 = 0x0401;

    pub const ALL: [u32; 2] = [STREAM_RESOLVE, CHECKIN];
}

/// Whether `op` is one the v1 table defines.
pub fn is_known(op: u32) -> bool {
    music_api::ALL.contains(&op)
        || provider::ALL.contains(&op)
        || lyrics::ALL.contains(&op)
        || ext::ALL.contains(&op)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn the_table_matches_the_trait_surface() {
        assert_eq!(music_api::ALL.len(), 78);
        assert_eq!(provider::ALL.len(), 23);
        assert_eq!(lyrics::ALL.len(), 2);
    }

    #[test]
    fn every_number_is_unique() {
        let mut seen = HashSet::new();
        for op in music_api::ALL
            .iter()
            .chain(provider::ALL.iter())
            .chain(lyrics::ALL.iter())
            .chain(ext::ALL.iter())
        {
            assert!(seen.insert(*op), "duplicate op {op:#06x}");
        }
    }

    #[test]
    fn groups_keep_their_ranges() {
        assert!(
            music_api::ALL
                .iter()
                .all(|op| (0x0100..0x0180).contains(op))
        );
        assert!(provider::ALL.iter().all(|op| (0x0200..0x0220).contains(op)));
        assert!(lyrics::ALL.iter().all(|op| (0x0300..0x0310).contains(op)));
        assert!(ext::ALL.iter().all(|op| (0x0400..0x0410).contains(op)));
    }

    #[test]
    fn unknown_ops_are_reported_as_such() {
        assert!(!is_known(0x0000));
        assert!(!is_known(0x0199));
        assert!(!is_known(0x0999));
        assert!(is_known(music_api::SEARCH));
        assert!(is_known(provider::SIGN_IN));
        assert!(is_known(ext::STREAM_RESOLVE));
    }
}
