# track menu
menu-new-playlist = 新建播放列表
menu-import-playlist = 导入播放列表…
menu-edit-tags = 编辑标签
menu-no-playlists = 没有播放列表
menu-search-playlists = 搜索播放列表
menu-add-to-library = 添加到收藏
menu-remove-from-library = 从收藏中移除
menu-remove-from-playlist = 从播放列表中移除
menu-remove-tracks-from-playlist = { $count ->
    [one] 从播放列表中移除 { $count } 首歌曲
   *[other] 从播放列表中移除 { $count } 首歌曲
}
menu-remove-from-history = 从历史记录中移除
menu-remove-tracks-from-history = { $count ->
    [one] 从历史记录中移除 { $count } 首歌曲
   *[other] 从历史记录中移除 { $count } 首歌曲
}
menu-delete-track-file = 删除歌曲文件
menu-delete-track-files = 删除 { $count } 个歌曲文件
menu-play-next = 下一首播放
menu-play-tracks-next = { $count ->
    [one] 下一首播放 { $count } 首歌曲
   *[other] 下一首播放 { $count } 首歌曲
}
menu-add-to-queue = 添加到播放队列
menu-add-tracks-to-queue = { $count ->
    [one] 将 { $count } 首歌曲添加到播放队列
   *[other] 将 { $count } 首歌曲添加到播放队列
}
menu-play-last = 最后播放
menu-play-tracks-last = { $count ->
    [one] 最后播放 { $count } 首歌曲
   *[other] 最后播放 { $count } 首歌曲
}
menu-song-radio = 前往歌曲电台
menu-start-radio = 开启电台
menu-go-to-album = 前往专辑
menu-go-to-artist = 前往歌手
menu-view-details = 查看详情
menu-copy-link = 复制链接
menu-cut = 剪切
menu-copy = 复制
menu-paste = 粘贴
menu-select-all = 全选
menu-remove-from-queue = 从播放队列中移除
menu-edit-playlist = 编辑播放列表
menu-delete-playlist = 删除播放列表
menu-export-playlist = 导出播放列表…
menu-add-playlist-to-library = 添加到音乐库
menu-remove-playlist-from-library = 从音乐库中移除
menu-make-playlist-public = 设为公开
menu-make-playlist-private = 设为私密
menu-play-artist = 播放歌手

# playlist editor
playlist-name-placeholder = 播放列表名称
playlist-create-title = 创建播放列表
playlist-edit-title = 编辑播放列表
playlist-cover-choose = 选择图片
playlist-cover-remove = 移除封面
playlist-cover-unopenable = Wisp 无法打开此文件
playlist-cover-too-large = 封面图片过大，请换一张更小的图片
playlist-cover-not-a-picture = 此文件不是 JPEG、PNG 或 WebP 图片
playlist-delete-title = 删除播放列表
playlist-again-title = 再次添加？
playlist-again-confirm = 这首歌曲已在“{ $name }”中。要再添加一份吗？
playlist-again-add = 再次添加

# confirm
confirm-delete-track-files-title = 删除歌曲文件？
confirm-delete-track-files = 文件将从磁盘上永久删除。此操作无法撤销。

menu-copy-link-none = 此项没有可分享的链接
menu-copy-link-local = 本地歌手和播放列表没有链接
cover-view = 查看封面
menu-add-to-playlist = 添加到歌单
menu-add-tracks-to-playlist = 把 { $count } 首歌加入歌单
menu-new-smart-playlist = 新建智能歌单
menu-smart-playlist-rules = 智能规则
smart-rules-create-title = 新建智能歌单
smart-rules-edit-title = 智能规则
smart-rules-loading = 正在从服务器读取规则…
smart-rules-saving = 正在保存…
smart-rules-failed-title = 规则读取失败
smart-rules-match-all = 全部匹配
smart-rules-match-any = 任一匹配
smart-rules-match-toggle = 切换：全部满足或任一满足
smart-rules-of-following = 以下条件
smart-rules-add-condition = 添加条件
smart-rules-add-group = 添加分组
smart-rules-remove-group = 删除该分组
smart-rules-remove-condition = 删除该条件
smart-rules-empty-group = 这里还没有规则
smart-rules-field = 字段
smart-rules-operator = 运算符
smart-rules-value-placeholder = 值
smart-rules-range-and = 与
smart-rules-true = 是
smart-rules-false = 否
smart-rules-unknown-note = 按服务器原样保留
smart-rules-synced-title = 此歌单从文件同步
smart-rules-synced-message = 下次扫描可能会覆盖在这里做的修改。
smart-rules-sort-label = 排序方式
smart-rules-sort-placeholder = title, dateloved, random…
smart-rules-order-none = 不指定方向
smart-rules-order-asc = 升序
smart-rules-order-desc = 降序
smart-rules-order-toggle = 排序方向
smart-rules-limit-label = 数量上限
smart-rules-limit-placeholder = 全部匹配
smart-rules-problem-empty-group = 该分组里没有规则
smart-rules-problem-missing-field = 条件未指定字段
smart-rules-problem-range = 范围需要上下两个边界
smart-rules-problem-days = 天数需要一个数字
smart-rules-problem-playlist = 歌单规则需要指定一个歌单
smart-rules-problem-truth = 该规则需要一个真值
smart-rules-problem-compared = 值与该规则不匹配
smart-rules-problem-boolean = 布尔字段需要真值
smart-rules-problem-bad-order = 排序方向只能是 asc 或 desc
menu-save-library = 资料库
menu-go-to-playlist = 前往歌单
playlist-create-shelf = 保存到
cover-zoom-in = 放大
cover-zoom-out = 缩小
cover-zoom-fit = 适应窗口
tray-quit = 退出 Wisp
tray-artist-none = 未播放
tray-mode-sequential = 顺序播放
tray-mode-list = 列表循环
tray-mode-random = 随机播放
tray-mode-single = 单曲循环
