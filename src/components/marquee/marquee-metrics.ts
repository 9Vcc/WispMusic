/**
 * Marquee 的纯逻辑（与组件分开，便于单测，无需 JSX/CSS 依赖）。
 *
 * 溢出判定只接受**与动画状态无关**的两个量：文本自然宽度、容器可用宽度。
 * 历史实现用「轨道内文本的 scrollWidth vs 容器宽度」，而轨道在滚动时会额外
 * 挂一份副本文案，副本会把容器的 flex 基准宽度（max-content）翻倍，于是：
 * 溢出 → 挂副本 → 容器变宽 → 判定为不溢出 → 摘副本 → 容器变窄 → 又溢出……
 * 实测在 1280 宽窗口、8 字歌名 + 12 字歌手串的组合下以约 19Hz 来回翻转，
 * 歌名与右侧歌手列随之高频横移（用户看到的闪动/抖动）。
 * 现在副本绝对定位（不参与布局）、文本宽度由独立的量算节点测得，两个输入
 * 都不再受动画状态影响，环路被切断。
 */

/** 巡航滚动速度 px/s */
export const MARQUEE_SPEED = 50;
/** 每组滚动结束后在起点停留的时间 */
export const MARQUEE_HOLD_MS = 3000;
/**
 * 一组连续滚 3 轮（轮数硬编码在 marquee.module.css 的关键帧里）。
 * 总时长 = 3 轮巡航 + 首尾缓入缓出的额外开销，系数与 CSS 中的推导对应。
 */
export const MARQUEE_GROUP_DURATION_FACTOR = 3.192;
/**
 * 循环间距：一份文本与下一份之间的留白。恒定保留（不随滚动状态增减），
 * 这样轨道宽 = 单份文本 + 间距 恒定，位移基准与滚动时长都以它为准。
 */
export const MARQUEE_LOOP_GAP_PX = 32;
/**
 * 溢出判定容差：文本与可用宽度差在 1px 内视为「放得下」。
 * 贴边文本（差值 <1px）在窗口尺寸微调时会在两态间反复翻转，
 * 容差把这种边界抖动压掉。
 */
export const MARQUEE_OVERFLOW_TOLERANCE_PX = 1;

export interface MarqueeState {
	shouldAnimate: boolean;
	/** 滚动一组的时长（秒） */
	duration: number;
}

export function computeMarqueeState({
	textWidth,
	availableWidth,
	reducedMotion,
	loopGapPx = 0,
}: {
	/** 单份文本的自然宽度（量算节点实测） */
	textWidth: number;
	/** 容器可用宽度 */
	availableWidth: number;
	reducedMotion: boolean;
	/** 循环间距：每轮实际位移 = 文本宽 + 间距，时长按它换算才对准巡航速度 */
	loopGapPx?: number;
}): MarqueeState {
	const isOverflowing =
		textWidth > availableWidth + MARQUEE_OVERFLOW_TOLERANCE_PX && textWidth > 0;
	return {
		shouldAnimate: isOverflowing && !reducedMotion,
		duration:
			((textWidth + Math.max(0, loopGapPx)) / MARQUEE_SPEED) *
			MARQUEE_GROUP_DURATION_FACTOR,
	};
}
