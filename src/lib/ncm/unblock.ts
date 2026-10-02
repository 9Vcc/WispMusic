// 解灰实现分两层：
// 1. 本地 unm 引擎（@unblockneteasemusic/server，与 VutronMusic 同款）：
//    跨平台搜索匹配（酷我/酷狗/咪咕/QQ/B站等），可解出无损，经 Rust 拉起的 Node sidecar 调用；
//    环境变量与设置项（enableFlac/orderFirst/qqCookie/jooxCookie）与 VutronMusic 完全一致
// 2. 纯 HTTP 音源（@neteasecloudmusicapienhanced/unblockmusic-utils@0.4.4 matchID 移植版）：
//    unm 不可用（无 Node）或匹配失败时的回退；音质按设置中的「音频质量」经 qualityValue 映射

import { invoke } from "@tauri-apps/api/core";
import { QUALITY_BY_KEY, type QualityKey } from "../constants/song";
import { createLruCache } from "../utils/lru";
import { fetchViaRust, type NcmFetchResponse } from "./fetch";

export interface UnblockMatchResult {
	code: number;
	message: string;
	data: { url: string; source: string } | null;
}

interface UnblockSource {
	name: string;
	/** 将设置中的音质 key 映射为该音源请求参数的音质值；缺省表示音源不支持按音质请求 */
	qualityValue?: (key: QualityKey) => string;
	match(id: string, quality?: string): Promise<string | null>;
}

async function httpGetJson(
	url: string,
): Promise<Record<string, any> | undefined> {
	const res = await fetchViaRust(url);
	if (res.status < 200 || res.status >= 400) return undefined;
	try {
		return JSON.parse(res.body) as Record<string, any>;
	} catch {
		return undefined;
	}
}

// —— 解灰链路的超时预算 ——

/**
 * 整条解灰链的预算：超过它就返回「无可用音源」。
 *
 * 这一层是**唯一能保证 `await resolveUnblockedSource(...)` 一定返回**的机制。
 * 缺了它，playSong 会永远停在音源解析上：corePlayer.play 从未被调用，
 * store.isLoadingMusic 停在 true，用户看到的就是「切歌之后一直转圈」。
 * 第三方音源慢是常态，但慢不等于可以无限等。
 */
const MATCH_TOTAL_BUDGET_MS = 25_000;
/**
 * 本地 unm 引擎的预算。故意小于总预算：unm 解不出来还要留给 7 个 HTTP 回退源
 * 机会，否则整条链的时间全被一个引擎吃掉。
 */
const MATCH_UNM_BUDGET_MS = 15_000;
/** 单个第三方 HTTP 源的预算（这些免费接口挂起时往往一个字都不回） */
const MATCH_SOURCE_BUDGET_MS = 8_000;

/**
 * 给一个可能不返回的等待加预算。
 *
 * 只让**等待方**按时脱身：task 本身继续跑，不会被取消（Tauri IPC 也取消不了）。
 * 所以这不是放弃这次匹配，只是不把交互干吊在一口死井上。
 */
function withBudget<T>(
	task: Promise<T>,
	budgetMs: number,
	onTimeout: () => T,
): Promise<T> {
	let timer: ReturnType<typeof setTimeout> | null = null;
	const deadline = new Promise<T>((resolve) => {
		timer = setTimeout(() => resolve(onTimeout()), budgetMs);
	});
	// Promise.race 会给两个入参都挂上处理函数，task 事后 reject 不会成未捕获异常
	return Promise.race([task, deadline]).finally(() => {
		if (timer !== null) clearTimeout(timer);
	});
}

// bugpk / byfuns 直接使用 NCM 音质名，不支持的特殊音质(环绕声/母带等)按 hires 请求
const ncmLevelName = (key: QualityKey) => {
	const level = QUALITY_BY_KEY[key].level;
	return ["standard", "higher", "exhigh", "lossless", "hires"].includes(level)
		? level
		: "hires";
};

const bugpk: UnblockSource = {
	name: "bugpk",
	qualityValue: ncmLevelName,
	async match(id, quality) {
		try {
			const data = await httpGetJson(
				`https://api.bugpk.com/api/163_music?type=json&ids=${id}&quality=${quality ?? "hires"}`,
			);
			if (data) return data.url ?? null;
			return null;
		} catch {
			return null;
		}
	},
};

