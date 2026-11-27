smart-rules-name-placeholder = Playlist name
playlist-create-local = Local files
playlist-delete-confirm = Delete “{ $name }”? This cannot be undone.
# login
login-restoring = Checking your saved session…
login-authorizing = Waiting for authorization in your browser…
login-signed-in = Signed in as { $name }
login-failed-title = Sign-in failed
login-problem-region = The service will not open a session from your country. Sign in from where the account is registered.
login-problem-credentials = Your saved session for this service is no longer valid. Sign in again to continue.
login-problem-password = The server did not accept that username and password. Check them and try again.
login-problem-network = Wisp could not reach the music service. Check your internet connection and try again.
login-problem-blocked = The music service is refusing connections from your network. If you use a VPN, try another server or turn it off.
login-problem-cancelled = The sign-in was not approved in the browser, or ran out of time. Start again to finish.
login-problem-refused = The service turned down the sign-in. Wait a moment and try again.
login-sign-in = Sign in with { $provider }
# [wisp] NetEase: QR and phone/email sign-in
login-sign-in-qr = Sign in by QR code
login-qr-title = Scan to sign in
login-qr-scan = Scan the code with the NetEase Cloud Music app

login-connect-cookies = Paste cookies manually
login-cookie-open = Open { $provider }
login-cookie-submit = Continue
login-cookie-hint = Paste the Cookie request header here
login-cookie-named-title = Paste your { $provider } cookie to finish signing in
login-cookie-named-hint = Paste the cookie value here
login-cookie-named-step-1 = Open { $site } and make sure you are signed in.
login-cookie-named-step-2 = Press F12 and open the Storage tab in Firefox, or the Application tab in Chrome.
login-cookie-named-step-3 = Expand Cookies, select { $site } and find the cookie named { $cookie }.
login-cookie-named-step-4 = Double-click its value and copy it.
login-cookie-named-note = The { $cookie } value alone is enough. A whole Cookie header copied from the Network tab works too.
login-window-title = Sign in to { $provider }
login-guest-title = Guest mode
login-guest-use = Use Guest mode
login-device-code = Enter this code at { $url }
login-account-title = Choose an account
login-account-detail = This session is signed in to more than one Google account. Pick the one Wisp should use.
login-profile-title = Choose a profile
login-profile-detail = This account has more than one profile. Pick the one Wisp should use.
login-choose-title = Sign in to { $provider }
login-choose-detail = Choose how to sign in to { $provider }.
login-providers = Music services
login-welcome-title = Welcome to Wisp
login-welcome-detail = You can add more services later in Settings.
login-or = or

# album and playlist pages
detail-playlist = Playlist

play-shuffle = Shuffle

# artist page
artist-eyebrow = Artist
artist-monthly-listeners = { $count ->
    [one] { $value } monthly listener
   *[other] { $value } monthly listeners
}
artist-followers = { $count ->
    [one] { $value } follower
   *[other] { $value } followers
}
artist-popular = Popular
artist-popular-eyebrow = Explore this artist
artist-popular-empty = Nothing to play from this artist yet
artist-popular-more = Show all
artist-popular-less = Show less
artist-favorite-songs = Your favorite songs
artist-favorite-songs-count = { $count ->
    [one] { $count } song in your Favorites
   *[other] { $count } songs in your Favorites
}
artist-releases = Releases
artist-releases-more = Show all
artist-releases-less = Show less
artist-filter-all = All
artist-filter-albums = Albums
artist-filter-singles = Singles
artist-filter-eps = EPs
artist-appears-on = Appears on
album-also-like = You might also like
album-tab-albums = Albums
album-tab-artists = Artists
album-label = ℗ { $label }

# user profile page
user-eyebrow = Profile
user-followers = { $count ->
    [one] { $value } follower
   *[other] { $value } followers
}
user-following = { $count ->
   *[other] { $value } following
}
user-playlists = Public playlists
user-playlists-empty = No public playlists yet

# home page
home-see-more = See more
home-quick-picks = Quick picks
home-quick-picks-empty = Like a few songs and they will show up here
home-recently-added = Recently added
home-playlists = Playlists
home-favorite-albums = Favorite albums
home-artists = Artists
home-collection-albums = Albums from your collection

# powerbar (quick search)
powerbar-title = Quick search
powerbar-placeholder = What do you want to listen to?
powerbar-open = Open
powerbar-song = Go to song

# search page
search-placeholder = What do you want to listen to?
search-browse = Browse all
search-submit = Search
search-recent = Recent searches
search-forget = Remove from history
genre-load-more = Load more
genre-loading-more = Loading…
genre-load-failed = Could not load more — retry
genre-empty = Nothing to show here yet
search-best-match = Best match
search-no-matches = No matches
search-results = Results
search-songs = Songs
search-artists = Artists
search-albums-playlists = Albums & playlists
search-tag = { $kind } ·
search-saved =
    { $count ->
        [one] { $count } song in Library
       *[other] { $count } songs in Library
    }
