import { RepeatType, ShuffleType } from "../constants/player";
import { QualityKey } from "../constants/song";
import type { ILyricLine } from "../utils/lyric-parser";
import { Song, SongLyric, SongQualityDetail } from "./song";

/** 播放队列来源：记录当前队列是从哪个歌单/专辑/歌手播放的，重启后可恢复上下文 */
export interface PlaySource {
	id: string | number;
	type: "list" | "album" | "artist";
	name: string;
}

export interface PlayerState {
	currentSong: Song | null;
	currentIndexInPlaylist: number;
	currentSongMusicDetail: SongQualityDetail[];
	currentSongLyrics: SongLyric | null;
	playlist: Song[];
	isPlaying: boolean;
	isLoadingMusic: boolean;
	preferMusicLevel: QualityKey;
	currentMusicLevel: QualityKey;
	repeatMode: RepeatType;
	isShuffle: ShuffleType;
	volume: number;
	playbackRate: number; // 播放倍速 0.1 ~ 5.0
	duration: number; // 当前歌曲总时长 s
	// 高频的 progress / currentTime 移至 progressStore，避免触发 persist 与大范围重渲染
}

// 播放列表切片类型定义
export interface PlaylistSlice {
	playlist: Song[];
	currentIndexInPlaylist: number;
	addToPlaylist: (song: Song) => void;
	clearPlaylist: () => void;
	removeFromPlaylist: (song: Song) => void;
	isInPlaylist: (song: Song) => boolean;

	fmPlaylist: Song[];
}

// 播放控制切片类型定义
export interface PlayerControlSlice {
	isPlaying: boolean;
	isLoadingMusic: boolean;
	repeatMode: RepeatType;
	isShuffle: ShuffleType;
	volume: number;
	playbackRate: number;
	duration: number;

	playSong: (song: Song, isFm?: boolean) => void;
	playQueue: (songs: Song[]) => void;
	playList: (
		listId: string | number,
		listType: "list" | "album" | "voicelist" | "userfm",
	) => void;
	playArtist: (artistId: string) => void;
	togglePlay: () => void;
	next: (isManual?: boolean) => void;
	prev: (isManual?: boolean) => void;
	updateProgress: () => void;
	updateVolume: (volume: number) => void;
	setPlaybackRate: (rate: number) => void;
	seek: (percentage: number) => void;
	toggleRepeatMode: () => void;
	toggleShuffleMode: () => void;
	cyclePlayMode: () => void;

	isFmMode: boolean; // 是否是漫游模式
	fmRepeatMode: boolean; // true：单曲循环，false：无限下一首
	toggleFmRepeatMode: () => void;
	playFm: () => void;
	fetchFmSongs: () => Promise<void>;
	nextFmSong: () => void;
	trashFmSong: () => Promise<void>; // 将当前播放的歌曲从 FM 加入垃圾桶
}

// 歌曲信息切片类型定义
export interface SongInfoSlice {
	currentSong: Song | null;
	currentSongMusicDetail: SongQualityDetail[];
	currentSongLyrics: SongLyric | null;
	currentMusicLevelKey: QualityKey;

	setCurrentMusicLevelKey: (level: QualityKey) => void;
}

export type SharedPlayerState = PlaylistSlice &
	PlayerControlSlice &
	SongInfoSlice &
	LocalLyricSlice &
	LyricOffsetSlice &
	LyricVersionSlice;

/** 本地歌词来源：manual=用户手动选择，auto=随歌曲自动匹配同目录同名文件 */
export type LocalLyricSource = "manual" | "auto";

export interface LocalLyricState {
	/** 歌词文件绝对路径 */
	filePath: string;
	/** 仅文件名，用于界面展示 */
	fileName: string;
	/** 已解析的歌词行，直接供歌词组件渲染 */
	lines: ILyricLine[];
	/** 是否为逐字歌词（yrc）格式 */
	isVerbatim: boolean;
	source: LocalLyricSource;
	/** 该歌词所属歌曲 id：切歌后旧歌词不再生效，避免张冠李戴 */
	songId: number;
}

/** 本地歌词切片：手动选择的歌词文件与「同目录同名歌词」自动匹配 */
export interface LocalLyricSlice {
	localLyric: LocalLyricState | null;
	isLoadingLocalLyric: boolean;
	/** 最近一次加载失败的提示文案（含文件名），成功或清除后为 null */
	localLyricError: string | null;

	/** 打开文件选择器加载本地歌词；返回是否加载成功（用户取消返回 false） */
	pickLocalLyric: () => Promise<boolean>;
	/**
	 * 尝试加载与音频同目录、同名的歌词文件。
	 * 非本地歌曲、关闭自动匹配或没有同名文件时都会清空当前本地歌词。
	 */
	loadSiblingLyric: (
		song: Song | null,
		options?: { silent?: boolean; force?: boolean },
	) => Promise<boolean>;
	/** 清除当前本地歌词，回退到在线歌词 */
	clearLocalLyric: () => void;
}

/**
 * 歌词偏移切片：按歌曲持久化的时间偏移（毫秒），用于人工对齐人声。
 *
 * 正值 = 歌词整体提前。参考 EchoMusic 的 timeOffsetMap 设计：
 * 按歌曲 id 记忆（解灰音源/不同母带的时间轴差异是逐首歌的），重启后仍然生效。
 */
export interface LyricOffsetSlice {
	/** key = 歌曲 id，value = 偏移毫秒（正值 = 歌词提前） */
	lyricTimeOffsetMap: Record<number, number>;
	/** 在当前歌曲的偏移上叠加 delta（毫秒），返回调整后的值 */
	adjustLyricOffset: (deltaMs: number) => number;
	/** 直接设置当前歌曲的歌词偏移（毫秒） */
	setLyricOffset: (offsetMs: number) => void;
	/** 清除当前歌曲的歌词偏移 */
	resetLyricOffset: () => void;
}

/** 手动选择的歌词版本（酷狗歌词候选），按歌曲持久化，仅存可再次取回的描述信息 */
export interface LyricVersionSelection {
	/** 酷狗歌词 id */
	id: string;
	accesskey: string;
	/** 1=逐字(KRC)，2=逐行(LRC) */
	krctype: number;
	/** 推荐分（0-100），用于星级展示 */
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

/** 歌词版本切片：多版本歌词的搜索、按歌曲记忆与应用 */
export interface LyricVersionSlice {
	/** 手动选择的歌词版本，key = 歌曲 id（持久化，存可再次取回的描述信息） */
	lyricVersionMap: Record<number, LyricVersionSelection>;
	/** 已解析的歌词行缓存（内存），key = 歌曲 id */
	lyricVersionLines: Record<number, ILyricLine[]>;
	/** 版本选择对话框的候选列表（对应最近一次搜索的歌曲） */
	lyricVersionCandidates: LyricVersionSelection[];
	/** 候选搜索状态 */
	lyricVersionStatus: "idle" | "loading" | "error";
	/** 候选搜索所针对的歌曲 id */
	lyricVersionSongId: number | null;

	/** 拉取当前歌曲的多版本候选列表（force=true 时忽略同曲缓存） */
	loadLyricVersionCandidates: (force?: boolean) => Promise<void>;
	/** 应用某个歌词版本到当前歌曲（拉取内容并解析） */
	applyLyricVersion: (version: LyricVersionSelection) => Promise<boolean>;
	/** 清除当前歌曲的手动选择，恢复在线歌词 */
	clearLyricVersion: () => Promise<void>;
}
