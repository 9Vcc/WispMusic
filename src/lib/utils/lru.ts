/**
 * 极简 LRU：基于 Map 的插入序实现「命中即刷新为最近使用、超限从最旧淘汰」。
 *
 * 用于「按 key 缓存、只增不减会单调增长」的场景（笔画数据、手动歌词记忆等）。
 * 与直接裸用 Map 的区别只有一条：容量有上限，最久未用的条目会被淘汰。
 *
 * 约定：未命中返回 undefined，因此不要把 undefined 当作合法值缓存。
 */
export interface LruCache<K, V> {
	readonly size: number;
	get(key: K): V | undefined;
	has(key: K): boolean;
	set(key: K, value: V): void;
	/** 返回是否真的删掉了（与 Map 语义一致，SWR 之类的调用方会用到返回值） */
	delete(key: K): boolean;
	keys(): K[];
}

export interface LruCacheOptions<V> {
	/** 估算单条值占的字节数。与 maxBytes 一起给出时，总量超预算也会从最旧淘汰 */
	estimateSize?: (value: V) => number;
	/** 总字节预算（estimateSize 的口径）。条数上限对「单条可以很大」的缓存不够用
	 *  （如整张歌单），字节预算才是真正贴近内存的那个上限 */
	maxBytes?: number;
}

export function createLruCache<K, V>(
	limit: number,
	options: LruCacheOptions<V> = {},
): LruCache<K, V> {
	const max = Math.max(1, Math.floor(limit));
	const { estimateSize, maxBytes } = options;
	const store = new Map<K, V>();
	const sizes = new Map<K, number>();
	let totalBytes = 0;

	const forget = (key: K) => {
		totalBytes -= sizes.get(key) ?? 0;
		sizes.delete(key);
		store.delete(key);
	};

	const evictWhileOver = () => {
		while (store.size > max) {
			const oldest = store.keys().next().value;
			if (oldest === undefined) break;
			forget(oldest);
		}
		// 字节预算下至少保留最新一条：单条本身就超预算时不能把它自己淘汰掉，
		// 否则每次写入都被立刻清空，读侧永远 miss 并反复重取
		while (maxBytes !== undefined && totalBytes > maxBytes && store.size > 1) {
			const oldest = store.keys().next().value;
			if (oldest === undefined) break;
			forget(oldest);
		}
	};

	return {
		get size() {
			return store.size;
		},
		get(key) {
			const value = store.get(key);
			if (value === undefined) return undefined;
			// 命中只调整顺序，不重新估算：约定值不可变（SWR 每次写入新 state
			// 对象），估算在 set 时算一次即可，避免大对象在每次读取时全量走一遍
			store.delete(key);
			store.set(key, value);
			return value;
		},
		has(key) {
			return store.has(key);
		},
		set(key, value) {
			forget(key);
			store.set(key, value);
			const size = estimateSize ? Math.max(0, estimateSize(value)) : 0;
			sizes.set(key, size);
			totalBytes += size;
			evictWhileOver();
		},
		delete(key) {
			const existed = store.has(key);
			forget(key);
			return existed;
		},
		keys() {
			return [...store.keys()];
		},
	};
}
