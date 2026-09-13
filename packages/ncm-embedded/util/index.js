// 基于 @neteasecloudmusicapienhanced/api@4.30.2 util/index.js 的浏览器移植版：
// 移除 fs/path/logger 与 china_ip_ranges.txt 加载，其余行为保持一致
const floor = Math.floor
const random = Math.random
const keys = Object.keys

// 预编译encodeURIComponent以减少查找开销
const encode = encodeURIComponent

module.exports = {
  toBoolean(val) {
    if (typeof val === 'boolean') return val
    if (val === '') return val
    return val === 'true' || val == '1'
  },

  cookieToJson(cookie) {
    if (!cookie) return {}
    let cookieArr = cookie.split(';')
    let obj = {}

    for (let i = 0, len = cookieArr.length; i < len; i++) {
      let item = cookieArr[i]
      let arr = item.split('=')
      if (arr.length === 2) {
        obj[arr[0].trim()] = arr[1].trim()
      }
    }
    return obj
  },

  cookieObjToString(cookie) {
    const cookieKeys = keys(cookie)
    const result = []

    for (let i = 0, len = cookieKeys.length; i < len; i++) {
      const key = cookieKeys[i]
      result[i] = `${encode(key)}=${encode(cookie[key])}`
    }

    return result.join('; ')
  },

  // 浏览器版无 china_ip_ranges.txt，固定走兜底逻辑（随机 116.x 前缀）
  generateRandomChineseIP() {
    const fallback = `116.${getRandomInt(25, 94)}.${generateIPSegment()}.${generateIPSegment()}`
    return fallback
  },

  // 生成chainId的函数
  generateChainId(cookie) {
    const version = 'v1'
    const randomNum = Math.floor(Math.random() * 1e6)
    const deviceId =
      getCookieValue(cookie, 'sDeviceId') || 'unknown-' + randomNum
    const platform = 'web'
    const action = 'login'
    const timestamp = Date.now()

    return `${version}_${deviceId}_${platform}_${action}_${timestamp}`
  },

  generateDeviceId() {
    const hexChars = '0123456789ABCDEF'
    const chars = []
    for (let i = 0; i < 52; i++) {
      const randomIndex = Math.floor(Math.random() * hexChars.length)
      chars.push(hexChars[randomIndex])
    }
    return chars.join('')
  },
}

function getRandomInt(min, max) {
  return floor(random() * (max - min + 1)) + min
}

function generateIPSegment() {
  return getRandomInt(1, 255)
}

// 用于从cookie字符串中获取指定值的辅助函数
function getCookieValue(cookieStr, name) {
  if (!cookieStr) return ''

  const cookies = '; ' + cookieStr
  const parts = cookies.split('; ' + name + '=')
  if (parts.length === 2) return parts.pop().split(';').shift()
  return ''
}
