import {
	needsDowngrade,
	QUALITY_BY_KEY,
	QUALITY_BY_LEVEL,
	type QualityKey,
} from "@/lib/constants/song";
import { getSongUrl, unblockMusic } from "@/lib/services/song";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";
import type { Song } from "@/lib/types";
import { createLruCache } from "@/lib/utils/lru";

/**
 * 歌曲「元数据 → 可播放地址」的解析收敛点。
 *
 * 播放、预取、悬停预取三条路径都必须走这里，否则解析结果无法共享、
 * URL 缓存也各建一份。
 */

const UNBLOCK_SOURCE_PREFIX = "unblock:";

export function toUnblockSource(url: string) {
	return `${UNBLOCK_SOURCE_PREFIX}${url}`;
}

export interface ResolvedSongSource {
	url: string;
	levelKey: QualityKey;
}

/** 过期前留出的安全余量：卡在边界上取到的地址可能在播放中途失效 */
const URL_CACHE_SAFETY_MS = 60 * 1000;
/** 上限：一次会话里重复播放的歌远少于这个数，再多也只是白占内存 */
const URL_CACHE_MAX_TTL_MS = 30 * 60 * 1000;
const URL_CACHE_LIMIT = 64;

/**
 * 解灰结果缓存时长。
 *
 * 第三方音源地址通常比官方签名地址长寿，但为了不拿着失效地址起播失败，
 * 取一个保守值，并在真正播挂时立刻让这条缓存失效（见 invalidateUnblockCache）。
 */
const UNBLOCK_CACHE_TTL_MS = 10 * 60 * 1000;

const urlCache = createLruCache<
	string,
	{ value: ResolvedSongSource; expiresAt: number }
>(URL_CACHE_LIMIT);

/**
 * 由 `expi` 折算缓存时长。
 *
 * 各音源实现里 `expi` 既可能是绝对 Unix 时间戳（秒），也可能是「有效秒数」，
 * 按量级区分：10 亿以上只可能是时间戳（2001 年以后的秒级时间戳）。
 */
function cacheTtlMs(expi: number): number {
	if (!Number.isFinite(expi) || expi <= 0) return 0;
	const now = Date.now();
	const expiresAt = expi > 1e9 ? expi * 1000 : now + expi * 1000;
	return Math.min(expiresAt - now - URL_CACHE_SAFETY_MS, URL_CACHE_MAX_TTL_MS);
}

function readUrlCache(key: string) {
	const hit = urlCache.get(key);
	if (!hit) return undefined;
	if (hit.expiresAt <= Date.now()) {
		urlCache.delete(key);
		return undefined;
	}
	return hit.value;
}

export function getKnownMaxQualityKey(song: Song): QualityKey | undefined {
	const maxBrLevel = song.privilege?.maxBrLevel;
	if (!maxBrLevel) return undefined;
	return QUALITY_BY_LEVEL[maxBrLevel as keyof typeof QUALITY_BY_LEVEL]?.key;
}

/**
 * 按设置音质请求歌曲 URL。
 * 优先请求 preferKey（与手动切换音质行为一致，privilege.maxBrLevel 可能低于实际可用音质）；
 * 仅当无 URL 或返回试听片段(freeTrialInfo)时，才降级到 privilege 标记的最大可用音质重试。
 *
 * 命中 URL 缓存时直接返回，不再发起网络往返——同一首歌反复播放时，这一次
 * API 往返（含 renderer → Rust 的 IPC）原本是起播延迟里最靠前的固定开销。
 */
export async function resolveSongUrlWithQuality(
	songId: number | string,
	preferKey: QualityKey,
	knownMaxKey: QualityKey | undefined,
	signal: AbortSignal,
): Promise<ResolvedSongSource | undefined> {
	const attempts: QualityKey[] = [preferKey];
	if (
		knownMaxKey &&
		knownMaxKey !== preferKey &&
		needsDowngrade(preferKey, knownMaxKey)
	) {
		attempts.push(knownMaxKey);
	}

	for (const key of attempts) {
		const cacheKey = `${songId}:${key}`;
		const cached = readUrlCache(cacheKey);
		if (cached) return cached;

		const res = await getSongUrl(
			[songId.toString()],
			QUALITY_BY_KEY[key].level,
			false,
			signal,
		);
		const item = res?.[0];
		if (item?.url && !item.freeTrialInfo) {
			const resolved: ResolvedSongSource = {
				url: item.url,
				levelKey:
					QUALITY_BY_LEVEL[item.level as keyof typeof QUALITY_BY_LEVEL]?.key ??
					key,
			};
			const ttl = cacheTtlMs(item.expi);
			if (ttl > 0) {
				urlCache.set(cacheKey, {
					value: resolved,
					expiresAt: Date.now() + ttl,
				});
			}
			return resolved;
		}
	}

	return undefined;
}

// 按设置读取解灰开关与音源；关闭时直接放弃，音源留空（自动）时不传 source 让后端依次尝试
export async function requestUnblock(id: number, signal?: AbortSignal) {
	const { unblockEnabled, unblockSource } = useSettingStore.getState().network;
	if (!unblockEnabled) return undefined;
	return unblockMusic(id, signal, unblockSource || undefined);
}

/**
 * 解析出解灰后的可播放地址，并缓存结果。
 *
 * 解灰成功意味着刚刚付掉了一整条链路：sidecar 的 `/match` 往返（1~5s，内部还要
 * 依次问多个第三方音源）。不做缓存的话，同一首灰歌每次点都要重付一遍。
 */
export async function resolveUnblockedSource(
	songId: number,
	signal: AbortSignal,
): Promise<ResolvedSongSource | undefined> {
	const cacheKey = `unlock:${songId}`;
	const cached = readUrlCache(cacheKey);
	if (cached) return cached;

	const res = await requestUnblock(songId, signal);
	const data = typeof res?.data === "string" ? res.data : "";
	if (!data) return undefined;

	const resolved: ResolvedSongSource = {
		url: toUnblockSource(data),
		levelKey: "unlock",
	};
	urlCache.set(cacheKey, {
		value: resolved,
		expiresAt: Date.now() + UNBLOCK_CACHE_TTL_MS,
	});
	return resolved;
}

/** 解灰地址起播失败时调用：让缓存立刻失效，下次重新走 /match 拿新地址 */
export function invalidateUnblockCache(songId: number) {
	urlCache.delete(`unlock:${songId}`);
}

/**
 * 解析出「可直接交给播放器」的地址：可用歌曲走官方接口，拿不到（灰歌、
 * 请求失败、仅试听）时退回解灰。预取与悬停预取共用。
 *
 * 本地歌曲直接返回本地路径，不做任何网络请求。
 */
export async function resolvePlayableSource(
	song: Song,
	signal: AbortSignal,
): Promise<ResolvedSongSource | undefined> {
	if (song.localFilePath) {
		return { url: song.localFilePath, levelKey: "local" };
	}

	const preferKey = useSettingStore.getState().audio.preferQuality;
	const canPlay = song.privilege
		? song.privilege.st >= 0 && song.privilege.pl > 0
		: true;

	if (canPlay) {
		const resolved = await resolveSongUrlWithQuality(
			song.id,
			preferKey,
			getKnownMaxQualityKey(song),
			signal,
		);
		if (resolved) return resolved;
	}

	// 灰歌顺带预热解灰：无论调用方最后用不用得上，命中都会被会话级 LRU 记住
	return resolveUnblockedSource(song.id, signal);
}
