import { useCallback, useEffect, useRef } from "react";
import { corePlayer } from "@/lib/player/corePlayer";
import { resolvePlayableSource } from "@/lib/player/song-source";
import type { Song } from "@/lib/types";
import { createLruCache } from "@/lib/utils/lru";

/**
 * 播放预热（预取）。
 *
 * 原生侧的 `player_preload` 走的是与正式播放同一条音源解析路径，会把整首拉下来
 * 落盘到播放缓存里。于是「预取过 → 点歌」只剩本地解码耗时，也就是真正的秒切。
 * 旧实现的预热只有一个触发点：当前曲剩余 ≤10 秒时预取下一首——对"首次点一首歌"
 * 完全没有提前量，用户点下去才刚开始拉流。
 *
 * 这里补上按意图触发的入口：列表行悬停驻留。
 */

/** 悬停驻留多久才认为「用户打算播这首」 */
export const PREFETCH_HOVER_DELAY_MS = 180;

/**
 * 两次预取之间的最小间隔。
 *
 * 预取会下**整首**，放任鼠标快速划过一屏就等于同时开十几个下载，把带宽从
 * 正在播放的那一首手里抢走——与「秒切」的目标正好相反。所以宁可少预取几次。
 */
const PREFETCH_MIN_INTERVAL_MS = 800;

/**
 * 「用户正在连续切歌」的窗口。
 *
 * 这个时刻由播放侧在每次点歌时刷新（`noteManualSongSwitch`）。处在窗口里就说明
 * 手还在快速划过列表：此时悬停预取下的整首歌，几乎一定马上又被换掉——正是要省掉
 * 的那份开销（抢带宽、抢原生线程）。所以连续切歌期间不做非紧急预取。
 */
const SWITCH_BURST_WINDOW_MS = 1200;
let lastManualSwitchAt = 0;

export function noteManualSongSwitch(at = Date.now()) {
	lastManualSwitchAt = at;
}

/** 此刻是否正在连续切歌 */
export function inSwitchBurst(now = Date.now()): boolean {
	return now - lastManualSwitchAt < SWITCH_BURST_WINDOW_MS;
}

/** 已发起过的歌曲。只要发起过就不再重复（含解析失败的），避免反复重下同一首。
 *  失败不会影响正式播放：播放路径不查这张表，只是这一首少了一次预热。 */
const attempted = createLruCache<string, true>(64);

let lastStartedAt = 0;

function prefetchAllowedByNetwork() {
	if (typeof navigator === "undefined") return true;
	if (navigator.onLine === false) return false;
	// 计量网络 / 省流量模式下不做任何后台预取：预热属于"额外流量"
	const connection = (
		navigator as Navigator & { connection?: { saveData?: boolean } }
	).connection;
	return !connection?.saveData;
}

/**
 * 预热一首歌：解析出播放地址并让原生侧提前拉流落盘。
 *
 * @param urgent 绕过最小间隔限制。自动切歌的"下一首"预取不该被悬停节流挡住；
 *               已发起过的歌（attempted 命中）依然跳过，不会重复下载。
 * @returns 本次是否真的发起了预取
 */
export function prefetchSong(song: Song | undefined, urgent = false): boolean {
	if (!song || song.localFilePath) return false;

	const key = String(song.id);
	if (attempted.has(key)) return false;
	if (!urgent && Date.now() - lastStartedAt < PREFETCH_MIN_INTERVAL_MS) {
		return false;
	}
	// 连续切歌期间不做（非紧急）预取：预取下的是整首歌，而这些歌马上又会被换掉
	if (!urgent && inSwitchBurst()) return false;
	if (!prefetchAllowedByNetwork()) return false;

	attempted.set(key, true);
	lastStartedAt = Date.now();

	const controller = new AbortController();
	resolvePlayableSource(song, controller.signal)
		.then((source) => {
			if (source) corePlayer.preload(source.url, song.id, source.levelKey);
		})
		.catch((error) => {
			console.debug("[PREFETCH] 跳过预取:", error);
		});

	return true;
}

/**
 * 歌曲行的悬停预取：驻留 `PREFETCH_HOVER_DELAY_MS` 才发起，移开立即取消定时器。
 * 悬停是"即将点这首"最强的现成信号，比按可视区整片预取省得多。
 */
export function useSongPrefetchOnHover(song: Song, enabled = true) {
	const timer = useRef<number | null>(null);

	const cancel = useCallback(() => {
		if (timer.current !== null) {
			window.clearTimeout(timer.current);
			timer.current = null;
		}
	}, []);

	const onMouseEnter = useCallback(() => {
		if (!enabled) return;
		cancel();
		timer.current = window.setTimeout(() => {
			timer.current = null;
			prefetchSong(song);
		}, PREFETCH_HOVER_DELAY_MS);
	}, [cancel, enabled, song]);

	// 行被卸载（换页 / 列表回收）时清掉挂着的定时器
	useEffect(() => cancel, [cancel]);

	return { onMouseEnter, onMouseLeave: cancel };
}
