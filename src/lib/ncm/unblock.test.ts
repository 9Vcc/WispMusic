import {
	afterEach,
	beforeAll,
	beforeEach,
	describe,
	expect,
	it,
	vi,
} from "vitest";

/**
 * 解灰链路的预算测试。三个外部依赖都换成替身：
 * invoke = 本地 unm sidecar，fetchViaRust = 第三方 HTTP 音源，
 * settingStore 被 matchID 动态 import（缓存 key 依赖它）。
 */
const { invokeMock, fetchMock } = vi.hoisted(() => ({
	invokeMock: vi.fn(),
	fetchMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("./fetch", () => ({ fetchViaRust: fetchMock }));
vi.mock("../store/settingStore/settingStore", () => ({
	useSettingStore: {
		getState: () => ({
			network: {
				unblockEnableFlac: true,
				unblockOrderFirst: true,
				unblockQqCookie: "",
				unblockJooxCookie: "",
			},
			audio: { preferQuality: "sq" },
		}),
	},
}));

import { matchID } from "./unblock";

const TOTAL_MS = 25_000;
const UNM_MS = 15_000;
const SOURCE_MS = 8_000;

/** 永不返回的等待：模拟第三方接口连上了但一个字都不回 */
const hang = () => new Promise<never>(() => {});

const jsonBody = (payload: unknown) =>
	Promise.resolve({
		status: 200,
		headers: {},
		setCookies: [],
		body: JSON.stringify(payload),
		bodyIsBase64: false,
	});

/**
 * 边推进假时间边等结论。
 *
 * 直接 `advanceTimersByTimeAsync(25_000)` 再 await 是不行的：解灰链在真正排上
 * 自己的 deadline 之前还要先穿过动态 import 与几层 await，一次性推进的窗口早就
 * 过去了，此后假时间不再前进、那个 await 也就永远不落地（真·挂住测试进程）。
 */
async function resolveWithin<T>(
	promise: Promise<T>,
	maxFakeMs = 60_000,
	stepMs = 500,
): Promise<{ value: T; elapsedMs: number }> {
	const startedAt = Date.now();
	while (Date.now() - startedAt <= maxFakeMs) {
		const settled = await Promise.race([
			// 用被伪装的 Date.now() 记录耗时：假时钟才是这条链真正「等了多久」的口径
			promise.then((value) => ({ value, elapsedMs: Date.now() - startedAt })),
			vi.advanceTimersByTimeAsync(stepMs).then(() => null),
		]);
		if (settled) return settled;
	}
	throw new Error(`假时间 ${maxFakeMs}ms 内解灰链仍未给出结论（超时预算失效）`);
}

describe("matchID 超时预算", () => {
	beforeAll(async () => {
		// matchID 用动态 import 取 settingStore（为了绕开 store → api → ncm → unblock 的
		// 循环依赖）。首次加载要走真实 I/O，而 vitest 的假时钟不推进这条路——装好假时钟
		// 之后再首次 import 就会卡在预算开始之前。先预热一次，后续动态 import 命中模块
		// 缓存，只花微任务。
		await import("../store/settingStore/settingStore");
	});

	beforeEach(() => {
		vi.useFakeTimers();
		vi.clearAllMocks();
		vi.spyOn(console, "warn").mockImplementation(() => {});
	});

	afterEach(() => {
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	it("整条链都挂死时，到整链预算返回「无可用音源」而不是永远不返回", async () => {
		invokeMock.mockImplementation(hang);
		fetchMock.mockImplementation(hang);

		const { value, elapsedMs } = await resolveWithin(matchID("2650815712"));

		expect(value.code).toBe(500);
		expect(value.data).toBeNull();
		// 提前返回等于「地址解析还没跑完就当失败」，同样不可接受
		expect(elapsedMs).toBeGreaterThanOrEqual(TOTAL_MS);
	}, 30_000);

	it("单个源挂死不会拖垮整链：预算到就换下一个源", async () => {
		// unm 立刻失败 → 进入 HTTP 回退；第一个源 bugpk 永不返回
		invokeMock.mockRejectedValue(new Error("sidecar down"));
		fetchMock.mockImplementation((url: string) =>
			url.includes("bugpk")
				? hang()
				: jsonBody({ url: "http://cdn.example/byfuns.mp3" }),
		);

		const { value, elapsedMs } = await resolveWithin(matchID("1"));

		expect(value.data?.url).toBe("http://cdn.example/byfuns.mp3");
		expect(value.code).toBe(200);
		// 死源只花了它自己的预算，整链远没到点就出结果了
		expect(elapsedMs).toBeLessThan(TOTAL_MS);
		expect(elapsedMs).toBeGreaterThanOrEqual(SOURCE_MS);
	}, 30_000);

	it("预算只是让等待方脱身：迟到的成功照样写进缓存，再点同一首不再重解", async () => {
		let lateResolve: (value: { url: string; source: string }) => void = () => {};
		invokeMock.mockImplementation(
			() =>
				new Promise((resolve) => {
					lateResolve = resolve as typeof lateResolve;
				}),
		);
		fetchMock.mockImplementation(hang);

		const first = await resolveWithin(matchID("7"));
		expect(first.value.data).toBeNull();

		// 底层匹配一直在跑，此刻才解出来——结果必须仍然落进缓存
		lateResolve({ url: "http://cdn.example/late.mp3", source: "kuwo" });
		await vi.advanceTimersByTimeAsync(1);

		invokeMock.mockClear();
		fetchMock.mockClear();
		const second = await matchID("7");

		expect(second.data?.url).toBe("http://cdn.example/late.mp3");
		expect(invokeMock).not.toHaveBeenCalled();
		expect(fetchMock).not.toHaveBeenCalled();
	}, 30_000);

	it("unm 预算小于整链预算，解不出时留给回退源时间", async () => {
		invokeMock.mockImplementation(hang);
		fetchMock.mockImplementation((url: string) =>
			url.includes("bugpk")
				? jsonBody({ url: "http://cdn.example/bugpk.mp3" })
				: hang(),
		);

		const { value, elapsedMs } = await resolveWithin(matchID("9"));

		expect(value.data?.url).toBe("http://cdn.example/bugpk.mp3");
		// unm 只占用它自己那一段，剩下时间够问回退源
		expect(elapsedMs).toBeGreaterThanOrEqual(UNM_MS);
		expect(elapsedMs).toBeLessThan(TOTAL_MS);
	}, 30_000);

	it("同一首的并发匹配只发一次引擎请求", async () => {
		invokeMock.mockImplementation(hang);
		fetchMock.mockImplementation(hang);

		// 点歌与悬停预取常常同时要同一首
		const a = matchID("11");
		await vi.advanceTimersByTimeAsync(1_000);
		const b = matchID("11");

		const { value } = await resolveWithin(Promise.all([a, b]));

		expect(invokeMock).toHaveBeenCalledTimes(1);
		// 两个等待方各守各的预算，都按时拿到结论
		expect(value[0].data).toBeNull();
		expect(value[1].data).toBeNull();
	}, 30_000);

	it("并发等待方共享同一次成功", async () => {
		let lateResolve: (value: { url: string; source: string }) => void = () => {};
		invokeMock.mockImplementation(
			() =>
				new Promise((resolve) => {
					lateResolve = resolve as typeof lateResolve;
				}),
		);
		fetchMock.mockImplementation(hang);

		const a = matchID("12");
		await vi.advanceTimersByTimeAsync(1_000);
		const b = matchID("12");

		lateResolve({ url: "http://cdn.example/shared.mp3", source: "kugou" });
		const [ra, rb] = await Promise.all([a, b]);

		expect(ra.data?.url).toBe("http://cdn.example/shared.mp3");
		expect(rb.data?.url).toBe("http://cdn.example/shared.mp3");
		expect(invokeMock).toHaveBeenCalledTimes(1);
	}, 30_000);
});
