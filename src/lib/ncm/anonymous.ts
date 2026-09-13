// 游客 token 管理：对应 Node 版 generateConfig.js 的启动时匿名注册逻辑
import { callEmbeddedApi } from "ncm-embedded/browser";
import { embeddedExternals } from "./externals";
import { createRequest } from "./request";
import { getAnonymousToken, persistDeviceId, setAnonymousToken } from "./state";

let pendingRegistration: Promise<void> | null = null;

function hasUserCookie(): boolean {
	try {
		return (localStorage.getItem("cookie") || "").includes("MUSIC_U");
	} catch {
		return false;
	}
}

// 并发去重；失败不阻塞（游客接口退化为无 MUSIC_A 请求，登录流程不依赖它）
export async function ensureAnonymousToken(): Promise<void> {
	if (hasUserCookie()) return;
	if (getAnonymousToken()) return;

	if (!pendingRegistration) {
		pendingRegistration = registerAnonymous()
			.catch((error) => {
				console.warn("[ncm] 匿名注册失败（游客接口可能受限）:", error);
			})
			.finally(() => {
				pendingRegistration = null;
			});
	}
	return pendingRegistration;
}

async function registerAnonymous(): Promise<void> {
	const result = await callEmbeddedApi(
		"/register/anonimous",
		{},
		createRequest,
		embeddedExternals,
	);
	const cookieStr: string = result.body.cookie || "";
	const musicA = cookieStr
		.split(";")
		.map((part) => part.trim())
		.find((part) => part.startsWith("MUSIC_A="));
	if (musicA) {
		setAnonymousToken(musicA.slice("MUSIC_A=".length));
	}
	persistDeviceId();
}