const byfuns: UnblockSource = {
	name: "byfuns",
	qualityValue: ncmLevelName,
	async match(id, quality) {
		try {
			const res = await fetchViaRust(
				`https://api.byfuns.top/1/?id=${id}&level=${quality ?? "lossless"}`,
				{ followRedirects: false },
			);
			if (res.status < 200 || res.status >= 400) return null;
			if (res.headers["location"]) return res.headers["location"];
			const url = res.body.trim();
			if (url.startsWith("http")) return url;
			return null;
		} catch {
			return null;
		}
	},
};

// ddyr 为 lx 系接口：128k / 320k / flac / hires
const ddyr: UnblockSource = {
	name: "ddyr",
	qualityValue: (key) => {
		switch (key) {
			case "l":
				return "128k";
			case "m":
			case "h":
				return "320k";
			case "sq":
				return "flac";
			default:
				return "hires";
		}
	},
	async match(id, quality) {
		try {
			const data = await httpGetJson(
				`https://yy.zddyr.top/lx/api/?source=netease&songmid=${id}&quality=${quality ?? "hires"}`,
			);
			if (data) return data.url ?? null;
			return null;
		} catch {
			return null;
		}
	},
};

// gdmusic 以 br 请求数字码率：128 / 320 / 999(无损 flac)
const gdmusic: UnblockSource = {
	name: "gdmusic",
	qualityValue: (key) => {
		switch (key) {
			case "l":
				return "128";
			case "m":
			case "h":
				return "320";
			default:
				return "999";
		}
	},
	async match(id, quality) {
		try {
			const data = await httpGetJson(
				`https://music-api.gdstudio.xyz/api.php?types=url&source=netease&id=${id}&br=${quality ?? "999"}`,
			);
			if (data && typeof data === "object" && data.url) return data.url;
			return null;
		} catch {
			return null;
		}
	},
};

// msls / qijieya 为两段式语义：先不跟随重定向读 3xx 的 Location 头，
// 失败再跟随重定向读响应体（响应体为 http 开头的文本则直接作为播放地址）
async function twoPhaseMatch(url: string): Promise<string | null> {
	try {
		const res = await fetchViaRust(url, { followRedirects: false });
		if (res.status >= 200 && res.status < 400) {
			if (res.headers["location"]) return res.headers["location"];
			let data: Record<string, any> | undefined;
			try {
				data = JSON.parse(res.body) as Record<string, any>;
			} catch {
				data = undefined;
			}
			if (data && typeof data === "object" && data.url) return data.url;
			return null;
		}
	} catch {
		// 进入第二阶段
	}
	try {
		const res: NcmFetchResponse = await fetchViaRust(url);
		if (typeof res.body === "string" && res.body.startsWith("http")) {
			return res.body;
		}
		return null;
	} catch {
		return null;
	}
}

const msls: UnblockSource = {
	name: "msls",
	async match(id) {
		return twoPhaseMatch(`https://api.msls1441.com/?type=url&id=${id}`);
	},
};

const oi: UnblockSource = {
	name: "oi",
	async match(id) {
		try {
			const data = await httpGetJson(
				`https://oiapi.net/api/Music_163?id=${id}`,
			);
			if (data && data.data && data.data[0]) return data.data[0].url ?? null;
			return null;
		} catch {
			return null;
		}
	},
};

const qijieya: UnblockSource = {
	name: "qijieya",
	async match(id) {
		return twoPhaseMatch(`https://api.qijieya.cn/meting/?type=url&id=${id}`);
	},
};

// 顺序与原包 fs.readdirSync 的字母序一致
const sources: UnblockSource[] = [
	bugpk,
	byfuns,
	ddyr,
	gdmusic,
	msls,
	oi,
	qijieya,
];

// 本地 unm 引擎可用的跨平台音源（youtube/ytdlp/pyncmd 需外部二进制，不提供）；
// 候选音源由设置页自由填写（逗号分隔），留空时 unm-server 使用该默认列表
export const UNM_SOURCE_NAMES = [
	"bodian",
	"kuwo",
	"kugou",
	"migu",
	"qq",
	"bilibili",
] as const;

