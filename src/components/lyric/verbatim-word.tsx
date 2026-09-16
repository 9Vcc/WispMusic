/**
 * @VerbatimWord
 *
 * 致谢:
 * 本文件中的动画曲线与动画参数参考自以下开源项目：
 *
 * - 项目名称: applemusic-like-lyrics
 * - 原作者: amll-dev
 * - 项目地址: https://github.com/amll-dev/applemusic-like-lyrics
 *
 * - 特此向原作者表示感谢。
 */

import {
	type MotionValue,
	motion,
	useMotionTemplate,
	useSpring,
	useTransform,
} from "framer-motion";
import React, { useEffect, useMemo, useRef } from "react";
import type { LyricWord } from "@/lib/utils/lyric-parser";
import {
	VERBATIM_PROGRESS_SPRING,
	VERBATIM_TRANSLATE_SPRING,
} from "./lyric-animation";

/**
 * 逐字填充基色：**基色不透明**，明暗层级全部由遮罩 alpha 表达（已唱 0.85 /
 * 未唱 0.32，随进度插值）。默认纯白即歌词页的既有外观；桌面歌词窗口会注入
 * --dl-word-fill-color 跟随用户配色（该窗口无 plus-lighter 链路，见
 * desktop-lyric-colors.ts）。
 */
const WORD_FILL_COLOR = "var(--dl-word-fill-color, rgba(255,255,255,1))";

const clamp01 = (value: number) => Math.min(1, Math.max(0, value));
const isCJK = (text: string) =>
	/^[\p{Unified_Ideograph}\u0800-\u9FFC]+$/u.test(text);

const ANIMATION_FRAME_QUANTITY = 32;
/**
 * 强调动画（弹跳/发光/浮动）的速度阈值：单字演唱时长（逐字时间间隔）达到该值才弹。
 *
 * 旧值 760ms 只剩慢歌拖长音才命中——中文流行歌单字演唱普遍 150~500ms，
 * 中快速歌词整行只有扫色没有弹跳，看起来就是「该播动画的歌词没播」。
 * 现值 500ms ≈ 120 字/分钟：一字一拍（90~120 BPM）的重音都能触发；
 * 急促连唱（<500ms/字）仍保持克制，避免满屏都在弹、强调失去意义。
 */
const EMPHASIS_TRIGGER_DURATION = 500;
const EMPHASIS_VISIBLE_DURATION = 1300;

/**
 * 强调动画的触发判定（导出仅供测试）：
 * - CJK 词元按单字演唱时长判定；
 * - 拉丁词元额外要求 2~7 个字符——单字符多为标注、超长多为串词，都不弹。
 */
export function shouldEmphasizeWord(char: string, durationMs: number): boolean {
	const text = char.trim();
	return isCJK(text)
		? durationMs >= EMPHASIS_TRIGGER_DURATION
		: durationMs >= EMPHASIS_TRIGGER_DURATION &&
				text.length <= 7 &&
				text.length > 1;
}

function cubicBezier(x1: number, y1: number, x2: number, y2: number) {
	const cx = 3 * x1;
	const bx = 3 * (x2 - x1) - cx;
	const ax = 1 - cx - bx;
	const cy = 3 * y1;
	const by = 3 * (y2 - y1) - cy;
	const ay = 1 - cy - by;

	const sampleX = (t: number) => ((ax * t + bx) * t + cx) * t;
	const sampleY = (t: number) => ((ay * t + by) * t + cy) * t;
	const sampleXDerivative = (t: number) => (3 * ax * t + 2 * bx) * t + cx;

	return (x: number) => {
		const target = clamp01(x);
		let t = target;

		for (let i = 0; i < 4; i++) {
			const dx = sampleX(t) - target;
			const derivative = sampleXDerivative(t);
			if (Math.abs(dx) < 0.001 || Math.abs(derivative) < 0.001) break;
			t -= dx / derivative;
		}

		let lo = 0;
		let hi = 1;
		t = clamp01(t);
		for (let i = 0; i < 6; i++) {
			const dx = sampleX(t) - target;
			if (Math.abs(dx) < 0.001) break;
			if (dx > 0) hi = t;
			else lo = t;
			t = (lo + hi) / 2;
		}

		return sampleY(t);
	};
}

const bezIn = cubicBezier(0.2, 0.4, 0.58, 1);
const bezOut = cubicBezier(0.3, 0, 0.58, 1);

const matrixScale = (scale: number) => {
	const s = scale.toFixed(6);
	return `matrix3d(${s},0,0,0,0,${s},0,0,0,0,1,0,0,0,0,1)`;
};

const emphasizeEase = (value: number) => {
	const t = clamp01(value);
	return t < 0.5 ? bezIn(t / 0.5) : 1 - bezOut((t - 0.5) / 0.5);
};

