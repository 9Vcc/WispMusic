import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * 只验 signal 这一段语义：内置 API 不经过 fetch，取消能生效全靠
 * embeddedApiRequest 自己把等待方摘出来，所以把依赖都换成替身。
 */
const { callEmbeddedApiMock, latestResolve } = vi.hoisted(() => {
	const slot: { resolve?: (value: unknown) => void } = {};
	return { callEmbeddedApiMock: vi.fn(), latestResolve: slot };
});

vi.mock("ncm-embedded/browser", () => ({ callEmbeddedApi: callEmbeddedApiMock }));
vi.mock("./anonymous", () => ({ ensureAnonymousToken: () => Promise.resolve() }));
vi.mock("./externals", () => ({ embeddedExternals: {} }));
vi.mock("./request", () => ({ createRequest: vi.fn() }));

import { embeddedApiRequest } from "./index";

/** 由测试自己了结的 promise：用来观察「调用方脱身之后底层还在跑」 */
const controllable = () =>
	new Promise((resolve) => {
		latestResolve.resolve = resolve;
	});

const unhandled: unknown[] = [];
const capture = (reason: unknown) => unhandled.push(reason);

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("embeddedApiRequest 的取消语义", () => {
	beforeEach(() => {
		vi.clearAllMocks();
		callEmbeddedApiMock.mockImplementation(controllable);
		unhandled.length = 0;
		process.on("unhandledRejection", capture);
	});

	afterEach(() => {
		process.off("unhandledRejection", capture);
	});

	it("信号已 abort 时不再发起请求", async () => {
		const controller = new AbortController();
		controller.abort();

		await expect(
			embeddedApiRequest("/song/url/v1", {}, null, controller.signal),
		).rejects.toMatchObject({ name: "AbortError" });
		expect(callEmbeddedApiMock).not.toHaveBeenCalled();
	});

	it("等待期间 abort 让调用方脱身，但底层不被打断", async () => {
		const controller = new AbortController();
		const waiting = embeddedApiRequest<{ code: number }>(
			"/song/url/match",
			{ id: "1" },
			null,
			controller.signal,
		);

		controller.abort();
		await expect(waiting).rejects.toMatchObject({ name: "AbortError" });

		// 解灰的迟到结果还要靠底层跑完才能落进缓存（见 unblock.ts 的 record）
		latestResolve.resolve?.({ body: { code: 200, data: "http://cdn/x.mp3" } });
		await flush();
		expect(callEmbeddedApiMock).toHaveBeenCalledTimes(1);
	});

	it("脱身后底层再失败不会留下未处理的 rejection", async () => {
		const controller = new AbortController();
		const waiting = embeddedApiRequest("/a", {}, null, controller.signal);
		controller.abort();
		await expect(waiting).rejects.toMatchObject({ name: "AbortError" });

		latestResolve.resolve?.({ body: null });
		await flush();
		expect(unhandled).toHaveLength(0);
	});

	it("不传 signal 时行为不变：正常取到 body", async () => {
		const waiting = embeddedApiRequest<{ code: number }>("/a", {}, null);
		// 请求是在 ensureAnonymousToken 之后才真正发出的，先让微任务队列跑完
		await flush();
		latestResolve.resolve?.({ body: { code: 200 } });
		await expect(waiting).resolves.toEqual({ code: 200 });
	});

	it("底层报错时仍按原样转成 Error（取消语义不影响失败路径）", async () => {
		callEmbeddedApiMock.mockRejectedValue({
			status: 500,
			body: { msg: "upstream failed" },
		});
		const controller = new AbortController();

		await expect(
			embeddedApiRequest("/a", {}, null, controller.signal),
		).rejects.toThrow("upstream failed");
	});
});
