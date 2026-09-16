/**
 * 桌面歌词「配对轮换」的纯判定逻辑（两种动效共用）。
 *
 * 配对 = 连续两句歌词 (first, first+1)。播放语义（横向双行 / 竖排双列一致）：
 * - 配对固定挂载：**第一句完整播放结束后再播放第二句**（两格都在自己的演唱
 *   窗口内做逐字/笔画动画），当前句在配对内推进时不换对；
 * - 只有当前句推进到配对之外（第二句也播完）才整对轮换：先整对退出（散开 /
 *   滑出），再换进以下一句为首的新配对。
 *
 * 与旧实现的关键差别：旧行为是「每换一句就滚动一格」（当前句晋级、下一句从
 * 底部滑入的连续滚动），现改为按对翻页，两格轮流播完再换对。
 */

/** 行号是否落在配对 (first, first+1) 内 */
export function isIndexInPair(
	currentIndex: number,
	pairFirst: number,
): boolean {
	if (pairFirst < 0) return false;
	return currentIndex === pairFirst || currentIndex === pairFirst + 1;
}

/** 配对槽位：first=配对首句（先播）、second=配对次句（后播），越界为 undefined */
export function resolvePairSlots({
	first,
	lineCount,
}: {
	first: number;
	lineCount: number;
}): { first?: number; second?: number } {
	if (first < 0 || first >= lineCount) return {};
	return {
		first,
		second: first + 1 < lineCount ? first + 1 : undefined,
	};
}
