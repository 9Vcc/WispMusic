export interface LyricWord {
	startTime: number;
	duration: number;
	char: string;
}

export interface ILyricLine {
	lineStart: number;
	lineEnd?: number; // 由空行时间戳标记的结束时间
	lineText: string;
	words?: LyricWord[]; // 逐字歌词用，逐行歌词为 undefined
	isBG?: boolean;
	isLeadDots?: boolean;
	leadDotsDuration?: number;
}

function insertLeadDots(lyrics: ILyricLine[]): ILyricLine[] {
	if (lyrics.length === 0) return lyrics;

	const MIN_GAP = 5000;
	const FIRST_DOT_DELAY = 1000;

	const result: ILyricLine[] = [];

	// 前奏：第一行歌词之前如果间隔足够长，插入 dots
	if (lyrics[0].lineStart >= MIN_GAP) {
		result.push({
			lineStart: 0,
			lineText: "···",
			isLeadDots: true,
			leadDotsDuration: lyrics[0].lineStart,
		});
	}

	for (let i = 0; i < lyrics.length; i++) {
		result.push(lyrics[i]);
		if (i < lyrics.length - 1) {
			const current = lyrics[i];
			const next = lyrics[i + 1];

			// 没有明确的结束时间标记，不插入 dots
			if (current.lineEnd == null) continue;

			const gap = next.lineStart - current.lineEnd;

			if (gap >= MIN_GAP) {
				// dots 出现在过门起点稍后
				const dotDelay = Math.min(FIRST_DOT_DELAY, gap * 0.3);
				const dotTime = current.lineEnd + dotDelay;
				result.push({
					lineStart: Math.round(dotTime),
					lineText: "···",
					isLeadDots: true,
					leadDotsDuration: next.lineStart - Math.round(dotTime),
				});
			}
		}
	}
	return result;
}

function checkIsBgLine(text: string): { text: string; isBG: boolean } {
	const trimmed = text.trim();
	if (trimmed.startsWith("（") && trimmed.endsWith("）")) {
		return { text: trimmed.slice(1, -1), isBG: true };
	}
	if (trimmed.startsWith("(") && trimmed.endsWith(")")) {
		return { text: trimmed.slice(1, -1), isBG: true };
	}
	return { text, isBG: false };
}

/**
 * 将 duration 为 0 的字符合并到相邻的非零字上，
 * 避免 useTransform 输入区间退化导致显示异常。
 * 向后合并：零时长字 append 到前一个字；
 * 行首的零时长字则 prepend 到后一个字。
 */
function mergeZeroDuration(words: LyricWord[]): LyricWord[] {
	if (words.length === 0) return words;

	const result: LyricWord[] = [];
	let pending = "";

	for (const w of words) {
		if (w.duration === 0) {
			if (result.length > 0) {
				result[result.length - 1] = {
					...result[result.length - 1],
					char: result[result.length - 1].char + w.char,
				};
			} else {
				pending += w.char;
			}
		} else {
			result.push({ ...w, char: pending + w.char });
			pending = "";
		}
	}

	if (pending && result.length > 0) {
		const last = result[result.length - 1];
		result[result.length - 1] = { ...last, char: last.char + pending };
	}

	return result;
}

/**
 * LRC 时间标签：[mm:ss] / [mm:ss.x] / [mm:ss.xx] / [mm:ss.xxx] / [mm:ss:xx]
 * 小数部分按位数判定单位：1 位 = 十分之一秒，2 位 = 百分之一秒，3 位 = 毫秒。
 */