const getEmphasisMetrics = (duration: number) => {
	const safeDuration = Math.max(EMPHASIS_VISIBLE_DURATION, duration);
	let amount = safeDuration / 2000;
	amount = amount > 1 ? Math.sqrt(amount) : amount ** 3;
	let blur = safeDuration / 3000;
	blur = blur > 1 ? Math.sqrt(blur) : blur ** 3;

	return {
		amount: Math.min(1.2, amount * 0.6),
		blur: Math.min(0.8, blur * 0.5),
	};
};

const getCharDelay = (duration: number, idx: number, totalChars: number) =>
	(duration / 2.5 / Math.max(totalChars, 1)) * idx;

function makeEmphasisFrames(
	idx: number,
	totalChars: number,
	amount: number,
	blur: number,
): Keyframe[] {
	const frames: Keyframe[] = [];
	for (let i = 0; i <= ANIMATION_FRAME_QUANTITY; i++) {
		const progress = i / ANIMATION_FRAME_QUANTITY;
		const pulse = emphasizeEase(progress);
		const scale = 1 + pulse * 0.1 * amount;
		const offsetX = -pulse * 0.03 * amount * (totalChars / 2 - idx);
		const offsetY = -pulse * 0.025 * amount;
		const glowLevel = pulse * blur;

		frames.push({
			offset: progress,
			transform: `${matrixScale(scale)} translate(${offsetX.toFixed(5)}em, ${offsetY.toFixed(5)}em)`,
			textShadow: `0 0 ${Math.min(0.3, blur * 0.3).toFixed(5)}em rgba(255, 255, 255, calc(${glowLevel.toFixed(5)} * var(--lyric-alpha-scale, 1)))`,
		});
	}
	return frames;
}

function makeFloatFrames(): Keyframe[] {
	const frames: Keyframe[] = [];
	for (let i = 0; i <= ANIMATION_FRAME_QUANTITY; i++) {
		const progress = i / ANIMATION_FRAME_QUANTITY;
		const y = Math.sin(progress * Math.PI);
		frames.push({
			offset: progress,
			transform: `translateY(${(-y * 0.05).toFixed(5)}em)`,
		});
	}
	return frames;
}

export const VerbatimWord = React.memo(function VerbatimWord({
	word,
	currentTimeMotion,
	orientation = "horizontal",
}: {
	word: LyricWord;
	currentTimeMotion: MotionValue<number>;
	/** 排版方向：horizontal=横排（填充从左到右）；vertical=竖排直书（填充从上到下） */
	orientation?: "horizontal" | "vertical";
}) {
	const vertical = orientation === "vertical";
	const emphasisMetrics = React.useMemo(() => {
		const { duration, char } = word;
		return shouldEmphasizeWord(char, duration)
			? getEmphasisMetrics(duration)
			: { amount: 0, blur: 0 };
	}, [word]);

	const rawProgress = useTransform(
		currentTimeMotion,
		[word.startTime, word.startTime + word.duration],
		[0, 1],
	);

	const progress = useSpring(rawProgress, { ...VERBATIM_PROGRESS_SPRING });
	const brightStop = useTransform(progress, (p) => `${p * 100}%`);
	const fadeStop = useTransform(
		progress,
		(p) => `${Math.min(116, p * 100 + 22)}%`,
	);
	// 基色为不透明白，亮度全部由遮罩 alpha 控制：
	// 已播放段偏白但留一点余量，避免 plus-lighter 叠加背景后过曝
	const brightAlpha = useTransform(progress, [0, 0.04, 1], [0.32, 0.85, 0.85]);
	const fadeAlpha = useTransform(progress, [0, 0.04, 1], [0.32, 0.5, 0.5]);

	const TRANSLATE_DURATION = 600;
	const rawTranslateProgress = useTransform(
		currentTimeMotion,
		[word.startTime, word.startTime + TRANSLATE_DURATION],
		[0, 1],
	);
	const translateProgress = useSpring(rawTranslateProgress, {
		...VERBATIM_TRANSLATE_SPRING,
	});
	// 上浮沿行内方向：横排向上（-y），竖排向左（-x，即朝读者方向挑起）
	const translateY = useTransform(
		translateProgress,
		[0, 1],
		vertical ? ["0em", "0em"] : ["0em", "-0.05em"],
	);
	const translateX = useTransform(
		translateProgress,
		[0, 1],
		vertical ? ["0em", "-0.05em"] : ["0em", "0em"],
	);

	// --lyric-alpha-scale（由歌词页根据背景亮度设置）统一压低遮罩 alpha，
	// 避免亮色背景下 plus-lighter 加法混合把已播放段顶到裁切过曝
	const wordMaskImage = useMotionTemplate`linear-gradient(${vertical ? "180deg" : "90deg"},
      rgba(0,0,0,calc(${brightAlpha} * var(--lyric-alpha-scale, 1))) 0%,
      rgba(0,0,0,calc(${brightAlpha} * var(--lyric-alpha-scale, 1))) ${brightStop},
      rgba(0,0,0,calc(${fadeAlpha} * var(--lyric-alpha-scale, 1))) ${fadeStop},
      rgba(0,0,0,calc(0.32 * var(--lyric-alpha-scale, 1))) 100%
    )`;

	const chars = useMemo(() => {
		let signature = 0;
		return Array.from(word.char).map((char) => {
			signature += char.codePointAt(0) ?? 0;
			return {
				char,
				key: `${word.startTime}-${word.duration}-${signature}-${char}`,
			};
		});
	}, [word.char, word.duration, word.startTime]);
	const needsPerChar = emphasisMetrics.amount > 0;
	const emphasisDuration = Math.max(1000, word.duration);

	if (!needsPerChar) {
		return (
			<motion.span
				style={{
					display: "inline-block",
					whiteSpace: "pre",
					margin: "-0.25em",
					padding: "0.25em",
					x: translateX,
					y: translateY,
					color: WORD_FILL_COLOR,
					WebkitMaskImage: wordMaskImage,
					maskImage: wordMaskImage,
					WebkitMaskRepeat: "no-repeat",
					maskRepeat: "no-repeat",
					WebkitMaskSize: "100% 100%",
					maskSize: "100% 100%",
					willChange: "transform, text-shadow, mask-image",
				}}
			>
				{word.char}
			</motion.span>
		);
	}

	return (
		<motion.span
			style={{
				display: "inline-block",
				whiteSpace: "pre",
				margin: "-0.25em",
				padding: "0.25em",
				x: translateX,
				y: translateY,
				color: WORD_FILL_COLOR,
				WebkitMaskImage: wordMaskImage,
				maskImage: wordMaskImage,
				WebkitMaskRepeat: "no-repeat",
				maskRepeat: "no-repeat",
				WebkitMaskSize: "100% 100%",
				maskSize: "100% 100%",
				willChange: "transform, text-shadow, mask-image",
			}}
		>
			{chars.map(({ char, key }, idx) => (
				<WavyChar
					key={key}
					char={char}
					idx={idx}
					currentTimeMotion={currentTimeMotion}
					wordStartTime={word.startTime}
					duration={emphasisDuration}
					amount={emphasisMetrics.amount}
					blur={emphasisMetrics.blur}
					totalChars={chars.length}
				/>
			))}
		</motion.span>
	);
});

