import type { Transition } from "framer-motion";

export const LYRIC_TEXT_SPRING: Transition = {
	type: "spring",
	stiffness: 100,
	damping: 20,
} as const;

export const LYRIC_SCROLL_SPRING = {
	type: "spring" as const,
	stiffness: 80,
	damping: 25,
	mass: 1,
	restDelta: 0.01,
};

export const LYRIC_JUMP_SPRING = {
	type: "spring" as const,
	stiffness: 120,
	damping: 20,
	mass: 0.5,
};

/**
 * 逐字遮罩（已唱部分）的跟随弹簧。
 *
 * ⚠️ 这组参数直接决定「逐字高亮比人声慢多少」：`useSpring` 每帧带当前速度重启弹簧动画，
 * 对线性推进的目标来说就是二阶跟踪，**稳态延迟 = 2ζ/ωn（秒），与词长无关**。
 *
 * 旧参数 220/28/1.05（ζ=0.92、ωn=14.5）→ 延迟 ≈ 117~133ms（60fps 逐帧解析解实测），
 * 单个音节往往只有 200~400ms，所以高亮明显落后于人声。
 *
 * 现参数 1800/60/0.5（ζ=1.0 临界阻尼、ωn=60）→ 延迟 ≈ 17~33ms（≈1~2 帧），
 * 已在 60fps 的帧粒度下限附近（再硬也量不出差别），阶跃响应 83ms 到 95%，
 * 因此既跟得上人声、又仍有一点点缓动而不是硬跳。
 * 修改前请先按 2ζ/ωn 估算延迟——ζ 越接近 1 越好（不能过冲，遮罩溢出会显得"唱过头"）。
 */
export const VERBATIM_PROGRESS_SPRING = {
	type: "spring" as const,
	stiffness: 1800,
	damping: 60,
	mass: 0.5,
	restSpeed: 10,
};

export const VERBATIM_TRANSLATE_SPRING = {
	type: "spring" as const,
	stiffness: 120,
	damping: 22,
	mass: 1,
};

export const INSTANT_TWEEN: Transition = {
	type: "tween",
	duration: 0,
	ease: "linear",
} as const;

export const LAYOUT_TRANSITION = { ...LYRIC_SCROLL_SPRING };

/**
 * 歌词偏移（对齐）常量与工具已移至 `@/lib/constants/lyric`
 * （偏移属于歌词领域配置，避免 lib 反向依赖 components）。
 */

/**
 * 根据当前滚动状态返回 Y 轴过渡配置
 */
export function getScrollYTransition(opts: {
	isLayoutChanging: boolean;
	isScrolling: boolean;
	isLargeJump: boolean;
	scrollDelay: number;
}): Transition {
	if (opts.isLayoutChanging) {
		return { ...LYRIC_SCROLL_SPRING, delay: 0 };
	}
	if (opts.isScrolling) {
		return INSTANT_TWEEN;
	}
	if (opts.isLargeJump) {
		return { ...LYRIC_JUMP_SPRING, delay: 0 };
	}
	return { ...LYRIC_SCROLL_SPRING, delay: opts.scrollDelay };
}

/**
 * @LyricAnimations
 *
 * 致谢:
 * 缓动函数参考自以下开源项目：
 *
 * - 项目名称: applemusic-like-lyrics
 * - 原作者: amll-dev
 * - 项目地址: https://github.com/amll-dev/applemusic-like-lyrics
 *
 * - 特此向原作者表示感谢。
 */

export function easeOutExpo(x: number): number {
	return x >= 1 ? 1 : 1 - 2 ** (-10 * x);
}
export function easeInOutBack(x: number): number {
	const c1 = 1.70158;
	const c2 = c1 * 1.525;
	return x < 0.5
		? ((2 * x) ** 2 * ((c2 + 1) * 2 * x - c2)) / 2
		: ((2 * x - 2) ** 2 * ((c2 + 1) * (x * 2 - 2) + c2) + 2) / 2;
}
