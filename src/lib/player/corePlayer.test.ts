import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
// listen 返回一个永不 resolve 的 promise：事件订阅与本用例无关
vi.mock("@tauri-apps/api/event", () => ({
	listen: vi.fn(() => new Promise(() => {})),
}));

import { invoke } from "@tauri-apps/api/core";
import { corePlayer } from "./corePlayer";

interface FakeSnapshot {
	requestId: number;
	isReady: boolean;
	isPlaying: boolean;
	currentTime: number;
	duration: number;
	volume: number;
	speed: number;
	source: string | null;
	audioLevel: number;
	audioSpectrum: number[];
	replayGainDb: number | null;
	replayGainApplied: boolean;
	replayGainPreampDb: number;
	equalizerEnabled: boolean;
	equalizerGainsDb: number[];
	crossfadeDuration: number;
}

/**
 * isPlaying 恒为 false：node 环境没有 requestAnimationFrame，
 * 而起播回调只看「载入有没有结论」，与原生是否已在出声无关。
 */
const snapshot = (requestId: number): FakeSnapshot => ({
	requestId,
	isReady: true,
	isPlaying: false,
	currentTime: 0,
	duration: 180,
	volume: 0.7,
	speed: 1,
	source: "test",
	audioLevel: 0,
	audioSpectrum: [],
	replayGainDb: null,
	replayGainApplied: false,
	replayGainPreampDb: 0,
	equalizerEnabled: false,
	equalizerGainsDb: [],
	crossfadeDuration: 0,
});

const invokeMock = vi.mocked(invoke);

/** 只看最近一次 player_load：corePlayer 是模块级单例，调用会累积 */
const lastLoadArgs = () =>
	[...invokeMock.mock.calls]
		.reverse()
		.find(([command]) => command === "player_load")?.[1] as
		| { requestId: number }
		| undefined;

/**
 * 注意用例顺序：corePlayer 是单例，playRequestId 单调递增、代次基线只播种一次。
 * 第一条必须排在最前——它观察的是「尚未播种、从 1 开始计数」的原始状态。
 */
describe("corePlayer 载入代次", () => {
	beforeEach(() => {
		vi.clearAllMocks();
		vi.spyOn(console, "error").mockImplementation(() => {});
		vi.spyOn(console, "warn").mockImplementation(() => {});
	});

	it("不取基线时复现故障：原生把首次载入判成迟到请求，转圈永不落幕", async () => {
		invokeMock.mockImplementation(async (command: string) => {
			if (command === "player_latest_request_id") return 12;
			// 原生对迟到的旧请求回的是「当前状态」，携带的是别的代次
			return snapshot(12);
		});

		const onPlay = vi.fn();
		const onError = vi.fn();
		// 少了 ensureRequestIdBaseline 这一步 —— 这正是修复前的形态
		corePlayer.play("http://example/a.mp3", () => {}, onPlay, undefined, onError);

		await vi.waitFor(() => expect(onError).toHaveBeenCalled());
		expect(onPlay).not.toHaveBeenCalled();
		// 本地计数从 1 开始，永远低于原生代次 12：要连点十几首才自己恢复
		expect(lastLoadArgs()?.requestId).toBeLessThan(12);
	});

	it("基线取不到时退回本地计数，不阻塞播放", async () => {
		invokeMock.mockRejectedValueOnce(new Error("ipc down"));
		await expect(corePlayer.ensureRequestIdBaseline()).resolves.toBeUndefined();
		expect(invokeMock).toHaveBeenCalledWith("player_latest_request_id");

		invokeMock.mockImplementation(async (_command: string, args?: unknown) =>
			snapshot((args as { requestId: number }).requestId),
		);
		const onPlay = vi.fn();
		corePlayer.play("http://example/b.mp3", () => {}, onPlay);

		await vi.waitFor(() => expect(onPlay).toHaveBeenCalled());
		expect(lastLoadArgs()).toBeDefined();
	});

	it("以原生代次为基线，重载后的首次载入不再被当成迟到请求", async () => {
		invokeMock.mockImplementation(async (command: string, args?: unknown) => {
			if (command === "player_latest_request_id") return 12;
			return snapshot((args as { requestId: number }).requestId);
		});

		await corePlayer.ensureRequestIdBaseline();
		const onPlay = vi.fn();
		corePlayer.play("http://example/c.mp3", () => {}, onPlay);

		await vi.waitFor(() => expect(onPlay).toHaveBeenCalled());
		// 原生代次只增不减：落后于它的 requestId 会被判成迟到命令直接丢弃，
		// 页面重载后本地从 1 重新数就会永久卡在那个状态（没声音、转圈不落幕）
		expect(lastLoadArgs()?.requestId).toBeGreaterThan(12);
	});

	it("原生快照代次与本次不符时按失败落幕，而不是留下永不消失的转圈", async () => {
		invokeMock.mockImplementation(async (command: string) => {
			if (command === "player_latest_request_id") return 0;
			// 本次载入确实已发出，但原生回的是别的代次——此后不会再来任何结论
			return snapshot(999_999);
		});

		const onPlay = vi.fn();
		const onError = vi.fn();
		corePlayer.play("http://example/d.mp3", () => {}, onPlay, undefined, onError);

		await vi.waitFor(() => expect(onError).toHaveBeenCalled());
		expect(onPlay).not.toHaveBeenCalled();
		// 在途标记一并丢弃：否则 togglePlay 认为加载仍在路上，用户点了也不会重试
		expect(corePlayer.isLoading()).toBe(false);
	});
});
