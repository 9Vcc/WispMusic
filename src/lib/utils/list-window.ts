/**
 * 视口窗口化：只渲染滚动视口内（外加 overscan）的行。
 *
 * 用于「列表可能上千条、但一次只看得见十几行」的场景（迷你播放器播放列表）：
 * 不窗口化时上千首歌就是上千个 DOM 子树，而且折叠态也一直挂着。
 *
 * start/end 用于 slice，topPad/bottomPad 是两侧占位高度。三者满足
 * `topPad + (end - start) × itemHeight + bottomPad === total × itemHeight`，
 * 即滚动内容总高与不窗口化时完全一致，滚动条长度与滚动位置都不受影响
 * —— 前提是每一行都固定行高（占位高度必须与真实行高一致）。
 */
export interface ListWindow {
	start: number;
	end: number;
	topPad: number;
	bottomPad: number;
}

export function computeListWindow(options: {
	total: number;
	itemHeight: number;
	scrollTop: number;
	viewportHeight: number;
	overscan?: number;
}): ListWindow {
	const { total, itemHeight } = options;
	const overscan = Math.max(0, options.overscan ?? 0);

	if (total <= 0 || itemHeight <= 0) {
		return { start: 0, end: 0, topPad: 0, bottomPad: 0 };
	}

	const scrollTop = Number.isFinite(options.scrollTop)
		? Math.max(0, options.scrollTop)
		: 0;
	const viewportHeight = Number.isFinite(options.viewportHeight)
		? Math.max(0, options.viewportHeight)
		: 0;

	const firstVisible = Math.min(
		Math.floor(scrollTop / itemHeight),
		total - 1,
	);
	const visibleCount = Math.ceil(viewportHeight / itemHeight) + 1;

	const start = Math.max(firstVisible - overscan, 0);
	const end = Math.min(
		Math.max(firstVisible + visibleCount + overscan, start + 1),
		total,
	);

	return {
		start,
		end,
		topPad: start * itemHeight,
		bottomPad: (total - end) * itemHeight,
	};
}