// 设置的音频质量经 store 读取；动态导入以避免 settingStore → api → ncm → unblock 的静态循环依赖
async function getNetworkUnblockSettings() {
	try {
		const { useSettingStore } = await import(
			"../store/settingStore/settingStore"
		);
		const { network } = useSettingStore.getState();
		return {
			enableFlac: network.unblockEnableFlac,
			orderFirst: network.unblockOrderFirst,
			qqCookie: network.unblockQqCookie,
			jooxCookie: network.unblockJooxCookie,
		};
	} catch {
		return {
			enableFlac: true,
			orderFirst: true,
			qqCookie: "",
			jooxCookie: "",
		};
	}
}

async function getPreferQualityKey(): Promise<QualityKey | null> {
	try {
		const { useSettingStore } = await import(
			"../store/settingStore/settingStore"
		);
		return useSettingStore.getState().audio.preferQuality;
	} catch {
		return null;
	}
}

/**
 * 把「已经跑完但没人等了」的匹配结果登记进会话缓存的回调。
 *
 * 预算到点只是让等待方脱身，匹配本身停不下来（Tauri IPC 不可取消）。既然算力
 * 已经花了，迟到成功的地址就该留下——用户再点同一首就是缓存命中，而不是重付一遍
 * 整条链。没有这一步的话，慢音源的灰歌会每次都从 0 开始等。
 */
type MatchRecorder = (result: UnblockMatchResult) => void;

/**
 * 本地 unm 引擎匹配（Rust sidecar：Node + @unblockneteasemusic/server）。
 * 环境变量语义与 VutronMusic 一致（ENABLE_FLAC / FOLLOW_SOURCE_ORDER / QQ_COOKIE / JOOX_COOKIE）；
 * 失败返回空结果由调用方回退 HTTP 音源。
 */
async function matchViaUnm(
	id: string,
	source?: string,
	record?: MatchRecorder,
): Promise<UnblockMatchResult> {
	try {
		const settings = await getNetworkUnblockSettings();
		const call = invoke<{
			url: string;
			source: string;
			br: number | null;
			size: number | null;
			md5: string | null;
		} | null>("unm_match", {
			songmid: id,
			source: source || undefined,
			enableFlac: settings.enableFlac,
			orderFirst: settings.orderFirst,
			qqCookie: settings.qqCookie || undefined,
			jooxCookie: settings.jooxCookie || undefined,
		});
		const toResult = (
			data: { url: string; source: string } & Record<string, unknown>,
		): UnblockMatchResult => ({
			code: 200,
			message: "success",
			data: { url: data.url, source: data.source },
		});

		if (record) {
			// 双参 then 而不是 .then().catch()：后者会给 call 留下一条无人处理的
			// reject 分支（await 那条路径才是负责报错的）
			call.then(
				(data) => {
					if (data?.url) record(toResult(data));
				},
				() => {},
			);
		}

		const data = await withBudget(call, MATCH_UNM_BUDGET_MS, () => null);

		if (data?.url) {
			return toResult(data);
		}
		return { code: 500, message: "unm engine returned no url", data: null };
	} catch (error) {
		// Node 缺失/未安装依赖/匹配失败等：记录后由调用方回退 HTTP 音源
		console.warn("[unblock] local unm engine unavailable:", error);
		return {
			code: 500,
			message: (error as Error)?.message ?? "unm engine failed",
			data: null,
		};
	}
}

async function matchBySource(
	source: UnblockSource,
	id: string,
	record?: MatchRecorder,
): Promise<string | null> {
	const preferKey = await getPreferQualityKey();
	const quality =
		preferKey && source.qualityValue
			? source.qualityValue(preferKey)
			: undefined;
	const call = source.match(id, quality);

	if (record) {
		call.then(
			(url) => {
				if (!url) return;
				record({
					code: 200,
					message: "success",
					data: { url, source: source.name },
				});
			},
			() => {},
		);
	}

	// 单个源只占预算内的一小段：它挂起时（免费接口常见）必须让位给下一个源，
	// 否则整条回退链会被第一个死源吃掉
	return withBudget(call, MATCH_SOURCE_BUDGET_MS, () => null);
}

// —— 解灰结果缓存 ——

