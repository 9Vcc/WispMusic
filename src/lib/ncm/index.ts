// 内置网易云 API 的统一入口：供 src/lib/api.ts 的 embedded 分支调用
import { callEmbeddedApi } from "ncm-embedded/browser";
import { ensureAnonymousToken } from "./anonymous";
import { embeddedExternals } from "./externals";
import { createRequest, type RequestAnswer } from "./request";

function isFormData(body: unknown): body is FormData {
	return typeof FormData !== "undefined" && body instanceof FormData;
}

export async function embeddedApiRequest<T>(
	path: string,
	params: Record<string, string>,
	body?: BodyInit | null,
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

	await ensureAnonymousToken();

	try {
		const result = await callEmbeddedApi(
			path,
			mergedParams,
			createRequest,
			embeddedExternals,
		);
		return result.body as T;
	} catch (error) {
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
