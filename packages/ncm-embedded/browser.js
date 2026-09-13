// 浏览器版模块入口：静态注册 @neteasecloudmusicapienhanced/api@4.30.2 的模块层
// 路由映射复刻 server.js 规则的逆过程；request/网络能力由外部注入
const { cookieToJson } = require('./util/index')

const modules = {
  album: require('./module/album.js'),
  album_new: require('./module/album_new.js'),
  album_sub: require('./module/album_sub.js'),
  artist_album: require('./module/artist_album.js'),
  artist_desc: require('./module/artist_desc.js'),
  artist_detail: require('./module/artist_detail.js'),
  artist_list: require('./module/artist_list.js'),
  artist_songs: require('./module/artist_songs.js'),
  artist_sub: require('./module/artist_sub.js'),
  artist_sublist: require('./module/artist_sublist.js'),
  artist_top_song: require('./module/artist_top_song.js'),
  avatar_upload: require('./module/avatar_upload.js'),
  captcha_sent: require('./module/captcha_sent.js'),
  captcha_verify: require('./module/captcha_verify.js'),
  check_music: require('./module/check_music.js'),
  cloud_upload_complete: require('./module/cloud_upload_complete.js'),
  cloud_upload_token: require('./module/cloud_upload_token.js'),
  cloudsearch: require('./module/cloudsearch.js'),
  fm_trash: require('./module/fm_trash.js'),
  homepage_block_page: require('./module/homepage_block_page.js'),
  like: require('./module/like.js'),
  likelist: require('./module/likelist.js'),
  login: require('./module/login.js'),
  login_cellphone: require('./module/login_cellphone.js'),
  login_qr_check: require('./module/login_qr_check.js'),
  login_qr_create: require('./module/login_qr_create.js'),
  login_qr_key: require('./module/login_qr_key.js'),
  login_refresh: require('./module/login_refresh.js'),
  login_status: require('./module/login_status.js'),
  logout: require('./module/logout.js'),
  lyric_new: require('./module/lyric_new.js'),
  personal_fm: require('./module/personal_fm.js'),
  playlist_catlist: require('./module/playlist_catlist.js'),
  playlist_cover_update: require('./module/playlist_cover_update.js'),
  playlist_create: require('./module/playlist_create.js'),
  playlist_delete: require('./module/playlist_delete.js'),
  playlist_detail: require('./module/playlist_detail.js'),
  playlist_subscribe: require('./module/playlist_subscribe.js'),
  playlist_track_all: require('./module/playlist_track_all.js'),
  playlist_tracks: require('./module/playlist_tracks.js'),
  playlist_update: require('./module/playlist_update.js'),
  recent_listen_list: require('./module/recent_listen_list.js'),
  recommend_songs: require('./module/recommend_songs.js'),
  record_recent_album: require('./module/record_recent_album.js'),
  record_recent_playlist: require('./module/record_recent_playlist.js'),
  record_recent_song: require('./module/record_recent_song.js'),
  register_anonimous: require('./module/register_anonimous.js'),
  search_default: require('./module/search_default.js'),
  search_suggest: require('./module/search_suggest.js'),
  simi_artist: require('./module/simi_artist.js'),
  song_detail: require('./module/song_detail.js'),
  song_download_url_v1: require('./module/song_download_url_v1.js'),
  song_music_detail: require('./module/song_music_detail.js'),
  song_order_update: require('./module/song_order_update.js'),
  song_url_match: require('./module/song_url_match.js'),
  song_url_v1: require('./module/song_url_v1.js'),
  top_album: require('./module/top_album.js'),
  top_playlist: require('./module/top_playlist.js'),
  top_playlist_highquality: require('./module/top_playlist_highquality.js'),
  top_song: require('./module/top_song.js'),
  toplist: require('./module/toplist.js'),
  toplist_detail: require('./module/toplist_detail.js'),
  user_account: require('./module/user_account.js'),
  user_cloud: require('./module/user_cloud.js'),
  user_cloud_del: require('./module/user_cloud_del.js'),
  user_detail: require('./module/user_detail.js'),
  user_playlist: require('./module/user_playlist.js'),
  user_subcount: require('./module/user_subcount.js'),
  user_update: require('./module/user_update.js'),
}

// 例外表：server.js 中这三个路由保留下划线
const ROUTE_EXCEPTIONS = {
  '/personal_fm': 'personal_fm',
  '/fm_trash': 'fm_trash',
  '/daily_signin': 'daily_signin',
}

function getModuleName(path) {
  if (ROUTE_EXCEPTIONS[path]) return ROUTE_EXCEPTIONS[path]
  return path.replace(/^\//, '').replace(/\//g, '_')
}

async function callEmbeddedApi(path, params, requestImpl, externals) {
  const moduleName = getModuleName(path)
  const module = modules[moduleName]
  if (typeof module !== 'function') {
    return Promise.reject({
      status: 404,
      body: { code: 404, msg: `未知接口: ${path}` },
    })
  }

  let cookie = params.cookie
  if (typeof cookie === 'string') {
    cookie = cookieToJson(cookie)
  }

  const query = { ...params, cookie: cookie || {} }
  return module(query, requestImpl, externals)
}

module.exports = { callEmbeddedApi, getModuleName }
