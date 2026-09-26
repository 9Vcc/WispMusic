import { describe, expect, it } from "vitest";
import { createSwrLruCache, estimateCacheValueBytes } from "./swr-cache";

/** SWR 缓存里存的是它的 state 对象，这里用最小替身 */
const state = (data: unknown) => ({ data, isValidating: false }) as never;
const dataOf = (value: unknown) =>
	(value as { data?: unknown } | undefined)?.data;
/** 命中判定用 get 而非 has：SWR 的 Cache 接口只要求 get/set/delete/keys */
const hit = (cache: ReturnType<typeof createSwrLruCache>, key: string) =>
	cache.get(key) !== undefined;

describe("createSwrLruCache", () => {
	it("提供 SWR 需要的 Map 形态接口", () => {
		const cache = createSwrLruCache(4);
		cache.set("a", state(1));

		expect(hit(cache, "a")).toBe(true);
		expect(dataOf(cache.get("a"))).toBe(1);
		expect([...cache.keys()]).toEqual(["a"]);
		cache.delete("a");
		expect(cache.get("a")).toBeUndefined();
		expect([...cache.keys()]).toEqual([]);
	});

	it("超过上限时淘汰最久未访问的 key", () => {
		const cache = createSwrLruCache(2);
		cache.set("playlist-tracks-1", state("a"));
		cache.set("playlist-tracks-2", state("b"));
		cache.set("playlist-tracks-3", state("c"));

		expect(hit(cache, "playlist-tracks-1")).toBe(false);
		expect(dataOf(cache.get("playlist-tracks-2"))).toBe("b");
		expect(dataOf(cache.get("playlist-tracks-3"))).toBe("c");
	});

	it("读取会刷新热度，被访问过的 key 不会被后面的写入挤掉", () => {
		const cache = createSwrLruCache(2);
		cache.set("a", state(1));
		cache.set("b", state(2));
		// a 重新变为最近访问，随后写入 c 应淘汰 b
		expect(dataOf(cache.get("a"))).toBe(1);
		cache.set("c", state(3));

		expect(hit(cache, "b")).toBe(false);
		expect(dataOf(cache.get("a"))).toBe(1);
		expect(dataOf(cache.get("c"))).toBe(3);
	});

	it("keys() 快照可安全遍历（遍历中写入不会打乱当前迭代）", () => {
		const cache = createSwrLruCache(8);
		cache.set("a", state(1));
		cache.set("b", state(2));

		const seen: string[] = [];
		for (const key of cache.keys()) {
			seen.push(key);
			cache.set(`new-${key}`, state(0));
		}
		expect(seen).toEqual(["a", "b"]);
	});

	it("字节预算：堆起多张大歌单时淘汰最旧的，而不只靠条数上限", () => {
		const playlist = (n: number) =>
			Array.from({ length: 50 }, (_, i) => ({
				id: n * 100 + i,
				name: `song-${i}`,
				ar: "artist",
			}));
		// 预算按同一估算口径只装得下两条（state.data 才是被估算的部分）
		const perEntry = estimateCacheValueBytes(playlist(0));
		const cache = createSwrLruCache(64, perEntry * 2 + 64);

		for (let i = 0; i < 5; i += 1) {
			cache.set(`playlist-tracks-${i}`, state(playlist(i)));
		}

		expect(hit(cache, "playlist-tracks-0")).toBe(false);
		expect(hit(cache, "playlist-tracks-1")).toBe(false);
		expect(hit(cache, "playlist-tracks-4")).toBe(true);
		expect([...cache.keys()].length).toBeLessThanOrEqual(2);
	});

	it("估算随数据规模增长：同一结构的 500 首应远大于 1 首", () => {
		const song = (i: number) => ({ id: i, name: `song-${i}`, ar: "artist" });
		const one = estimateCacheValueBytes([song(0)]);
		const many = estimateCacheValueBytes(
			Array.from({ length: 500 }, (_, i) => song(i)),
		);

		expect(one).toBeGreaterThan(0);
		expect(many).toBeGreaterThan(one * 50);
	});
});