const timeTagRegex = /\[(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?\]/g;

/** 仅用于「文件里是否存在时间标签」的探测，避免使用带状态的全局正则 */
const hasTimeTagRegex = /\[\d{1,3}:\d{1,2}(?:[.:]\d{1,3})?\]/;

function timeTagToMs(match: RegExpExecArray): number {
	const minutes = parseInt(match[1], 10);
	const seconds = parseInt(match[2], 10);
	const fraction = match[3];

	let milliseconds = 0;
	if (fraction) {
		const value = parseInt(fraction, 10);
		milliseconds =
			fraction.length === 1
				? value * 100
				: fraction.length === 2
					? value * 10
					: value;
	}

	return minutes * 60 * 1000 + seconds * 1000 + milliseconds;
}

/**
 * 按开始时间排序。Array.prototype.sort 是稳定排序，
 * 因此对本来就有序的在线歌词是空操作，只影响乱序的本地文件。
 */
function sortByStartTime(lyrics: ILyricLine[]): void {
	lyrics.sort((a, b) => a.lineStart - b.lineStart);
}

/** 应用 [offset:±ms]：正 offset 表示歌词整体提前，等价于把标签时间往前挪 */
function applyOffset(lyrics: ILyricLine[], offsetMs: number): void {
	if (!offsetMs) return;

	lyrics.forEach((line) => {
		line.lineStart = Math.max(0, line.lineStart - offsetMs);
		if (line.lineEnd != null) {
			line.lineEnd = Math.max(0, line.lineEnd - offsetMs);
		}
		if (line.words) {
			line.words = line.words.map((word) => ({
				...word,
				startTime: Math.max(0, word.startTime - offsetMs),
			}));
		}
	});
}

/**
 * 解析普通 lrc 歌词
 *
 * 相比只认「行首一个 [mm:ss.xx]」的最简实现，这里额外支持本地歌词文件里常见的写法：
 * - 一行多个时间标签（`[00:12.00][00:15.00]同一句`）；
 * - `[mm:ss]` / `[mm:ss.x]` / `[mm:ss.xx]` / `[mm:ss.xxx]` 与 `[mm:ss:xx]`；
 * - `[offset:±ms]` 整体时间轴校正；
 * - 行序不严格递增的文件（解析后统一按时间排序，稳定排序对已有序文件是空操作）。
 * 时间标签必须连续出现在行首，句中出现的 `[00:01]` 不会被误判为标签。
 *
 * @param rawString lrc 歌词字符串
 * @returns 返回解析得到的 LyricLine
 */
export function ParseLyric(rawString: string | undefined) {
	if (!rawString) return null;

	const lines = rawString.split("\n");

	const lyrics: ILyricLine[] = [];
	let offsetMs = 0;

	lines.forEach((line) => {
		const trimmedLine = line.trim();
		if (!trimmedLine) return;

		const offsetMatch = /^\[offset:\s*([+-]?\d+)\s*\]/i.exec(trimmedLine);
		if (offsetMatch) {
			offsetMs = parseInt(offsetMatch[1], 10) || 0;
			return;
		}

		// 标签必须自行首起连续出现，遇到夹在文本里的方括号即停止
		timeTagRegex.lastIndex = 0;
		const times: number[] = [];
		let consumed = 0;
		let tagMatch: RegExpExecArray | null = timeTagRegex.exec(trimmedLine);

		while (tagMatch && tagMatch.index === consumed) {
			times.push(timeTagToMs(tagMatch));
			consumed = timeTagRegex.lastIndex;
			tagMatch = timeTagRegex.exec(trimmedLine);
		}

		if (times.length === 0) return;

		const text = trimmedLine.slice(consumed).trim();

		if (text) {
			// 有歌词文本的行，正常添加（同一行多个时间标签展开为多行）
			const bg = checkIsBgLine(text);
			times.forEach((time) => {
				lyrics.push({
					lineStart: time,
					lineText: bg.text,
					isBG: bg.isBG,
				});
			});
		} else if (lyrics.length > 0) {
			// 空歌词行：[01:40.48]\n —— 标记前一行的结束时间（过门开始）
			lyrics[lyrics.length - 1].lineEnd = times[0];
		}
	});

	if (lyrics.length === 0) return insertLeadDots(lyrics);

	sortByStartTime(lyrics);
	applyOffset(lyrics, offsetMs);

	return insertLeadDots(lyrics);
}

/**
 * 解析逐字歌词
 * @param rawString 逐字歌词字符串
 * @returns 返回解析得到的 LyricLine
 */
export function ParseVerbatimLyric(rawString: string | undefined) {
	if (!rawString) return null;

	const lines = rawString.split("\n");
	const lyrics: ILyricLine[] = [];

	const lineRegex = /^\[(\d+),(\d+)\]/;
	const wordRegex = /\((\d+),(\d+),\d+\)([^()]+)/g;

	lines.forEach((line) => {
		const trimmedLine = line.trim();
		if (!trimmedLine) return;

		const lineMatch = lineRegex.exec(trimmedLine);
		if (lineMatch) {
			const lineStart = parseInt(lineMatch[1]);
			const words: LyricWord[] = [];
			let lineText = "";

			let wordMatch;

			while ((wordMatch = wordRegex.exec(trimmedLine)) !== null) {
				const startTime = parseInt(wordMatch[1]);
				const duration = parseInt(wordMatch[2]);
				const char = wordMatch[3];

				words.push({ startTime, duration, char });
				lineText += char;
			}

			if (words.length > 0) {
				const merged = mergeZeroDuration(words);
				const bg = checkIsBgLine(lineText);
				const finalWords = bg.isBG ? merged.slice(1, -1) : merged;
				if (finalWords.length > 0) {
					// 逐字歌词可直接从最后一个 word 算出行结束时间
					const lastWord = finalWords[finalWords.length - 1];
					const lineEnd = lastWord.startTime + lastWord.duration;
					lyrics.push({
						lineStart,
						lineEnd,
						lineText: bg.text,
						words: finalWords,
						isBG: bg.isBG,
					});
				}
			}
		}
	});

	if (lyrics.length > 0) sortByStartTime(lyrics);

	return insertLeadDots(lyrics);
}

/** 本地歌词文件无法使用时的错误，message 直接面向用户展示 */
export class LocalLyricParseError extends Error {
	constructor(message: string) {
		super(message);
		this.name = "LocalLyricParseError";
	}
}

export interface ParsedLocalLyric {
	lines: ILyricLine[];
	/** 是否为逐字歌词（网易云 yrc 格式），普通 .lrc 一律为 false */
	isVerbatim: boolean;
	/** 解析过程中的非致命提示，用于在界面上补充说明 */
	warnings: string[];
}

/**
 * 逐字歌词的判定：行内有真实可用的逐字时间轴。
 *
 * 不能只看 `words` 是否存在——合并零时长字符后可能出现空数组，
 * 而渲染层是拿 `words.length > 0` 决定要不要走逐字动效的。
 */
export function hasWordTiming(lines: readonly ILyricLine[]): boolean {
	return lines.some((line) => (line.words?.length ?? 0) > 0);
}

/** 是否属于「逐字单位」字符：CJK 汉字与全角标点逐字扫过，拉丁/数字整词扫过 */
const WIDE_GLYPH_REGEX = /[\u2E80-\u9FFF\uF900-\uFAFF\uFF00-\uFFEF\u3000-\u303F]/;

/**
 * 把一整行歌词切成逐字动效的词元。
 *
 * 切分只影响动效粒度，拼接结果必须与原文逐字相同（渲染层直接用词元拼出整行文本）；
 * 空白并入前一个词元，避免扫过时在空格处停顿。
 */
function splitLineIntoTokens(text: string): string[] {
	const tokens: string[] = [];
	let latinBuffer = "";

	const flushLatin = () => {
		if (latinBuffer) {
			tokens.push(latinBuffer);
			latinBuffer = "";
		}
	};

	for (const char of Array.from(text)) {
		if (WIDE_GLYPH_REGEX.test(char)) {
			flushLatin();
			tokens.push(char);
		} else if (/\s/.test(char)) {
			// 空白并入「当前」词元：优先尚未提交的拉丁缓冲（"制作人" + " " + ":" + " " 里的
			// 两个空格必须分别落在自己的位置），否则并入上一个已提交词元，都没有则另起一个。
			// 顺序必须严格保持，词元拼接才能与原文逐字相同。
			if (latinBuffer) {
				latinBuffer += char;
			} else if (tokens.length > 0) {
				tokens[tokens.length - 1] += char;
			} else {
				latinBuffer += char;
			}
		} else {
			latinBuffer += char;
		}
	}

	flushLatin();
	return tokens;
}

/** 中位数；样本不足两笔时返回 null（单笔不可信） */
function medianMs(values: readonly number[]): number | null {
	if (values.length < 2) return null;
	const sorted = [...values].sort((a, b) => a - b);
	const middle = Math.floor(sorted.length / 2);
	return sorted.length % 2 === 1
		? sorted[middle]
		: Math.round((sorted[middle - 1] + sorted[middle]) / 2);
}

/**
 * 兜底跨度的参照值（毫秒）。
 *
 * 主用「本行到下一行行首的间隔」中位数：实测网易云 lrc 的行尾标记极少
 * （186017 是 102 行里只有 5 行有标记、1418812971 一行都没有），
 * 而间隔几乎总是可取且样本多、抗离群；连续演唱的歌词里它与「一句的时长」同量级，
 * 足以把大段间奏这类离群值挑出来。
 * 行数太少取不到两段间隔时，才退回「已知行尾的真跨度」中位数。
 */
function referenceSpanMs(lines: readonly ILyricLine[]): number | null {
	const real = lines.filter((line) => !line.isLeadDots);

	const gaps: number[] = [];
	for (let i = 0; i < real.length - 1; i++) {
		const gap = real[i + 1].lineStart - real[i].lineStart;
		if (gap > 0) gaps.push(gap);
	}
	const gapMedian = medianMs(gaps);
	if (gapMedian !== null) return gapMedian;

	return medianMs(
		real
			.filter(
				(line) =>
					line.lineEnd != null && (line.lineEnd as number) > line.lineStart,
			)
			.map((line) => (line.lineEnd as number) - line.lineStart),
	);
}

/**
 * 为缺少逐字时间的行按行时长生成逐字时间轴（首字落在行首、末字收在行尾）。
 *
 * 背景：逐字歌词（yrc）只覆盖部分歌曲，本地歌词文件也常见只有逐行时间的 .lrc。
 * 这类歌词过去只能整行淡入淡出，与逐字歌词的「逐字扫过」观感不一致；
 * 补一条按行时长均摊的兜底时间轴后，无论歌词来自本地文件、解灰音源还是在线接口，
 * 歌词页都用同一套动效渲染。
 *
 * 跨度取值（直接决定高亮跟不跟得上人声）：
 * - 有行尾标记（网易云 lrc 的过门空时间标签 / 逐字末词）时用标记，它本身就是演唱结束点；
 * - 没有标记时**不能直接拉满到下一行**：那会把中间的伴奏也算成演唱时长，
 *   实测有行被拉到 31s、46s（大段间奏/尾奏），扫过整段慢放，听感就是「跟不上人声」。
 *   这里用同曲参照跨度 ×1.5 封顶，超出部分算作伴奏，扫完即停——
 *   宁可「唱完就停」也不要「整段慢放」，因为后者正是本函数要解决的听感问题。
 *
 * 不修改入参，返回新的行对象（本地歌词的解析结果是 store 里的共享引用，必须保持只读）。
 */
export function applyFallbackWordTiming(
	lines: readonly ILyricLine[],
	fallbackLineDurationMs = 3000,
): ILyricLine[] {
	const referenceSpan = referenceSpanMs(lines);
	const spanCap = referenceSpan ? Math.round(referenceSpan * 1.5) : null;

	return lines.map((line, index) => {
		if (line.isLeadDots) return line;
		if ((line.words?.length ?? 0) > 0) return line;

		const tokens = splitLineIntoTokens(line.lineText);
		if (tokens.length === 0) return line;

		const hasEnd = line.lineEnd != null && line.lineEnd > line.lineStart;
		const nextStart = lines[index + 1]?.lineStart;
		const rawSpan = hasEnd
			? (line.lineEnd as number) - line.lineStart
			: nextStart != null && nextStart > line.lineStart
				? nextStart - line.lineStart
				: fallbackLineDurationMs;
		const span = hasEnd || spanCap === null ? rawSpan : Math.min(rawSpan, spanCap);
		const end = line.lineStart + span;

		const weights = tokens.map((token) => Math.max(1, Array.from(token).length));
		const weightSum = weights.reduce((sum, weight) => sum + weight, 0);

		let cursor = line.lineStart;
		const words: LyricWord[] = tokens.map((char, tokenIndex) => {
			const remaining = end - cursor;
			const isLast = tokenIndex === tokens.length - 1;
			const share = isLast
				? remaining
				: Math.round((span * weights[tokenIndex]) / weightSum);
			const duration = Math.max(1, Math.min(share, remaining));
			const word: LyricWord = { startTime: cursor, duration, char };
			cursor += duration;
			return word;
		});

		const lastWord = words[words.length - 1];
		return {
			...line,
			lineEnd:
				line.lineEnd ??
				(lastWord ? lastWord.startTime + lastWord.duration : end),
			words,
		};
	});
}

/**
 * 把逐字时间轴按「最近行」贴到另一套逐行歌词上。
 *
 * 用于两种歌词并存且只有一方带逐字时间的场景：本地歌词文件是逐行 .lrc，
 * 而在线/解灰歌词带 yrc——此时逐字时间不该被丢掉，否则同一首歌会因为
 * 「歌词来自哪里」而出现两种动效。
 *
 * 时间对齐用容差匹配（两套歌词的行首往往相差几百毫秒）；只有当词元拼出的文本
 * 与目标行完全一致时才采用，避免把另一套歌词的文字带进来。
 */
export function mergeWordTiming(
	base: readonly ILyricLine[],
	timed: readonly ILyricLine[],
	toleranceMs = 1200,
): ILyricLine[] {
	const sources = timed.filter((line) => (line.words?.length ?? 0) > 0);
	if (sources.length === 0) return [...base];

	return base.map((line) => {
		if (line.isLeadDots) return line;
		if ((line.words?.length ?? 0) > 0) return line;

		let matched: ILyricLine | null = null;
		let bestDiff = Number.POSITIVE_INFINITY;
		for (const source of sources) {
			const diff = Math.abs(source.lineStart - line.lineStart);
			if (diff < bestDiff) {
				bestDiff = diff;
				matched = source;
			}
		}

		if (!matched || bestDiff > toleranceMs || !matched.words) return line;

		const joined = matched.words.map((word) => word.char).join("");
		if (joined.trim() !== line.lineText.trim()) return line;

		return { ...line, lineEnd: line.lineEnd ?? matched.lineEnd, words: matched.words };
	});
}

/**
 * 解析用户提供的本地歌词文件。
 *
 * 与 `ParseLyric` 的区别是「面向不可信输入」：本地文件可能是空文件、二进制、
 * 别的编码、别的格式（JSON / 纯文本歌词 / 字幕），这些情况下要让用户看懂问题，
 * 因此统一抛 `LocalLyricParseError` 并携带中文原因。
 */
export function ParseLocalLyric(raw: string): ParsedLocalLyric {
	if (!raw || !raw.trim()) {
		throw new LocalLyricParseError("歌词文件为空，没有可读取的内容");
	}

	// 编码兜底解码后的典型特征：出现 NUL 或大量替换字符
	const replacementCount = (raw.match(/\uFFFD/g) ?? []).length;
	if (raw.includes("\u0000") || replacementCount > raw.length * 0.05) {
		throw new LocalLyricParseError(
			"无法识别歌词文件编码，请将文件另存为 UTF-8 编码后重试",
		);
	}

	const firstContentLine = raw
		.split("\n")
		.map((line) => line.trim())
		.find((line) => line.length > 0);

	if (
		firstContentLine &&
		/^[[{]\s*"/.test(firstContentLine) &&
		!hasTimeTagRegex.test(raw)
	) {
		throw new LocalLyricParseError("该文件不是歌词文件（内容为 JSON 数据）");
	}

	const warnings: string[] = [];
	const offsetMatch = /\[offset:\s*([+-]?\d+)\s*\]/i.exec(raw);
	if (offsetMatch) {
		const offset = parseInt(offsetMatch[1], 10) || 0;
		warnings.push(
			`已按文件中的 offset 标签校正时间轴 ${offset > 0 ? "+" : ""}${offset}ms`,
		);
	}

	const verbatim = ParseVerbatimLyric(raw);
	if (verbatim && verbatim.length > 0) {
		return { lines: verbatim, isVerbatim: true, warnings };
	}

	const plain = ParseLyric(raw);
	if (!plain || plain.length === 0) {
		throw new LocalLyricParseError(
			hasTimeTagRegex.test(raw)
				? "歌词文件里有时间标签，但没有解析出任何歌词文本"
				: "未找到歌词内容：文件里没有 [mm:ss.xx] 形式的时间标签，可能不是 LRC 歌词文件",
		);
	}

	return { lines: plain, isVerbatim: false, warnings };
}
