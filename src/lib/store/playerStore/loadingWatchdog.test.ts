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

const { abandonPendingLoad } = vi.hoisted(() => ({
	abandonPendingLoad: vi.fn(),
}));

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
	corePlayer: { abandonPendingLoad },
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
		subscribers.length = 0;
		storeState.isLoadingMusic = false;
		storeState.isPlaying = true;
	});

	afterEach(() => {
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	it("加载态滞留超过阈值时按无结论清除，并丢弃在途标记", () => {
		const dispose = initLoadingWatchdog();

		emitLoading(true);
		vi.advanceTimersByTime(STUCK_MS - 1);
		expect(storeState.isLoadingMusic).toBe(true);

		vi.advanceTimersByTime(2);
		expect(storeState.isLoadingMusic).toBe(false);
		expect(storeState.isPlaying).toBe(false);
		// 不清 corePlayer 的在途标记，togglePlay 会永久早退：转圈没了但点也没用
		expect(abandonPendingLoad).toHaveBeenCalledTimes(1);

		dispose();
	});

	it("在阈值内落幕则不做任何干预", () => {
		const dispose = initLoadingWatchdog();

		emitLoading(true);
		vi.advanceTimersByTime(STUCK_MS - 5_000);
		emitLoading(false);
		vi.advanceTimersByTime(STUCK_MS);

		expect(abandonPendingLoad).not.toHaveBeenCalled();
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
		expect(abandonPendingLoad).not.toHaveBeenCalled();
	});
});
