/**
 * 歌词偏移（对齐）常量，参考 EchoMusic 的 timeOffsetMap 设计。
 *
 * 正值 = 歌词整体提前（修「逐字高亮落后于人声」）；负值 = 歌词延后。
 * 偏移按歌曲 id 记忆并持久化（不同歌曲/音源的时间轴差异是逐首歌的）。
 */

/** 偏移上限（毫秒）：参考 EchoMusic 的 ±10s，足以覆盖不同母带的前奏差异 */
export const LYRIC_OFFSET_MAX_MS = 10000;

/** 步进按钮的步长（毫秒）：EchoMusic 默认 0.5s */
export const LYRIC_OFFSET_NUDGE_MS = 500;

/** 滑块的步长（毫秒）：比按钮更细，便于精调 */
export const LYRIC_OFFSET_SLIDE_STEP_MS = 100;

export const DEFAULT_LYRIC_OFFSET_MS = 0;

/** 夹在 ±上限内 */
export function clampLyricOffset(offsetMs: number): number {
	if (!Number.isFinite(offsetMs)) return DEFAULT_LYRIC_OFFSET_MS;
	return Math.min(
		LYRIC_OFFSET_MAX_MS,
		Math.max(-LYRIC_OFFSET_MAX_MS, Math.round(offsetMs)),
	);
}

/** 偏移的展示文案（EchoMusic 同款）：+1.5s / -0.5s / +0.0s */
export function formatLyricOffset(offsetMs: number): string {
	const sign = offsetMs >= 0 ? "+" : "-";
	return `${sign}${(Math.abs(offsetMs) / 1000).toFixed(1)}s`;
}
