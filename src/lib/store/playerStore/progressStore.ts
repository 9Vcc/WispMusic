import { create } from "zustand";

/**
 * 高频播放进度独立 store。
 *
 * 播放进度每 200ms 更新一次，与主 playerStore 分离可以避免这些高频 set
 * 穿过 persist 中间件（每次 set 都会触发全量 partialize + 序列化），也让
 * 订阅进度的组件不必连带主 store 的其他变更重渲染。
 */
interface ProgressState {
	/** 当前播放时长 s */
	currentTime: number;
	/** 播放进度 0 ~ 100 */
	progress: number;
	setProgress: (currentTime: number, progress: number) => void;
	resetProgress: () => void;
}

export const useProgressStore = create<ProgressState>()((set) => ({
	currentTime: 0,
	progress: 0,
	setProgress: (currentTime, progress) => set({ currentTime, progress }),
	resetProgress: () => set({ currentTime: 0, progress: 0 }),
}));
