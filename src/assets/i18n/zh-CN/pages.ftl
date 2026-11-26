playlist-delete-confirm = 删除“{ $name }”？此操作无法撤销。
# login
login-restoring = 正在检查已保存的会话…
login-authorizing = 正在浏览器中等待授权…
login-signed-in = 已登录为 { $name }
login-failed-title = 登录失败
login-problem-region = 该服务不会从你所在的国家/地区开启会话。请从账号注册的国家/地区登录。
login-problem-credentials = 你保存的会话已失效。请重新登录以继续。
login-problem-network = Wisp 无法连接到音乐服务。请检查网络连接并重试。
login-problem-blocked = 音乐服务拒绝来自当前网络的连接。如果使用了 VPN，请换一个节点或关闭后重试。
login-problem-cancelled = 你在批准登录之前关闭了浏览器页面。请重新开始以完成登录。
login-problem-refused = 服务拒绝了登录请求。请稍等片刻后重试。
login-sign-in = 使用 { $provider } 登录
# [wisp] 网易云：扫码与手机号/邮箱登录
login-sign-in-qr = 扫码登录
login-qr-title = 扫码登录
login-qr-scan = 打开网易云音乐 App，扫一扫二维码

login-connect-cookies = 手动粘贴 Cookies
login-cookie-open = 打开 { $provider }
login-cookie-submit = 继续
login-cookie-hint = 在此粘贴 Cookie 请求头
login-cookie-named-title = 粘贴你的 { $provider } Cookie 以完成登录
login-cookie-named-hint = 在此粘贴 Cookie 值
login-cookie-named-step-1 = 打开 { $site } 并确保你已登录。
login-cookie-named-step-2 = 按 F12，在 Firefox 中打开存储（Storage）标签页，或在 Chrome 中打开应用（Application）标签页。
login-cookie-named-step-3 = 展开 Cookies，选择 { $site }，然后找到名为 { $cookie } 的 Cookie。
login-cookie-named-step-4 = 双击其值并复制。
login-cookie-named-note = 只需 { $cookie } 的值即可。从网络（Network）标签页复制的完整 Cookie 请求头也可以。
login-window-title = 使用 { $provider } 登录
login-guest-title = 访客模式
login-guest-use = 使用访客模式
login-offline-title = 离线
login-offline-use = 离线使用本地音乐
login-device-code = 在 { $url } 输入此代码
login-account-title = 选择一个账户
login-account-detail = 此会话登录了多个 Google 账户。请选择 Wisp 应使用的账户。
login-choose-title = 登录 { $provider }
login-choose-detail = 选择登录 { $provider } 的方式。

# album and playlist pages
detail-playlist = 播放列表

play-shuffle = 随机播放

# artist page
artist-eyebrow = 歌手
artist-monthly-listeners = { $count ->
    [one] { $value } 位每月听众
   *[other] { $value } 位每月听众
}
artist-popular = 热门
artist-popular-eyebrow = 探索这位歌手
artist-popular-empty = 这位歌手暂无可播放内容
artist-popular-more = 显示全部
artist-popular-less = 收起
artist-favorite-songs-count = { $count ->
   *[other] 收藏了 { $count } 首歌
}
artist-releases = 发行作品
artist-releases-more = 显示全部
artist-releases-less = 收起
artist-filter-all = 全部
artist-filter-albums = 专辑
artist-filter-singles = 单曲
artist-filter-eps = EP
artist-appears-on = 收录于
album-also-like = 你可能也喜欢
album-tab-albums = 专辑
album-tab-artists = 歌手
album-label = ℗ { $label }

# user profile page
user-eyebrow = 个人资料
user-followers = { $count ->
    [one] { $value } 位关注者
   *[other] { $value } 位关注者
}
user-following = { $count ->
   *[other] 正在关注 { $value } 人
}
user-playlists = 公开播放列表
user-playlists-empty = 还没有公开播放列表

# home page
home-see-more = 查看更多
home-quick-picks = 快速推荐
home-quick-picks-empty = 收藏几首歌后，它们会显示在这里
home-recently-added = 最近添加
home-playlists = 播放列表
home-favorite-albums = 收藏的专辑
home-artists = 歌手
home-collection-albums = 你音乐库中的专辑

# search page
search-placeholder = 你想听什么？
search-browse = 浏览全部
search-submit = 搜索
search-recent = 最近搜索
search-forget = 从历史记录中移除
genre-load-more = 加载更多
genre-loading-more = 正在加载…
genre-load-failed = 加载失败，重试
genre-empty = 这里暂时没有可显示的内容
search-best-match = 最佳匹配
search-no-matches = 无匹配结果
search-results = 结果
search-songs = 歌曲
search-artists = 歌手
search-albums-playlists = 专辑和播放列表
search-tag = { $kind } ·
search-saved =
    { $count ->
        [one] 音乐库中有 { $count } 首歌曲
       *[other] 音乐库中有 { $count } 首歌曲
    }
# song page
song-eyebrow = 歌曲
song-view-album = 查看专辑
song-about = 关于这首歌
song-album = 专辑
song-released = 发行日期
song-streams = 播放次数
song-position = 位置
song-label = 唱片公司
song-popularity = 热度
song-popularity-value = { $value }%
song-disc-track = 第 { $disc } 张碟，第 { $track } 首
song-track = 第 { $track } 首
song-credits = 制作人员
song-performed-by = 表演者
song-details = 流派和详情
song-genres = 流派
song-language = 语言
song-content = 内容
song-explicit = 露骨内容
song-paid = 付费
song-clean = 干净版
artist-about = 关于歌手
artist-about-fallback = 探索这位歌手的热门歌曲和发行作品。
artist-about-open = 前往歌手
song-copyright = © { $notice }

