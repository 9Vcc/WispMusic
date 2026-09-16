/**
 * 桌面歌词「贴边」槽位解析（纯逻辑，与渲染解耦，便于单测）。
 *
 * 规则（用户约定）：
 * - 横向双行：**上行（当前句）恒贴左边、下行（下一句）恒贴右边**；
 * - 竖置双列：**当前句列恒贴左边（列内文本贴顶）、下一句列恒贴右边（列内文本贴底）**。
 *
 * 关键点是「与歌词行号奇偶无关」：旧实现把贴边侧写成 `index % 2`，于是
 * 偶数句当前行贴左、奇数句当前行贴右——用户看到的「上行跑到右边、下行跑到
 * 左边」就是这样来的（贴边不稳定）。贴边只应由**槽位**（当前句 / 下一句）决定。
 */
import type { ILyricLine } from "@/lib/utils/lyric-parser";

/** 贴边侧：start = 左（横排）/ 顶（竖排文字的行内起点）；end = 右 / 底 */
export type LyricEdge = "start" | "end";

/** 槽位 → 贴边侧：当前句恒 start，下一句恒 end */
export function resolveLyricEdge(isCurrent: boolean): LyricEdge {
	return isCurrent ? "start" : "end";
}

export interface SlotItem {
	line: ILyricLine;
	/** 行号：仅用于动画 key 与「本行演唱窗口」语义（过门圆点行会被跳过，故 next 以 currentIndex+1 占位） */
	index: number;
	isCurrent: boolean;
}

/**
 * 竖排双列槽位分配：当前句恒左列、下一句恒右列。
 *
 * 过门圆点行（isLeadDots）不占列——此时两列都为空，由呼吸圆点接管
 * （与横向双行的既有行为一致）。currentIndex < 0（尚未定位到任何句）同理。
 */
export function resolveVerticalSlots({
	line,
	nextLine,
	currentIndex,
}: {
	line?: ILyricLine;
	nextLine?: ILyricLine;
	currentIndex: number;
}): { left?: SlotItem; right?: SlotItem } {
	const current =
		line && !line.isLeadDots && currentIndex >= 0
			? { line, index: currentIndex, isCurrent: true }
			: undefined;
	// 当前句缺失（圆点 / 未定位）时不摆下一句：整页交给呼吸圆点，避免
	// 「上行空、下行占位」这种半截画面
	if (!current) return {};
	const upcoming =
		nextLine && !nextLine.isLeadDots
			? { line: nextLine, index: currentIndex + 1, isCurrent: false }
			: undefined;
	return { left: current, right: upcoming };
}
