import { get, set, del } from "idb-keyval";
import type { PersistStorage, StateStorage } from "zustand/middleware";
import type { SharedPlayerState } from "../types/player";

const lastSavedString: Record<string, string> = {};

export const idbStorage: StateStorage = {
	getItem: async (name: string): Promise<string | null> => {
		return (await get(name)) ?? null;
	},
	setItem: async (name: string, value: string): Promise<void> => {
		if (lastSavedString[name] === value) {
			return;
		}
		lastSavedString[name] = value;
		await set(name, value);
	},
	removeItem: async (name: string): Promise<void> => {
		await del(name);
	},
};

// localStorage 是同步写，高频 set 会阻塞主线程；内容未变时跳过写入
const lastSavedLocal: Record<string, string> = {};

export const localStorageWithDirtyCheck: StateStorage = {
	getItem: (name: string): string | null => {
		return localStorage.getItem(name);
	},
	setItem: (name: string, value: string): void => {
		if (lastSavedLocal[name] === value) {
			return;
		}
		lastSavedLocal[name] = value;
		localStorage.setItem(name, value);
	},
	removeItem: (name: string): void => {
		localStorage.removeItem(name);
	},
};

/**
 * playerStore 的 IndexedDB 层专用存储：直接存对象，不走 createJSONStorage。
 *
 * zustand persist 的 setItem 在**每次 set 都会执行且不做 diff**，若包一层
 * createJSONStorage，则任何一次 set（isPlaying / duration / 歌词刷新）都会
 * JSON.stringify 整条 playlist——上千首时是持续的多 MB 字符串分配与 GC 抖动，
 * 而 idbStorage 的字符串脏检查发生在 stringify 之后，省不掉这部分开销。
 *
 * 这里绕过 JSON：写入前用 playlist 的**引用**做 O(1) 脏检查（播放列表没换
 * 引用就整体跳过），真正落盘时交给 idb-keyval 的结构化克隆。
 */
type PlayerPersistedState = { playlist: SharedPlayerState["playlist"] };

// 结构化克隆无法复用字符串，用引用比较代替内容比较；播放列表每次变更都是新数组
let lastPersistedPlaylist: unknown = null;

export const playerPlaylistStorage: PersistStorage<PlayerPersistedState> = {
	getItem: async (name: string) => {
		const raw = await get(name);
		if (raw === null || raw === undefined) {
			return null;
		}
		// 旧版本经 createJSONStorage 写入的是 JSON 字符串：解析后回写成对象形态，
		// 完成一次性迁移，避免历史用户升级后播放列表被判为无效而清空
		if (typeof raw === "string") {
			try {
				const parsed = JSON.parse(raw) as {
					state: PlayerPersistedState;
					version?: number;
				};
				lastPersistedPlaylist = parsed?.state?.playlist ?? null;
				await set(name, parsed);
				return parsed;
			} catch {
				return null;
			}
		}
		const persisted = raw as { state: PlayerPersistedState; version?: number };
		lastPersistedPlaylist = persisted?.state?.playlist ?? null;
		return persisted;
	},
	setItem: async (name: string, value) => {
		if (value?.state?.playlist === lastPersistedPlaylist) {
			return;
		}
		lastPersistedPlaylist = value?.state?.playlist ?? null;
		await set(name, value);
	},
	removeItem: async (name: string) => {
		lastPersistedPlaylist = null;
		await del(name);
	},
};

