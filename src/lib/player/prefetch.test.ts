import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/** 预取的两个外部出口：地址解析与原生预热，都换成替身 */
const { resolvePlayableSource, preload } = vi.hoisted(() => ({
	resolvePlayableSource: vi.fn(),
	preload: vi.fn(),
}));

vi.mock("@/lib/player/song-source", () => ({ resolvePlayableSource }));
vi.mock("@/lib/player/corePlayer", () => ({ corePlayer: { preload } }));

import {
	inSwitchBurst,
	noteManualSongSwitch,
	prefetchSong,
} from "./prefetch";

const song = (id: number) => ({ id, name: `s${id}` }) as never;

describe("连续切歌期间的预取节流", () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.clearAllMocks();
		resolvePlayableSource.mockResolvedValue({ url: "http://x/a.mp3", levelKey: "h" });
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	it("点歌后立刻进入 burst 窗口，窗口内不预取", async () => {
		noteManualSongSwitch();
		expect(inSwitchBurst()).toBe(true);

		expect(prefetchSong(song(1001))).toBe(false);
		await vi.advanceTimersByTimeAsync(10);
		expect(resolvePlayableSource).not.toHaveBeenCalled();

		// 窗口过后同一个对象又能预取（说明没有被 attempted 误伤）
		await vi.advanceTimersByTimeAsync(1300);
		expect(inSwitchBurst()).toBe(false);
		expect(prefetchSong(song(1001))).toBe(true);
		await vi.advanceTimersByTimeAsync(10);
		expect(resolvePlayableSource).toHaveBeenCalledTimes(1);
	});

	it("紧急预取（自然放完接下一首）不受 burst 影响", async () => {
		noteManualSongSwitch();

		expect(prefetchSong(song(1002), true)).toBe(true);
		await vi.advanceTimersByTimeAsync(10);
		expect(resolvePlayableSource).toHaveBeenCalledTimes(1);
		expect(preload).toHaveBeenCalledTimes(1);
	});

	it("同一首只发起一次，重复点不会反复重下", async () => {
		expect(prefetchSong(song(1003), true)).toBe(true);
		expect(prefetchSong(song(1003), true)).toBe(false);
		await vi.advanceTimersByTimeAsync(10);
		expect(resolvePlayableSource).toHaveBeenCalledTimes(1);
	});

	it("本地歌曲不做任何预取", () => {
		expect(prefetchSong({ id: 1004, localFilePath: "D:/a.mp3" } as never, true)).toBe(false);
		expect(resolvePlayableSource).not.toHaveBeenCalled();
	});
});