/**
 * 解灰结果会话级缓存。
 *
 * 解灰一次要拉起本地引擎、依次打若干第三方音源，整链常常 1~5s；而同一首歌在
 * 切歌来回、单曲循环里会被反复请求。缓存掉重复的解灰，收益比预热更持久。
 *
 * · 只缓存成功结果——失败不缓存，音源是外部服务，一次超时不代表下次也不行；
 * · 带 TTL——拿到的是第三方 CDN 直链，可能有时效，过期就重新解一次；
 * · key 覆盖所有影响结果的输入（音源名单、无损/顺序优先、两个 Cookie、音质偏好），
 *   用户改了设置 key 就变，不会把旧设置下解出来的地址当成新设置的结果。
 */
const MATCH_CACHE_LIMIT = 60;
const MATCH_CACHE_TTL_MS = 30 * 60 * 1000;

const matchCache = createLruCache<
	string,
	{ result: UnblockMatchResult; at: number }
>(MATCH_CACHE_LIMIT);

async function matchCacheKey(id: string, sourceNames: string[]) {
	const settings = await getNetworkUnblockSettings();
	const preferQuality = await getPreferQualityKey();
	return [
		id,
		sourceNames.join(","),
		settings.enableFlac ? "flac" : "no-flac",
		settings.orderFirst ? "order-first" : "speed-first",
		settings.qqCookie,
		settings.jooxCookie,
		preferQuality ?? "",
	].join("|");
}

function readMatchCache(key: string): UnblockMatchResult | undefined {
	const hit = matchCache.get(key);
	if (!hit) return undefined;
	if (Date.now() - hit.at >= MATCH_CACHE_TTL_MS) {
		matchCache.delete(key);
		return undefined;
	}
	return hit.result;
}

export async function matchID(
	id: string,
	source?: string | null,
): Promise<UnblockMatchResult> {
	if (!id) {
		return { code: 400, message: "Missing id parameter", data: null };
	}

	// 音源为逗号分隔的自由文本（与 VutronMusic 一致）；空 = 自动模式
	const sourceNames = (source ?? "")
		.split(",")
		.map((s) => s.trim().toLowerCase())
		.filter(Boolean);

	const cacheKey = await matchCacheKey(id, sourceNames);
	const cached = readMatchCache(cacheKey);
	if (cached) return cached;

	// 缓存写入交给 record：不分「谁先被等到」。等待方按整链预算脱身后，迟到的
	// 成功仍会经同一个回调落进缓存，用户再点同一首就是命中而不是重付一遍。
	const record: MatchRecorder = (result) => {
		if (result.data) {
			matchCache.set(cacheKey, { result, at: Date.now() });
		}
	};

	return withBudget(
		resolveMatch(id, sourceNames, record),
		MATCH_TOTAL_BUDGET_MS,
		() => ({
			code: 500,
			message: `unblock timed out after ${MATCH_TOTAL_BUDGET_MS}ms`,
			data: null,
		}),
	);
}

/** 真正发起匹配的两条分支（自动模式 / 指定音源），由 matchID 负责缓存 */
async function resolveMatch(
	id: string,
	sourceNames: string[],
	record: MatchRecorder,
): Promise<UnblockMatchResult> {
	// 自动模式：本地 unm 引擎优先（可解无损），失败回退全部纯 HTTP 音源
	if (sourceNames.length === 0) {
		const unmResult = await matchViaUnm(id, undefined, record);
		if (unmResult.data) {
			return unmResult;
		}

		for (const s of sources) {
			try {
				const url = await matchBySource(s, id, record);
				if (url) {
					return {
						code: 200,
						message: "success",
						data: { url, source: s.name },
					};
				}
			} catch {}
		}

		return { code: 500, message: "No available source found", data: null };
	}

	// 指定音源：整表交给 unm 引擎（其内部过滤未知音源）；
	// unm 失败后仅在指定名单内回退 HTTP 音源
	const unmResult = await matchViaUnm(
		id,
		sourceNames.join(","),
		record,
	);
	if (unmResult.data) {
		return unmResult;
	}

	const httpSources = sources.filter((s) => sourceNames.includes(s.name));
	for (const s of httpSources) {
		try {
			const url = await matchBySource(s, id, record);
			if (url) {
				return {
					code: 200,
					message: "success",
					data: { url, source: s.name },
				};
			}
		} catch {}
	}

	return {
		code: 500,
		message: `No available source found from ${sourceNames.join(", ")}`,
		data: null,
	};
}
