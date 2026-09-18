import { beforeEach, describe, expect, it, vi } from "vitest";

// vi.mock 会被提升到 import 之前，缓存与计数器必须用 vi.hoisted 提前建立，
// 否则 mock 工厂首次执行时还处在 TDZ
const { idbStore, writeCount } = vi.hoisted(() => ({
	idbStore: new Map<string, unknown>(),
	writeCount: { value: 0 },
}));

vi.mock("idb-keyval", () => ({
	get: async (key: string) => idbStore.get(key),
	set: async (key: string, value: unknown) => {
		writeCount.value += 1;
		// 真实 idb-keyval 走结构化克隆，这里用 structuredClone 保持语义一致
		idbStore.set(key, structuredClone(value));
	},
	del: async (key: string) => {
		idbStore.delete(key);
	},
}));

import { playerPlaylistStorage } from "./idbStorage";

type PersistedPlaylist = Parameters<typeof playerPlaylistStorage.setItem>[1];

/** 只验证缓存行为，用最小可辨识对象代替完整 Song */
const payload = (...ids: number[]) =>
	({
		state: { playlist: ids.map((id) => ({ id, name: `song-${id}` })) },
		version: 0,
	}) as unknown as PersistedPlaylist;

const NAME = "player-store-idb";

describe("playerPlaylistStorage", () => {
	beforeEach(() => {
		idbStore.clear();
		writeCount.value = 0;
	});

	it("写入的是对象而非 JSON 字符串，读回结构不变", async () => {
		await playerPlaylistStorage.setItem(NAME, payload(1, 2));
		expect(typeof idbStore.get(NAME)).toBe("object");

		const read = await playerPlaylistStorage.getItem(NAME);
		expect(read?.state.playlist.map((song) => song.id)).toEqual([1, 2]);
	});

	it("playlist 引用未变时整体跳过写入", async () => {
		const value = payload(1, 2);
		await playerPlaylistStorage.setItem(NAME, value);
		expect(writeCount.value).toBe(1);

		// 同一次 partialize 结果的引用重复提交（persist 每次 set 都会调用）
		await playerPlaylistStorage.setItem(NAME, value);
		await playerPlaylistStorage.setItem(NAME, value);
		expect(writeCount.value).toBe(1);
	});

	it("playlist 换引用后照常写入", async () => {
		await playerPlaylistStorage.setItem(NAME, payload(1, 2));
		await playerPlaylistStorage.setItem(NAME, payload(1, 2, 3));

		expect(writeCount.value).toBe(2);
		const read = await playerPlaylistStorage.getItem(NAME);
		expect(read?.state.playlist).toHaveLength(3);
	});

	it("旧的 JSON 字符串会被解析并迁移成对象形态", async () => {
		const legacy = JSON.stringify({
			state: { playlist: [{ id: 7, name: "song-7" }] },
			version: 0,
		});
		idbStore.set(NAME, legacy);
		writeCount.value = 0;

		const read = await playerPlaylistStorage.getItem(NAME);
		expect(read?.state.playlist.map((song) => song.id)).toEqual([7]);
		// 迁移结果已回写为对象，下次启动不再重复解析
		expect(typeof idbStore.get(NAME)).toBe("object");
		expect(writeCount.value).toBe(1);
	});

	it("已迁移的数据再次读取不会重复写入", async () => {
		await playerPlaylistStorage.setItem(NAME, payload(1));
		writeCount.value = 0;

		await playerPlaylistStorage.getItem(NAME);
		await playerPlaylistStorage.getItem(NAME);
		expect(writeCount.value).toBe(0);
	});

	it("损坏的 JSON 字符串按无数据返回，不抛错", async () => {
		idbStore.set(NAME, "{ not json");
		await expect(playerPlaylistStorage.getItem(NAME)).resolves.toBeNull();
	});

	it("removeItem 后读回为空", async () => {
		await playerPlaylistStorage.setItem(NAME, payload(1));
		await playerPlaylistStorage.removeItem(NAME);

		expect(await playerPlaylistStorage.getItem(NAME)).toBeNull();
		// 删除后脏检查被重置：同样的数据可以再次写入
		writeCount.value = 0;
		await playerPlaylistStorage.setItem(NAME, payload(1));
		expect(writeCount.value).toBe(1);
	});
});
