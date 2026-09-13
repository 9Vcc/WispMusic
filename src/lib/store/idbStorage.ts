import { get, set, del } from "idb-keyval";
import { StateStorage } from "zustand/middleware";

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
