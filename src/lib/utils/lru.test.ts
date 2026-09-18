import { describe, expect, it } from "vitest";
import { createLruCache } from "./lru";

describe("createLruCache", () => {
	it("超过上限时淘汰最久未使用的条目", () => {
		const cache = createLruCache<string, number>(2);
		cache.set("a", 1);
		cache.set("b", 2);
		cache.set("c", 3);

		expect(cache.size).toBe(2);
		expect(cache.has("a")).toBe(false);
		expect(cache.get("b")).toBe(2);
		expect(cache.get("c")).toBe(3);
	});

	it("命中会刷新为最近使用，从而免于淘汰", () => {
		const cache = createLruCache<string, number>(2);
		cache.set("a", 1);
		cache.set("b", 2);
		expect(cache.get("a")).toBe(1);
		cache.set("c", 3);

		expect(cache.has("b")).toBe(false);
		expect(cache.get("a")).toBe(1);
		expect(cache.get("c")).toBe(3);
	});

	it("重复写入同一 key 不占用额外容量", () => {
		const cache = createLruCache<string, number>(2);
		cache.set("a", 1);
		cache.set("a", 2);

		expect(cache.size).toBe(1);
		expect(cache.get("a")).toBe(2);
	});

	it("delete / keys 与裸 Map 语义一致", () => {
		const cache = createLruCache<string, number>(3);
		cache.set("a", 1);
		cache.set("b", 2);
		cache.delete("a");

		expect(cache.keys()).toEqual(["b"]);
	});

	it("非法上限至少按 1 处理，不会退化成无上限", () => {
		const cache = createLruCache<string, number>(0);
		cache.set("a", 1);
		cache.set("b", 2);

		expect(cache.size).toBe(1);
		expect(cache.get("b")).toBe(2);
	});
});
