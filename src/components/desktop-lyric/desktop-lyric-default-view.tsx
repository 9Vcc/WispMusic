import {
	AnimatePresence,
	type MotionValue,
	motion,
	type Transition,
	useTransform,
} from "framer-motion";
import type { CSSProperties } from "react";
import { useLayoutEffect, useRef, useState } from "react";
import { VerbatimWord } from "@/components/lyric/verbatim-word";
import { cn } from "@/lib/utils";
import {
	DESKTOP_LYRIC_ALPHA,
	type DesktopLyricColors,
	withAlpha,
} from "@/lib/utils/desktop-lyric-colors";
import {
	DESKTOP_LYRIC_EDGE_INSET,
	DESKTOP_LYRIC_VERTICAL_SAFE_INSET,
} from "@/lib/utils/desktop-lyric-geometry";
import type { ILyricLine } from "@/lib/utils/lyric-parser";
import { useLyricPair } from "./use-lyric-pair";

/**
 * 桌面歌词「默认动效」视图。
 *
 * 播放模型与笔画动效**完全一致**（配对轮换，见 use-lyric-pair.ts）：
 * 配对 = 连续两句歌词，配对固定挂载——**第一句完整播放结束后再播放第二句**
 * （两格各自在自己的演唱窗口内做逐字填充），当前句在配对内推进不换对；
 * 只有两句都播完、当前句推进到配对之外才整对轮换（先整对滑出、再换进下两句）。
 * 旧行为是「每换一句就滚动一格」（当前句晋级、下一句从底部滑入的连续滚动），
 * 已废弃。
 *
 * 格位与贴边（两种方向同构）：
 * - 横向：上行 = 配对首句（贴左）、下行 = 配对次句（贴右）；
 * - 竖向：左列 = 配对首句（贴左 + 列内贴顶）、右列 = 配对次句（贴右 +
 *   列内贴底，避让工具条）。
 * 正在演唱的那一格用高亮色、另一格用普通文字色（同亮度同字号，与笔画动效
 * 一致）；未唱到的字仍按逐字遮罩保持暗态，已唱过的字保持亮态。
 *
 * 与歌词页的关键差异：桌面歌词浮在任意壁纸上，plus-lighter 叠加不可用，
 * 可读性由独立的暗色描边背衬层保证——背衬层与填充层同文本同布局完全
 * 重叠，永远完整描出整行轮廓，填充层（含逐字遮罩）叠在其上。
 */

export type DesktopLyricOrientation = "horizontal" | "vertical";

interface DesktopDefaultViewProps {
	/** 歌词全文（配对由视图内部按当前句索引推导，与笔画动效同一套语义） */
	lines: ILyricLine[];
	/** 当前演唱句索引 */
	currentIndex: number;
	direction: DesktopLyricOrientation;
	fontSize: number;
	/** 用户配色：正在演唱的格用高亮色、另一格用普通文字色 */
	colors: DesktopLyricColors;
	currentTimeMotion: MotionValue<number>;
}

/** 单行槽位高度（与 lineHeight 1.35 一致，钳制超高字符防溢出对侧行） */
const LINE_SLOT_RATIO = 1.35;
/** 格位进出场的滑动时长与缓动（EchoMusic lyric-slide 同参数） */
const SLOT_SLIDE: Transition = { duration: 0.22, ease: [0.22, 1, 0.36, 1] };
/** 整对退出时长：与滑动时长对齐，退出结束即换进新配对 */
const PAIR_EXIT_MS = 220;

