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

/**
 * 缓存总字节预算（估算口径）。条数上限对「单条可以很大」的 key 不够用：
 * `playlist-tracks-<id>` 一张千首歌单就是 1MB 量级，深挖几十张大歌单能压到
 * 数十 MB。24MB 比正常浏览的峰值（当前页 + 首页区块 + 侧边栏十几条）宽松得多，
 * 只在真的堆起多张大歌单时才开始淘汰最旧的那张。
 */
export const SWR_CACHE_MAX_BYTES = 24 * 1024 * 1024;

const ESTIMATE_SCALAR_BYTES = 8;
const ESTIMATE_CONTAINER_BYTES = 24;
/** 单条估算的节点上限：大歌单不整棵走完，估算只用于淘汰，偏差几成不影响结论 */
const ESTIMATE_NODE_BUDGET = 20000;
const ESTIMATE_MAX_DEPTH = 6;

/**
 * 估算一条缓存值占的字节数（估算口径，供 [[createLruCache]] 的字节预算使用）：
 * 字符串按 UTF-16 长度、标量按 8 字节、对象/数组加固定开销，节点数与深度有上限。
 */
export function estimateCacheValueBytes(value: unknown): number {
	return estimateNode(value, ESTIMATE_MAX_DEPTH, {
		nodes: ESTIMATE_NODE_BUDGET,
	});
}

function estimateNode(
	value: unknown,
	depth: number,
	budget: { nodes: number },
): number {
	if (budget.nodes <= 0) return 0;
	budget.nodes -= 1;

	if (value === null || value === undefined) return ESTIMATE_SCALAR_BYTES;
	switch (typeof value) {
		case "string":
			return value.length * 2 + ESTIMATE_SCALAR_BYTES;
		case "number":
		case "boolean":
		case "bigint":
			return ESTIMATE_SCALAR_BYTES;
		case "function":
		case "symbol":
			return 0;
	}

	if (depth <= 0) return ESTIMATE_CONTAINER_BYTES;
	if (Array.isArray(value)) {
		let total = ESTIMATE_CONTAINER_BYTES;
		for (const item of value) {
			total += estimateNode(item, depth - 1, budget);
			if (budget.nodes <= 0) break;
		}
		return total;
	}

	let total = ESTIMATE_CONTAINER_BYTES;
	for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
		total += key.length * 2 + estimateNode(item, depth - 1, budget);
		if (budget.nodes <= 0) break;
	}
	return total;
}

/**
 * 最近一次创建的缓存实例（provider 在根部只调用一次）。
 * 仅供调试面板做归因——除了读键与取值不做别的事，也不常驻任何引用。
 */
let activeCache: ReturnType<typeof createLruCache<string, State<unknown>>> | null =
	null;

export function createSwrLruCache(
	limit: number = SWR_CACHE_LIMIT,
	maxBytes: number = SWR_CACHE_MAX_BYTES,
): Cache<unknown> {
	const store = createLruCache<string, State<unknown>>(limit, {
		estimateSize: (state) => estimateCacheValueBytes(state?.data),
		maxBytes,
	});
	activeCache = store;

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

/** 诊断用：当前缓存里的键与各自持有的数据（键按最近使用顺序排列） */
export function getSwrCacheSnapshot(): {
	key: string;
	data: unknown;
}[] {
	if (!activeCache) return [];

	return activeCache.keys().map((key) => ({
		key,
		data: activeCache?.get(key)?.data,
	}));
}