# song page
song-eyebrow = Song
song-view-album = View album
song-about = About this song
song-album = Album
song-released = Released
song-streams = Streams
song-position = Position
song-label = Label
song-popularity = Popularity
song-popularity-value = { $value }%
song-disc-track = Disc { $disc }, track { $track }
song-track = Track { $track }
song-credits = Credits
song-performed-by = Performed by
song-details = Genres & details
song-genres = Genres
song-language = Language
song-content = Content
song-explicit = Explicit
song-paid = Paid
song-local = Local file
song-clean = Clean
artist-about = About the artist
artist-about-fallback = Explore the artist's popular songs and releases.
artist-about-open = Go to artist
song-copyright = © { $notice }
track-row-name = { $title } by { $artists }

theme-unavailable = { $name } (unavailable)
saver-fps = { $fps } FPS

radio-tagline = Picks tuned to your taste, updated as you listen
home-fm-now = Roaming: { $name }
radio-play = Play
radio-refresh = New picks
radio-dislike = Not for me
radio-empty = Nothing picked yet. Sign in and try again
radio-loading = Picking for you…
daily-empty = Nothing for today yet. Sign in for a daily update
daily-loading = Preparing today’s picks…
daily-refresh = Refresh
# tags
tags-edit-title = Edit tags
tags-sheet-song = Song
tags-sheet-album = Album
tags-sheet-details = Details
tags-title = Title
tags-artist = Artist
tags-artists = Artists
tags-add-artist = Add an artist
tags-remove-artist = Remove this artist
tags-track = Track number
tags-track-total = Tracks on release
tags-disc = Disc number
tags-disc-total = Discs in release
tags-album = Album
tags-album-artist = Album artist
tags-year = Year
tags-genre = Genre
tags-composer = Composer
tags-publisher = Publisher
tags-isrc = ISRC
tags-comment = Comment
# [wisp] NetEase: SMS code sign-in
login-sign-in-phone = Sign in with a code
login-phone-title = Sign in with a code
login-phone-number = Phone number
login-phone-code = SMS code
login-phone-send = Send code
login-phone-retry = Retry in { $seconds }s
login-phone-invalid = Enter a valid 11-digit phone number
login-phone-code-invalid = Enter the 4-6 digit code
login-phone-request-code = Request a code first
login-phone-submit = Sign in
# [wisp] Kugou: account and third-party QR sign-in
login-sign-in-password = Sign in with an account
login-sign-in-qq = QQ QR code
login-sign-in-wechat = WeChat QR code
login-password-title = Sign in with an account
login-password-username = Username
login-password-password = Password
login-password-submit = Sign in
login-password-username-empty = Enter your username
login-password-password-empty = Enter your password
# [wisp] Kugou login page
login-provider-title = Sign in to { $provider }
login-provider-subtitle = Choose how to sign in to your { $provider } account
login-provider-connecting = Connecting to { $provider }…
login-provider-qr-scan = Scan with the { $provider } app
login-kugou-title = Sign in to Kugou Music
login-kugou-subtitle = Choose how to sign in to your Kugou account
login-kugou-method-kugou = Kugou
login-kugou-method-phone = Code
login-kugou-method-qq = QQ
login-kugou-method-wechat = WeChat
login-kugou-connecting = Connecting to Kugou…
login-kugou-retry = Retry
login-kugou-refresh = Refresh QR code
login-kugou-back = Back
login-kugou-qr-kugou = Scan with the Kugou Music app
login-kugou-qr-qq = Scan with QQ
login-kugou-qr-wechat = Scan with WeChat
login-kugou-qr-confirm = Scanned; confirm on your phone
login-kugou-phone-hint = Enter your phone number to receive a code
login-phone-sent = The code has been sent

home-daily = Daily recommendations
home-daily-detail = Tailored for you
home-radio = Personal radio
home-radio-detail = Good vibes start here
login-verify-title = Security check
login-verify-code = Verification code
login-verify-submit = Verify
login-verify-detail = Kugou asks for a security check: { $hint }
login-verify-detail-plain = Complete the Kugou { $kind } check, then enter the code.
radio-card-title = For you
radio-now-playing = Now playing
radio-now-empty = No recommendations to show yet.
radio-mode-normal = Loved
radio-mode-small = Niche
radio-mode-peak = Quick
radio-mode-radio = Station
radio-mode-title-normal = Loved Radio
radio-mode-title-small = Niche Radio
radio-mode-title-peak = Quick Radio
radio-mode-title-radio = Station Radio
radio-pool-taste = Taste
radio-pool-style = Style
radio-pool-explore = Explore
