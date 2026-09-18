import { describe, expect, it } from "vitest";
import { createSwrLruCache } from "./swr-cache";

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
});
