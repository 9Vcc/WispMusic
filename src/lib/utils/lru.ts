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

export function createLruCache<K, V>(limit: number): LruCache<K, V> {
	const max = Math.max(1, Math.floor(limit));
	const store = new Map<K, V>();

	// 先删后插把 key 推到队尾，使其成为「最近使用」
	const touch = (key: K, value: V) => {
		store.delete(key);
		store.set(key, value);
	};

	return {
		get size() {
			return store.size;
		},
		get(key) {
			const value = store.get(key);
			if (value === undefined) return undefined;
			touch(key, value);
			return value;
		},
		has(key) {
			return store.has(key);
		},
		set(key, value) {
			touch(key, value);
			while (store.size > max) {
				const oldest = store.keys().next().value;
				if (oldest === undefined) break;
				store.delete(oldest);
			}
		},
		delete(key) {
			return store.delete(key);
		},
		keys() {
			return [...store.keys()];
		},
	};
}
