import { base64ToBytes, fetchViaRust } from "@/lib/ncm/fetch";

/**
 * 酷狗歌词公开接口（已实测可用，参考 EchoMusic 的多版本歌词功能）：
 * 1. mobiles.kugou.com 按歌名+歌手搜索歌曲，拿 hash / album_audio_id / duration
 * 2. lyrics.kugou.com/search?man=yes 返回同一首歌的多个歌词版本候选
 *    （官方推荐 / 用户上传 ugc / 第三方，带 score 星级、krctype、language、时长）
 * 3. lyrics.kugou.com/download?fmt=lrc&decode=true 返回 base64 的 LRC 文本
 *
 * 全部走 Rust fetchViaRust（无 CORS 限制）。候选支持 fmt=krc（逐字 KRC），
 * 但其密文在 2025 年后已无法用公开的 krc1 XOR+deflate 算法解开（实测），
 * 因此这里固定取 LRC（逐行），逐字扫过由兜底时间轴生成。
 */

const KUGOU_SONG_SEARCH_URL = "https://mobiles.kugou.com/api/v3/search/song";
const KUGOU_LYRIC_SEARCH_URL = "https://lyrics.kugou.com/search";
const KUGOU_LYRIC_DOWNLOAD_URL = "https://lyrics.kugou.com/download";
const REQUEST_USER_AGENT = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

export interface KugouSongMatch {
	hash: string;
	albumAudioId?: number;
	/** 歌曲时长（毫秒） */
	durationMs?: number;
	songName?: string;
	singerName?: string;
}

export interface KugouLyricVersion {
	id: string;
	accesskey: string;
	/** 1=逐字(KRC)，2=逐行(LRC)；本应用统一下载 LRC 渲染 */
	krctype: number;
	score?: number;
	/** 歌词时长（毫秒） */
	durationMs?: number;
	/** 来源：官方推荐歌词 / ugc / 第三方歌词 */
	productFrom?: string;
	song?: string;
	singer?: string;
	language?: string;
	nickname?: string;
}

async function getJson<T>(url: string): Promise<T> {
	const res = await fetchViaRust(url, {
		headers: {
			"User-Agent": REQUEST_USER_AGENT,
			Referer: "https://kugou.com/",
		},
	});
	if (res.status < 200 || res.status >= 400) {
		throw new Error(`酷狗接口请求失败（HTTP ${res.status}）`);
	}
	try {
		return JSON.parse(res.body) as T;
	} catch {
		throw new Error("酷狗接口返回了无法解析的内容");
	}
}

/** 在酷狗搜索歌曲，返回与目标时长最接近的一首（拿歌词搜索所需的 hash） */
export async function searchKugouSong(
	keyword: string,
	durationMs?: number,
): Promise<KugouSongMatch | null> {
	const trimmed = keyword.trim();
	if (!trimmed) return null;

	const url = `${KUGOU_SONG_SEARCH_URL}?format=json&keyword=${encodeURIComponent(
		trimmed,
	)}&page=1&pagesize=8`;
	const payload = await getJson<{
		status?: number;
		data?: { info?: Array<Record<string, unknown>> };
	}>(url);

	const info = payload.data?.info ?? [];
	const matches = info
		.map((item) => {
			const durationSec = Number(item.duration ?? 0);
			return {
				hash: String(item.hash ?? ""),
				albumAudioId:
					item.album_audio_id != null ? Number(item.album_audio_id) : undefined,
				durationMs:
					durationSec > 0 ? Math.round(durationSec * 1000) : undefined,
				songName: typeof item.songname === "string" ? item.songname : undefined,
				singerName:
					typeof item.singername === "string" ? item.singername : undefined,
			} satisfies KugouSongMatch;
		})
		.filter((match) => match.hash);

	if (matches.length === 0) return null;
	if (!durationMs) return matches[0];

	return matches.reduce((best, item) =>
		Math.abs((item.durationMs ?? 0) - durationMs) <
		Math.abs((best.durationMs ?? 0) - durationMs)
			? item
			: best,
	);
}

interface KugouLyricSearchPayload {
	status?: number;
	candidates?: Array<Record<string, unknown>>;
	info?: Array<Record<string, unknown>>;
}

function toVersion(item: Record<string, unknown>): KugouLyricVersion | null {
	const id = item.id != null ? String(item.id) : "";
	const accesskey = item.accesskey != null ? String(item.accesskey) : "";
	if (!id || !accesskey) return null;
	const duration = Number(item.duration ?? 0);
	return {
		id,
		accesskey,
		krctype: Number(item.krctype ?? 0),
		score: item.score != null ? Number(item.score) : undefined,
		durationMs: duration > 0 ? duration : undefined,
		productFrom:
			typeof item.product_from === "string" ? item.product_from : undefined,
		song: typeof item.song === "string" ? item.song : undefined,
		singer: typeof item.singer === "string" ? item.singer : undefined,
		language: typeof item.language === "string" ? item.language : undefined,
		nickname: typeof item.nickname === "string" ? item.nickname : undefined,
	};
}

/** 搜索一首歌的全部歌词版本候选（man=yes） */
export async function searchKugouLyricVersions(options: {
	hash: string;
	keyword: string;
	durationMs?: number;
	albumAudioId?: number;
}): Promise<KugouLyricVersion[]> {
	const params = new URLSearchParams({
		ver: "1",
		man: "yes",
		client: "pc",
		hash: options.hash,
		keyword: options.keyword,
	});
	if (options.durationMs) params.set("duration", String(options.durationMs));
	if (options.albumAudioId)
		params.set("album_audio_id", String(options.albumAudioId));

	const payload = await getJson<KugouLyricSearchPayload>(
		`${KUGOU_LYRIC_SEARCH_URL}?${params.toString()}`,
	);
	const raw = payload.candidates ?? payload.info ?? [];
	return raw
		.map(toVersion)
		.filter((version): version is KugouLyricVersion => version !== null);
}

/** 下载某个歌词版本的内容（LRC 文本，decode=true + base64 兜底解析） */
export async function fetchKugouLyricText(
	version: Pick<KugouLyricVersion, "id" | "accesskey">,
): Promise<string> {
	const params = new URLSearchParams({
		ver: "1",
		client: "pc",
		id: version.id,
		accesskey: version.accesskey,
		fmt: "lrc",
		decode: "true",
	});
	const payload = await getJson<{ status?: number; content?: string }>(
		`${KUGOU_LYRIC_DOWNLOAD_URL}?${params.toString()}`,
	);
	const content = payload.content ?? "";
	if (!content) throw new Error("酷狗歌词接口没有返回内容");

	// decode=true 时 content 是 base64；兼容直接返回明文的情况
	const decoded = new TextDecoder("utf-8").decode(base64ToBytes(content));
	const text = decoded.includes("[") ? decoded : content;
	if (!text.trim()) throw new Error("该歌词版本没有可用内容");
	return text;
}