export function DesktopDefaultView({
	lines,
	currentIndex,
	direction,
	fontSize,
	colors,
	currentTimeMotion,
}: DesktopDefaultViewProps) {
	const isVertical = direction === "vertical";
	// 正在演唱 / 另一格的行色：同亮度、只换色相（与笔画动效的「两格同亮」一致；
	// 默认白 = 改造前的 rgb(255 255 255 / 0.88)）
	const singingLineColor = withAlpha(
		colors.highlightColor,
		DESKTOP_LYRIC_ALPHA.current,
	);
	const idleLineColor = withAlpha(
		colors.textColor,
		DESKTOP_LYRIC_ALPHA.current,
	);

	const { pairFirst } = useLyricPair({
		currentIndex,
		exitMs: PAIR_EXIT_MS,
	});
	const secondIndex =
		pairFirst >= 0 && pairFirst + 1 < lines.length ? pairFirst + 1 : undefined;
	const firstLine = pairFirst >= 0 ? lines[pairFirst] : undefined;
	const secondLine = secondIndex === undefined ? undefined : lines[secondIndex];
	const hasPair = Boolean(firstLine || secondLine);
	// 本句演唱窗口（到下一句切句点为止；末句兜底），供溢出滚动用
	const windowMsOf = (index: number | undefined) =>
		index === undefined
			? 10000
			: Math.max(
					600,
					(lines[index + 1]?.lineStart ?? lines[index].lineStart + 6000) -
						lines[index].lineStart,
				);

	if (isVertical) {
		const columnSlide = fontSize * 1.35;
		const renderColumn = (
			index: number | undefined,
			side: "left" | "right",
		) => {
			const isLeft = side === "left";
			const item = index === undefined ? undefined : lines[index];
			const isSinging = index !== undefined && index === currentIndex;
			return (
				<div
					key={side}
					className={cn(
						"relative flex min-w-0 flex-1 overflow-hidden",
						isLeft ? "items-start justify-start" : "items-end justify-end",
					)}
				>
					<AnimatePresence mode="popLayout" initial={false}>
						{item && (
							<motion.div
								key={`col-${index}`}
								initial={{
									opacity: 0,
									x: isLeft ? -columnSlide : columnSlide,
								}}
								animate={{ opacity: 1, x: 0 }}
								exit={{
									opacity: 0,
									x: isLeft ? -columnSlide : columnSlide,
								}}
								transition={SLOT_SLIDE}
								className="max-h-full max-w-full"
								style={{
									writingMode: "vertical-rl",
									color: isSinging ? singingLineColor : idleLineColor,
								}}
							>
								{item.isLeadDots ? (
									<DesktopLeadDots vertical />
								) : (
									<DesktopLineText
										line={item}
										direction={direction}
										fontSize={fontSize}
										currentTimeMotion={currentTimeMotion}
										lineWindowMs={windowMsOf(index)}
									/>
								)}
							</motion.div>
						)}
					</AnimatePresence>
				</div>
			);
		};
		return (
			<div
				className="flex h-full w-full items-stretch overflow-hidden"
				style={{
					fontFamily: "var(--app-lyric-font-family, inherit)",
					color: singingLineColor,
					paddingTop: DESKTOP_LYRIC_EDGE_INSET / 2,
					paddingBottom: DESKTOP_LYRIC_EDGE_INSET / 2,
					paddingLeft: DESKTOP_LYRIC_EDGE_INSET / 2,
					paddingRight: DESKTOP_LYRIC_VERTICAL_SAFE_INSET,
				}}
			>
				{!hasPair ? (
					<div className="flex h-full w-full items-center justify-center">
						<DesktopLeadDots vertical />
					</div>
				) : (
					<>
						{renderColumn(pairFirst >= 0 ? pairFirst : undefined, "left")}
						{renderColumn(secondIndex, "right")}
					</>
				)}
			</div>
		);
	}

	// 横向双行：上行 = 配对首句（贴左）、下行 = 配对次句（贴右）。两行的 y
	// 恒定（不再有「晋级上滑」），整对轮换时整对滑出、新对从底部滑入——
	// 与笔画动效的「按对翻页」一致。行槽块恒定挂载（含呼吸圆点兜底），块
	// 自身 overflow-hidden，进出场滑动在块边缘裁切；左右各留一点边距，
	// 避免字形描边被窗口裁掉。
	const slotH = fontSize * LINE_SLOT_RATIO;
	const rows = [
		{
			index: pairFirst >= 0 ? pairFirst : undefined,
			line: firstLine,
			edge: "start" as const,
		},
		{ index: secondIndex, line: secondLine, edge: "end" as const },
	].filter((row) => row.index !== undefined && row.line);

	return (
		<div
			className="flex h-full w-full items-center justify-center overflow-hidden"
			style={{
				fontFamily: "var(--app-lyric-font-family, inherit)",
				paddingLeft: DESKTOP_LYRIC_EDGE_INSET,
				paddingRight: DESKTOP_LYRIC_EDGE_INSET,
			}}
		>
			<div
				className="relative w-full overflow-hidden"
				style={{ height: slotH * 2 }}
			>
				{!hasPair && (
					<div className="absolute inset-0 flex items-center justify-center">
						<DesktopLeadDots vertical={false} />
					</div>
				)}
				<AnimatePresence initial={false}>
					{rows.map(({ index, line, edge }) => {
						if (index === undefined || !line) return null;
						const isSinging = index === currentIndex;
						return (
							<motion.div
								key={`line-${index}`}
								initial={{ opacity: 0, y: slotH * 2 }}
								animate={{
									opacity: 1,
									y: edge === "start" ? 0 : slotH,
									color: isSinging ? singingLineColor : idleLineColor,
								}}
								exit={{ opacity: 0, y: -slotH }}
								transition={{
									...SLOT_SLIDE,
									opacity: { duration: 0.18, ease: "easeOut" },
								}}
								className={cn(
									"absolute inset-x-0 top-0 flex items-center overflow-hidden",
									edge === "start" ? "justify-start" : "justify-end",
								)}
								style={{ height: slotH }}
							>
								<span className="relative flex max-w-full items-center">
									{line.isLeadDots ? (
										<DesktopLeadDots vertical={false} />
									) : (
										<DesktopLineText
											line={line}
											direction={direction}
											fontSize={fontSize}
											currentTimeMotion={currentTimeMotion}
											lineWindowMs={windowMsOf(index)}
										/>
									)}
								</span>
							</motion.div>
						);
					})}
				</AnimatePresence>
			</div>
		</div>
	);
}

