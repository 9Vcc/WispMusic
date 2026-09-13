import type { StateCreator } from "zustand";
import {
	clampLyricOffset,
	DEFAULT_LYRIC_OFFSET_MS,
} from "@/lib/constants/lyric";
import type { LyricOffsetSlice, SharedPlayerState } from "@/lib/types/player";

/**
 * 歌词偏移切片：按歌曲持久化的时间偏移（EchoMusic 的 timeOffsetMap 模式）。
 *
 * 设计要点：
 * - 以歌曲 id 为 key 持久化——偏移是「这首歌的时间轴相对音源偏了多少」，
 *   换歌就要换基准，所以必须逐首记忆；重启后依然生效。
 * - 调整/重置静默生效，不打 toast；状态反馈由工具条的就地指示承担
 *   （偏移非 0 时重置按钮现身、按钮角落的绿点亮起）。
 * - 偏移只在歌词渲染的时基上生效（见 lyric.tsx），歌词数据本身不动，
 *   因此点击歌词行 seek 的目标仍是真实音频位置。
 */
export const createLyricOffsetSlice: StateCreator<
	SharedPlayerState,
	[],
	[],
	LyricOffsetSlice
> = (set, get) => ({
	lyricTimeOffsetMap: {},

	adjustLyricOffset: (deltaMs) => {
		const song = get().currentSong;
		if (!song) return DEFAULT_LYRIC_OFFSET_MS;

		const current =
			get().lyricTimeOffsetMap[song.id] ?? DEFAULT_LYRIC_OFFSET_MS;
		const next = clampLyricOffset(current + deltaMs);
		if (next === current) return current;

		set({
			lyricTimeOffsetMap: { ...get().lyricTimeOffsetMap, [song.id]: next },
		});
		return next;
	},

	setLyricOffset: (offsetMs) => {
		const song = get().currentSong;
		if (!song) return;

		const next = clampLyricOffset(offsetMs);
		if (next === (get().lyricTimeOffsetMap[song.id] ?? DEFAULT_LYRIC_OFFSET_MS))
			return;

		set({
			lyricTimeOffsetMap: { ...get().lyricTimeOffsetMap, [song.id]: next },
		});
	},

	resetLyricOffset: () => {
		const song = get().currentSong;
		if (!song) return;
		if (!(song.id in get().lyricTimeOffsetMap)) return;

		const map = { ...get().lyricTimeOffsetMap };
		delete map[song.id];
		set({ lyricTimeOffsetMap: map });
	},
});

/** 当前歌曲的歌词偏移（毫秒，正值 = 歌词提前） */
export const selectCurrentLyricOffset = (state: SharedPlayerState): number =>
	state.currentSong
		? (state.lyricTimeOffsetMap[state.currentSong.id] ??
			DEFAULT_LYRIC_OFFSET_MS)
		: DEFAULT_LYRIC_OFFSET_MS;