const WavyChar = React.memo(function WavyChar({
	char,
	idx,
	currentTimeMotion,
	wordStartTime,
	duration,
	amount,
	blur,
	totalChars,
}: {
	char: string;
	idx: number;
	currentTimeMotion: MotionValue<number>;
	wordStartTime: number;
	duration: number;
	amount: number;
	blur: number;
	totalChars: number;
}) {
	const elementRef = useRef<HTMLSpanElement | null>(null);
	const delay = getCharDelay(duration, idx, totalChars);

	useEffect(() => {
		const element = elementRef.current;
		if (!element || duration <= 0) return;

		const emphasis = element.animate(
			makeEmphasisFrames(idx, totalChars, amount, blur),
			{
				duration,
				delay,
				fill: "both",
				composite: "replace",
				iterations: 1,
			},
		);
		const float = element.animate(makeFloatFrames(), {
			duration: duration * 1.4,
			delay: delay - 400,
			fill: "both",
			composite: "add",
			iterations: 1,
		});
		const animations = [emphasis, float];

		for (const animation of animations) {
			animation.pause();
		}

		const syncAnimationTime = (time: number) => {
			const relativeTime = Math.max(0, time - wordStartTime);
			const endTime = Math.max(delay + duration, delay - 400 + duration * 1.4);
			const animationTime = Math.min(relativeTime, Math.max(0, endTime));

			for (const animation of animations) {
				animation.currentTime = animationTime;
			}
		};

		syncAnimationTime(currentTimeMotion.get());
		const unsubscribe = currentTimeMotion.on("change", syncAnimationTime);

		return () => {
			unsubscribe();
			for (const animation of animations) {
				animation.cancel();
			}
		};
	}, [
		amount,
		blur,
		currentTimeMotion,
		delay,
		duration,
		idx,
		totalChars,
		wordStartTime,
	]);

	return (
		<span
			className="transform-gpu"
			ref={elementRef}
			style={{
				display: "inline-block",
				transformOrigin: "50% 78%",
				backfaceVisibility: "hidden",
				WebkitFontSmoothing: "antialiased",
				textRendering: "geometricPrecision",
				willChange: "transform",
			}}
		>
			{char}
		</span>
	);
});