/** 单行渲染：暗色描边背衬层 + 逐字填充层（两层同文本同布局完全重叠） */
function DesktopLineText({
	line,
	direction,
	fontSize,
	currentTimeMotion,
	lineWindowMs,
}: {
	line: ILyricLine;
	direction: DesktopLyricOrientation;
	fontSize: number;
	currentTimeMotion: MotionValue<number>;
	/** 本行的演唱窗口（下一句 lineStart - 本句 lineStart），无逐字数据时作滚动时窗兜底 */
	lineWindowMs: number;
}) {
	const isVertical = direction === "vertical";
	// 横排单行不换行（窗口高度按字号给定，换行必然溢出）；
	// 竖排靠窗口高度自然折列，textAlign center 在竖排下沿行内方向（纵向）居中
	const textStyle: CSSProperties = {
		fontSize,
		fontWeight: 700,
		lineHeight: LINE_SLOT_RATIO,
		fontFamily: "var(--app-lyric-font-family, inherit)",
		fontSynthesis: "none",
		WebkitFontSmoothing: "antialiased",
		whiteSpace: isVertical ? "normal" : "nowrap",
		textAlign: isVertical ? "center" : undefined,
	};

	const hasWords = Boolean(line.words && line.words.length > 0);

	// —— 超宽句横向滚动（仅横排）：溢出量实测，滚动窗口对齐 EchoMusic
	//（30% 演唱进度起步、句尾前 2s 收住；无逐字数据按行窗口的 80% 兜底）。
	// 配对两格都跟播，因此两行都做溢出测量（未到它的窗口时位移恒为 0）
	const outerRef = useRef<HTMLSpanElement>(null);
	const innerRef = useRef<HTMLSpanElement>(null);
	const [overflowPx, setOverflowPx] = useState(0);
	useLayoutEffect(() => {
		if (isVertical) return;
		const outer = outerRef.current;
		const inner = innerRef.current;
		if (!outer || !inner) return;
		const measure = () =>
			setOverflowPx(Math.max(0, inner.offsetWidth - outer.offsetWidth));
		measure();
		// 外层随窗口缩放变化、内层随字号/文本/字体排版变化，都触发重测
		//（transform 滚动不改排版盒，不会误触发）
		const ro = new ResizeObserver(measure);
		ro.observe(outer);
		ro.observe(inner);
		return () => ro.disconnect();
	}, [isVertical]);

	const lastWord =
		line.words && line.words.length > 0
			? line.words[line.words.length - 1]
			: undefined;
	const naturalEndMs = lastWord
		? lastWord.startTime + lastWord.duration
		: line.lineStart + Math.max(2000, lineWindowMs * 0.8);
	const scrollEndMs = Math.max(line.lineStart + 2000, naturalEndMs - 2000);
	const scrollStartMs = line.lineStart + (scrollEndMs - line.lineStart) * 0.3;
	const scrollX = useTransform(
		currentTimeMotion,
		[scrollStartMs, scrollEndMs],
		[0, -overflowPx],
	);

	return (
		// max-w-full：外层被约束到行槽宽后，内层溢出量才能测出来
		//（nowrap 的 flex 子项 min-width:auto 不可收缩，没有这条约束
		// inner.offsetWidth − outer.offsetWidth 恒 0，滚动永不启动）；
		// 左右交替布局下靠左行的滚动起点（文本开头）也贴齐槽左缘
		<span
			ref={outerRef}
			className="relative inline-block max-w-full"
			style={textStyle}
		>
			<motion.span
				ref={innerRef}
				className="relative inline-block"
				style={{ x: isVertical ? 0 : scrollX }}
			>
				{/* 暗色描边背衬层：永远完整描出整行轮廓（任意壁纸下的可读性兜底） */}
				<span
					aria-hidden
					className="desktop-lyric-glyph-backdrop absolute inset-0"
				>
					{line.lineText}
				</span>
				<span className="desktop-lyric-glyph-halo relative">
					{hasWords && line.words ? (
						line.words.map((word) => (
							<VerbatimWord
								key={`${word.startTime}-${word.duration}-${word.char}`}
								word={word}
								currentTimeMotion={currentTimeMotion}
								orientation={isVertical ? "vertical" : "horizontal"}
							/>
						))
					) : (
						<span style={{ color: "inherit" }}>{line.lineText}</span>
					)}
				</span>
			</motion.span>
		</span>
	);
}

/** 过门 / 空闲的呼吸圆点（竖排时纵向排列） */
export function DesktopLeadDots({ vertical }: { vertical: boolean }) {
	return (
		<div
			className={cn(
				"flex items-center justify-center gap-3",
				vertical && "flex-col",
			)}
		>
			{[0, 1, 2].map((i) => (
				<motion.span
					key={i}
					className="rounded-full bg-white/70"
					style={{
						width: 10,
						height: 10,
						boxShadow: "0 1px 6px rgb(0 0 0 / 0.5)",
					}}
					animate={{ opacity: [0.2, 0.8, 0.2] }}
					transition={{
						duration: 1.4,
						repeat: Infinity,
						delay: i * 0.22,
						ease: "easeInOut",
					}}
				/>
			))}
		</div>
	);
}
