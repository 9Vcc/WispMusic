import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initLoadingWatchdog } from "./loadingWatchdog";

/**
 * 看门狗只依赖 store 的 isLoadingMusic 订阅与 corePlayer 的在途标记，
 * 这里把两者都换成替身：真实 store 会拉起两层 persist（IndexedDB + localStorage），
 * 与本题无关且在 node 环境下不可用。
 */
const { storeState, subscribers } = vi.hoisted(() => ({
	storeState: { isLoadingMusic: false, isPlaying: true },
	subscribers: [] as Array<{ listener: (isLoading: boolean) => void }>,
}));

const { nativeLoading } = vi.hoisted(() => ({ nativeLoading: { value: false } }));

vi.mock("./playerStore", () => ({
	usePlayerStore: {
		subscribe: (_selector: unknown, listener: (isLoading: boolean) => void) => {
			const entry = { listener };
			subscribers.push(entry);
			return () => {
				const index = subscribers.indexOf(entry);
				if (index !== -1) subscribers.splice(index, 1);
			};
		},
		getState: () => storeState,
		setState: (patch: Record<string, unknown>) => {
			Object.assign(storeState, patch);
		},
	},
}));

vi.mock("@/lib/player/corePlayer", () => ({
	corePlayer: { isLoading: () => nativeLoading.value },
}));

/** 模拟 zustand subscribeWithSelector：选中值变化时带新值回调监听器 */
const emitLoading = (isLoading: boolean) => {
	storeState.isLoadingMusic = isLoading;
	subscribers.forEach((entry) => entry.listener(isLoading));
};

const STUCK_MS = 25_000;

describe("initLoadingWatchdog", () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.clearAllMocks();
		vi.spyOn(console, "warn").mockImplementation(() => {});
		vi.spyOn(console, "debug").mockImplementation(() => {});
		subscribers.length = 0;
		storeState.isLoadingMusic = false;
		storeState.isPlaying = true;
		nativeLoading.value = false;
	});

	afterEach(() => {
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	it("无在途载入却仍显示加载时，超时按无结论清除", () => {
		const dispose = initLoadingWatchdog();

		emitLoading(true);
		vi.advanceTimersByTime(STUCK_MS - 1);
		expect(storeState.isLoadingMusic).toBe(true);

		vi.advanceTimersByTime(2);
		expect(storeState.isLoadingMusic).toBe(false);
		expect(storeState.isPlaying).toBe(false);

		dispose();
	});

	it("原生载入仍在途中时不误清，改为重新计时", () => {
		const dispose = initLoadingWatchdog();
		// 解灰音源要把整首下完才出声，几十秒是正常量级
		nativeLoading.value = true;

		emitLoading(true);
		vi.advanceTimersByTime(STUCK_MS * 3);
		expect(storeState.isLoadingMusic).toBe(true);

		// 原生侧一旦给出结论（这里体现为不再在途），下一轮计时就收尾
		nativeLoading.value = false;
		vi.advanceTimersByTime(STUCK_MS + 1);
		expect(storeState.isLoadingMusic).toBe(false);

		dispose();
	});

	it("在阈值内落幕则不做任何干预", () => {
		const dispose = initLoadingWatchdog();

		emitLoading(true);
		vi.advanceTimersByTime(STUCK_MS - 5_000);
		emitLoading(false);
		vi.advanceTimersByTime(STUCK_MS);

		expect(storeState.isPlaying).toBe(true);

		dispose();
	});

	it("中途又切歌时以最新一次加载重新计时", () => {
		const dispose = initLoadingWatchdog();

		emitLoading(true);
		vi.advanceTimersByTime(STUCK_MS - 1_000);
		// 用户又点了一首：先落幕再置 true，看门狗必须重新计时而不是立刻清除
		emitLoading(false);
		emitLoading(true);
		vi.advanceTimersByTime(1_000);
		expect(storeState.isLoadingMusic).toBe(true);

		vi.advanceTimersByTime(STUCK_MS);
		expect(storeState.isLoadingMusic).toBe(false);

		dispose();
	});

	it("注销后不再触发", () => {
		const dispose = initLoadingWatchdog();

		emitLoading(true);
		dispose();
		vi.advanceTimersByTime(STUCK_MS * 2);

		expect(storeState.isLoadingMusic).toBe(true);
	});
});
