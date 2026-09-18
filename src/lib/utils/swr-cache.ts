import type { Cache, State } from "swr";
import { createLruCache } from "./lru";

/**
 * SWR 的 cache provider：把 SWR 的**无上限**默认缓存换成带上限的 LRU。
 *
 * SWR 默认用 `initCache(new Map())`（node_modules/swr/dist/_internal/
 * config-context-*.mjs），也就是一个没有淘汰的全局 Map；而本项目多处 key 会装下
 * 大对象——`playlist-tracks-<id>` 是整张歌单的完整 Song[]、`user-cloud` 是整个
 * 云盘、探索页/歌手/新碟的无限分页每个 (分类, 页码) 都是一个独立 key。访问过的
 * 页面数据会在 webview 存活期内一直留着，是主窗口内存里最容易被忽略的一块
 * （点过十几个歌单就是几十 MB）。
 *
 * 换成 LRU 后超限时淘汰最久未访问的 key；被淘汰的 key 再次访问时由 SWR 按需
 * 重新请求，行为等同于「没有本地缓存」，不会出错。
 *
 * 上限取 64：同时「热」的 key 通常是十几个（当前页 + 首页区块 + 侧边栏），
 * 64 足够覆盖来回切换，又不至于让几张大 key 长期压着内存。
 */
export const SWR_CACHE_LIMIT = 64;

export function createSwrLruCache(
	limit: number = SWR_CACHE_LIMIT,
): Cache<unknown> {
	const store = createLruCache<string, State<unknown>>(limit);

	return {
		// 数组本身可迭代；SWR 只做一次遍历，无需返回 Map 的活迭代器
		keys: () => store.keys()[Symbol.iterator](),
		get: (key) => store.get(key),
		set: (key, value) => {
			store.set(key, value);
		},
		delete: (key) => {
			store.delete(key);
		},
	};
}
