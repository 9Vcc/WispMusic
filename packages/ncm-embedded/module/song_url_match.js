// 网易云歌曲解灰(适配SPlayer的UNM-Server)
// 支持qq音乐、酷狗音乐、酷我音乐、咪咕音乐、第三方网易云API等等(来自GD音乐台)

const createOption = require('../util/option.js')
const logger = require('../util/logger.js')

module.exports = async (query, request, externals) => {
  try {
    const result = await externals.unblockImpl.matchID(query.id, query.source)
    logger.info('开始解灰', query.id, result)
    // 所有音源均失败时 data 为 null，直接返回 code 500（不能返回空数组：前端会当 truthy 拼出无效 URL）
    if (!result.data || !result.data.url) {
      return {
        status: 200,
        body: {
          code: 500,
          msg: result.message || 'no available url',
          data: null,
        },
      }
    }
    const proxyUrl = result.data.url.includes('kuwo') ? result.data.url : ''
    return {
      status: 200,
      body: {
        code: 200,
        data: result.data.url,
        proxyUrl,
      },
    }
  } catch (e) {
    return {
      status: 200,
      body: {
        code: 500,
        msg: e.message || 'unblock error',
        data: null,
      },
    }
  }
}
