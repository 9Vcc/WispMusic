import {
	applyFallbackWordTiming,
	hasWordTiming,
	type ILyricLine,
	mergeWordTiming,
	ParseLyric,
	ParseVerbatimLyric,
} from "./lyric-parser";

/** 歌词页最终的歌词从哪里来 */
export type LyricOrigin = "manual" | "local" | "verbatim" | "plain" | "none";

export interface LyricResolution {
	lines: ILyricLine[];
	origin: LyricOrigin;
	/** 逐字动效是否可用（含按行时长兜底生成的时间轴） */
	hasWordTiming: boolean;
}

export interface ResolveLyricOptions {
	/**
	 * 手动选择的歌词版本（酷狗候选，按歌曲记忆）。
	 * 用户明确点过「应用」，优先级最高——高于本地歌词文件与在线歌词。
	 */
	manualLines?: readonly ILyricLine[] | null;
	/** 当前生效的本地歌词（手动选择或同目录自动匹配），优先于在线歌词 */
	localLines?: readonly ILyricLine[] | null;
	/** 在线逐字歌词（yrc）原文 */
	verbatimSource?: string;
	/** 在线逐行歌词（lrc）原文 */
	plainSource?: string;
	/** 缺行尾时间时兜底生成逐字时间轴所用的行时长 */
	fallbackLineDurationMs?: number;
}

const EMPTY: LyricResolution = {
	lines: [],
	origin: "none",
	hasWordTiming: false,
};

/**
 * 歌词页唯一的歌词来源解析入口。
 *
 * 歌词有两条独立来源：本地歌词文件（用户手动选择 / 同目录自动匹配）与在线歌词
 * （逐字 yrc + 逐行 lrc，解灰歌曲同样走这条）。修复前两者各自为政：
 *
 * - `ParseVerbatimLyric(yrc) || ParseLyric(lrc)` 用 `||` 串联，而空数组在 JS 里是
 *   真值——只要 yrc 解析出 0 行就会直接把逐行歌词一起挡掉，歌词页空白；
 * - 本地歌词一旦生效就整份替换在线歌词，即使本地文件只有逐行时间、而在线歌词
 *   带着完整的逐字时间轴，逐字动效也会被丢掉。
 *
 * 结果就是同一首歌「歌词从哪来」决定了有没有逐字动效（本地 .yrc 有、解灰歌曲的
 * 在线歌词可能没有、本地 .lrc 没有）。这里统一成一条链路：
 *
 * 1. 取有内容的一方做底（本地 > 在线逐字 > 在线逐行，一律按行数判断，不用 `||`）；
 * 2. 底不是在线逐字歌词时，按最近行把在线逐字时间轴贴回来（文本一致才贴）；
 * 3. 仍没有逐字时间的行按行时长生成兜底时间轴，保证任何来源的歌词都有同一套动效。
 */
export function resolveLyricLines({
	manualLines,
	localLines,
	verbatimSource,
	plainSource,
	fallbackLineDurationMs,
}: ResolveLyricOptions): LyricResolution {
	const verbatim = ParseVerbatimLyric(verbatimSource) ?? [];
	const plain = ParseLyric(plainSource) ?? [];
	const manual = manualLines ?? [];
	const local = localLines ?? [];

	let lines: ILyricLine[];
	let origin: LyricOrigin;

	if (manual.length > 0) {
		lines = [...manual];
		origin = "manual";
	} else if (local.length > 0) {
		lines = [...local];
		origin = "local";
	} else if (verbatim.length > 0) {
		lines = verbatim;
		origin = "verbatim";
	} else if (plain.length > 0) {
		lines = plain;
		origin = "plain";
	} else {
		return EMPTY;
	}

	if (origin !== "verbatim" && hasWordTiming(verbatim)) {
		lines = mergeWordTiming(lines, verbatim);
	}

	// 兜底补齐：逐行歌词、以及逐字歌词没覆盖到的行（如本地文件里的制作人员名单）
	// 都按行时长生成时间轴，保证同一首歌每一行都是同一套动效。
	lines = applyFallbackWordTiming(lines, fallbackLineDurationMs);

	return { lines, origin, hasWordTiming: hasWordTiming(lines) };
}
