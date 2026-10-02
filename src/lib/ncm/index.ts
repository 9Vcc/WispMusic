// 内置网易云 API 的统一入口：供 src/lib/api.ts 的 embedded 分支调用
import { callEmbeddedApi } from "ncm-embedded/browser";
import { ensureAnonymousToken } from "./anonymous";
import { embeddedExternals } from "./externals";
import { createRequest, type RequestAnswer } from "./request";

function isFormData(body: unknown): body is FormData {
	return typeof FormData !== "undefined" && body instanceof FormData;
}

/** 与 fetch 的取消语义保持一致：调用方靠 `err.name === "AbortError"` 区分「被取代」 */
function abortError(): DOMException {
	return new DOMException("The operation was aborted.", "AbortError");
}

function untilAborted(signal: AbortSignal): Promise<never> {
	return new Promise<never>((_, reject) => {
		// 建监听之前就可能已经 abort（本函数与发起请求之间还有一次 await），
		// abort 事件不会重放，这里必须补看一次当前状态
		if (signal.aborted) {
			reject(abortError());
			return;
		}
		signal.addEventListener("abort", () => reject(abortError()), { once: true });
	});
}

export async function embeddedApiRequest<T>(
	path: string,
	params: Record<string, string>,
	body?: BodyInit | null,
	signal?: AbortSignal,
): Promise<T> {
	const mergedParams: Record<string, any> = { ...params };

	if (isFormData(body)) {
		for (const [key, value] of body.entries()) {
			if (typeof value === "string") {
				mergedParams[key] = value;
			} else {
				// File/Blob 字段（imgFile）
				mergedParams[key] = value;
			}
		}
	} else if (typeof body === "string" && body) {
		try {
			const parsed = JSON.parse(body) as Record<string, unknown>;
			if (parsed && typeof parsed === "object") {
				Object.assign(mergedParams, parsed);
			}
		} catch {
			// 非 JSON body 忽略（业务层均为 JSON 或 FormData）
		}
	}

	if (signal?.aborted) throw abortError();

	await ensureAnonymousToken();

	const task = callEmbeddedApi(
		path,
		mergedParams,
		createRequest,
		embeddedExternals,
	).then((result) => result.body as T);

	try {
		// 内置 API 走 Tauri invoke，底层请求取消不了；但**等待方**必须能脱身，否则被
		// 取代的旧调用会把 playSong 的 await 一直吊着（快速切歌转圈的一环）。底层照常
		// 跑完，结果由各层缓存接住（见 unblock.ts 的 record）——只是没人等了。
		return signal ? await Promise.race([task, untilAborted(signal)]) : await task;
	} catch (error) {
		// 取消不是失败：原样抛给调用方，让 playSong 的 AbortError 分支继续生效
		if ((error as Error)?.name === "AbortError") throw error;
		const answer = error as Partial<RequestAnswer>;
		// 复刻 api.ts http() 的 301 处理：登录态失效时清理并跳转登录页
		if (answer?.status === 301) {
			localStorage.removeItem("cookie");
			localStorage.removeItem("userInfo");
			if (window.location.pathname !== "/login") {
				window.location.href = "/login";
			}
		}
		const body = (answer?.body ?? {}) as Record<string, unknown>;
		const message =
			(body.message as string) ||
			(body.msg as string) ||
			`Request failed: ${answer?.status ?? "unknown"}`;
		throw new Error(message);
	}
}