theme-unavailable = { $name }（不可用）

saver-fps = { $fps } FPS

radio-tagline = 实时推送契合口味的音乐，随反馈动态更新
home-fm-now = 正在漫游：{ $name }
radio-play = 播放
radio-refresh = 换一批
radio-dislike = 不喜欢
radio-empty = 还没有挑到歌，登录后再试试
radio-loading = 正在为你挑选…
daily-empty = 今天还没有推荐，登录后每天更新
daily-loading = 正在准备今天的推荐…
daily-refresh = 刷新
# tags
tags-edit-title = 编辑标签
tags-sheet-song = 歌曲
tags-sheet-album = 专辑
tags-sheet-details = 详情
tags-title = 标题
tags-artist = 歌手
tags-artists = 歌手
tags-add-artist = 添加歌手
tags-remove-artist = 移除此歌手
tags-track = 歌曲编号
tags-track-total = 发行歌曲总数
tags-disc = 碟片编号
tags-disc-total = 发行碟片总数
tags-album = 专辑
tags-album-artist = 专辑歌手
tags-year = 年份
tags-genre = 流派
tags-composer = 作曲家
tags-publisher = 发行商
tags-isrc = ISRC
tags-comment = 备注
# [wisp] 网易云：验证码登录
login-sign-in-phone = 验证码登录
login-phone-title = 验证码登录
login-phone-number = 手机号
login-phone-code = 短信验证码
login-phone-send = 发送验证码
login-phone-retry = { $seconds } 秒后重试
login-phone-invalid = 手机号格式不正确
login-phone-code-invalid = 请输入 4-6 位验证码
login-phone-request-code = 请先获取验证码
login-phone-submit = 登录
# [wisp] 酷狗：账号密码与第三方扫码登录
login-sign-in-password = 账号登录
login-sign-in-qq = QQ 扫码
login-sign-in-wechat = 微信扫码
login-password-title = 账号登录
login-password-username = 用户名
login-password-password = 密码
login-password-submit = 登录
login-password-username-empty = 请输入用户名
login-password-password-empty = 请输入密码
# [wisp] 酷狗登录页
login-provider-title = 登录{ $provider }
login-provider-subtitle = 选择一种方式登录{ $provider }账号
login-provider-connecting = 正在连接{ $provider }…
login-provider-qr-scan = 打开 { $provider } App 扫一扫
login-kugou-title = 登录酷狗音乐
login-kugou-subtitle = 选择一种方式登录酷狗账号
login-kugou-method-kugou = 酷狗
login-kugou-method-phone = 验证码
login-kugou-method-qq = QQ
login-kugou-method-wechat = 微信
login-kugou-connecting = 正在连接酷狗…
login-kugou-retry = 重试
login-kugou-refresh = 刷新二维码
login-kugou-back = 返回
login-kugou-qr-kugou = 打开酷狗音乐 App 扫一扫
login-kugou-qr-qq = 打开 QQ 扫一扫
login-kugou-qr-wechat = 打开微信扫一扫
login-kugou-qr-confirm = 已扫码，请在手机上确认
login-kugou-phone-hint = 输入手机号获取验证码
login-phone-sent = 验证码已发送

# [wisp] 补齐 Wisp zh-CN 缺失键（用户日志实测缺失）
login-welcome-title = 欢迎使用 WispMusic
login-welcome-detail = 登录以同步你的音乐
login-providers = 选择服务
track-row-name = 歌曲
home-daily = 每日推荐
home-daily-detail = 为你量身定制
home-radio = 私人漫游
home-radio-detail = 由此开启好心情~
smart-rules-name-placeholder = 歌单名称
playlist-create-local = 本地文件
login-problem-password = 服务器没有接受这组用户名和密码，请检查后重试。
login-profile-title = 选择配置文件
login-profile-detail = 该账户有多个配置文件，请选择 Wisp 要使用的那一个。
login-or = 或
artist-followers = { $count } 位粉丝
artist-favorite-songs = 你喜欢的歌
powerbar-title = 快速搜索
powerbar-placeholder = 想听点什么？
powerbar-open = 打开
powerbar-song = 前往歌曲
song-local = 本地文件
login-verify-title = 安全验证
login-verify-code = 验证码
login-verify-submit = 验证
login-verify-detail = 酷狗要求安全验证：{ $hint }
login-verify-detail-plain = 酷狗要求完成 { $kind } 验证，请验证后输入验证码。
radio-card-title = 为你推荐
radio-now-playing = 当前播放
radio-now-empty = 暂时没有可展示的推荐内容。
radio-mode-normal = 红心
radio-mode-small = 小众
radio-mode-peak = 速览
radio-mode-radio = 电台
radio-mode-title-normal = 红心 Radio
radio-mode-title-small = 小众 Radio
radio-mode-title-peak = 速览 Radio
radio-mode-title-radio = 电台 Radio
radio-pool-taste = 口味
radio-pool-style = 风格
radio-pool-explore = 探索
