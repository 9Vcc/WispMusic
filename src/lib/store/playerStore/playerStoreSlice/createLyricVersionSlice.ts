import { toast } from "sonner";
import type { StateCreator } from "zustand";
import {
	fetchKugouLyricText,
	searchKugouLyricVersions,
	searchKugouSong,
} from "@/lib/services/kugouLyric";
import type {
	LyricVersionSelection,
	LyricVersionSlice,
	SharedPlayerState,
} from "@/lib/types/player";
import { ParseLyric, type ILyricLine } from "@/lib/utils/lyric-parser";

/**
 * lyricVersionLines 只在用户手动切换歌词版本时写入，但原先没有上限，会按
 * songId 累积解析后的逐字行（每条可达数百 KB），跨歌永久驻留。这里只保留
 * 最近加载的若干首——手动挑版本的人通常只在那几首之间来回。
 *
 * 注意：数字键的普通对象不保证插入顺序，必须另存一份顺序表才能正确淘汰。
 */
const LYRIC_VERSION_LINES_LIMIT = 5;
const lyricVersionLinesOrder: number[] = [];

function rememberLyricVersionLines(
	current: Record<number, ILyricLine[]>,
	songId: number,
	lines: ILyricLine[],
): Record<number, ILyricLine[]> {
	const seen = lyricVersionLinesOrder.indexOf(songId);
	if (seen !== -1) lyricVersionLinesOrder.splice(seen, 1);
	lyricVersionLinesOrder.push(songId);

	const next = { ...current, [songId]: lines };
	while (lyricVersionLinesOrder.length > LYRIC_VERSION_LINES_LIMIT) {
		const evicted = lyricVersionLinesOrder.shift();
		if (evicted === undefined) break;
		delete next[evicted];
	}
	return next;
}

function forgetLyricVersionLines(songId: number): void {
	const seen = lyricVersionLinesOrder.indexOf(songId);
	if (seen !== -1) lyricVersionLinesOrder.splice(seen, 1);
}


/**
 * 歌词版本切片：参考 EchoMusic 的 LyricSourceDialog / manualLyricMap。
 *
 * 网易云每首歌只有一份官方歌词，但酷狗歌词库对同一首歌常有多个版本
 * （官方推荐 / 用户上传 / 第三方，逐行或逐字，时间轴质量参差）。
 * 当默认歌词缺失、时间轴糟糕或不完整时，用户可以从候选里挑一版替换。
 *
 * 状态设计：
 * - `lyricVersionMap`（持久化）按歌曲 id 记住「选了哪个版本」的描述信息，
 *   重启后据此重新拉取内容——酷狗的 accesskey 会过期，所以只存可再次定位的元数据；
 * - `lyricVersionLines`（内存）存解析后的行，随选随用；
 * - 候选列表按当前歌曲缓存，切歌后重新搜索。
 */
export const createLyricVersionSlice: StateCreator<
	SharedPlayerState,
	[],
	[],
	LyricVersionSlice
> = (set, get) => ({
	lyricVersionMap: {},
	lyricVersionLines: {},
	lyricVersionCandidates: [],
	lyricVersionStatus: "idle",
	lyricVersionSongId: null,

	loadLyricVersionCandidates: async (force = false) => {
		const song = get().currentSong;
		if (!song) return;

		const songId = song.id;
		if (
			!force &&
			get().lyricVersionSongId === songId &&
			get().lyricVersionCandidates.length > 0
		) {
			return;
		}

		set({ lyricVersionStatus: "loading", lyricVersionSongId: songId });
		try {
			const artistNames = (song.ar ?? [])
				.map((artist) => artist.name)
				.filter(Boolean);
			const keyword = [song.name, ...artistNames].join(" ").trim() || song.name;
			// 歌曲时长（毫秒）：优先本地元数据，其次网易云的 dt（毫秒）
			const localDuration = song.localOriginalMetadata?.durationSecs ?? 0;
			const durationMs =
				localDuration > 0
					? Math.round(localDuration * 1000)
					: song.dt > 0
						? song.dt
						: undefined;

			const match = await searchKugouSong(keyword, durationMs);
			if (!match) {
				set({ lyricVersionCandidates: [], lyricVersionStatus: "idle" });
				toast.info("酷狗歌词库中没有找到这首歌的候选版本");
				return;
			}

			const versions = await searchKugouLyricVersions({
				hash: match.hash,
				keyword: match.songName ?? song.name,
				durationMs: match.durationMs ?? durationMs,
				albumAudioId: match.albumAudioId,
			});
			set({ lyricVersionCandidates: versions, lyricVersionStatus: "idle" });
			if (versions.length === 0) {
				toast.info("酷狗歌词库中没有找到这首歌的候选版本");
			}
		} catch (error) {
			console.error("[LyricVersion] 搜索歌词候选失败:", error);
			set({ lyricVersionCandidates: [], lyricVersionStatus: "error" });
			toast.error("歌词版本搜索失败", {
				description: error instanceof Error ? error.message : undefined,
			});
		}
	},

	applyLyricVersion: async (version) => {
		const song = get().currentSong;
		if (!song) return false;

		const songId = song.id;
		try {
			const text = await fetchKugouLyricText(version);
			const lines = ParseLyric(text);
			if (!lines || lines.length === 0) {
				toast.error("该歌词版本没有可解析的内容");
				return false;
			}

			set({
				lyricVersionMap: { ...get().lyricVersionMap, [songId]: version },
				lyricVersionLines: rememberLyricVersionLines(
					get().lyricVersionLines,
					songId,
					lines,
				),
			});
			return true;
		} catch (error) {
			console.error("[LyricVersion] 应用歌词版本失败:", error);
			toast.error("歌词版本应用失败", {
				description: error instanceof Error ? error.message : undefined,
			});
			return false;
		}
	},

	clearLyricVersion: async () => {
		const song = get().currentSong;
		if (!song) return;
		if (!(song.id in get().lyricVersionMap)) return;

		const map = { ...get().lyricVersionMap };
		delete map[song.id];
		const lines = { ...get().lyricVersionLines };
		delete lines[song.id];
		forgetLyricVersionLines(song.id);
		set({ lyricVersionMap: map, lyricVersionLines: lines });
	},
});

/** 当前歌曲生效的手动歌词版本（无则回退在线歌词） */
export const selectActiveLyricVersion = (
	state: SharedPlayerState,
): LyricVersionSelection | null =>
	state.currentSong
		? (state.lyricVersionMap[state.currentSong.id] ?? null)
		: null;

/** 当前歌曲手动版本已解析的歌词行 */
export const selectActiveLyricVersionLines = (
	state: SharedPlayerState,
): import("@/lib/utils/lyric-parser").ILyricLine[] | null =>
	state.currentSong
		? (state.lyricVersionLines[state.currentSong.id] ?? null)
		: null;
