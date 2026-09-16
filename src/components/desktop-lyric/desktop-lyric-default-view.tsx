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
import { resolveVerticalSlots } from "./desktop-lyric-slots";

/**
 * 桌面歌词「默认动效」视图。
 *
 * 横向布局为双行（EchoMusic 式）：行元素按歌词索引作 key 绝对定位在
 * 两个行槽上——切句时旧当前句上滑退出、旧下一句**同 key 晋级**（行槽
 * y 与字号缩放平滑过渡，文本内容同步换为新当前句）、新下一句从底部
 * 滑入；当前句带逐字填充动效（与歌词页共用 VerbatimWord），下一句是
 * 静态低亮度预览；超宽当前句按演唱进度横向滚动（30% 进度起、句尾前
 * 2s 收住）。竖向布局保持单列直书，进出场沿水平轴整列滑动。
 *
 * 与歌词页的关键差异：桌面歌词浮在任意壁纸上，plus-lighter 叠加不可用，
 * 可读性由独立的暗色描边背衬层保证——背衬层与填充层同文本同布局完全
 * 重叠，永远完整描出整行轮廓，填充层（含逐字遮罩）叠在其上。
 */

export type DesktopLyricOrientation = "horizontal" | "vertical";

interface DesktopDefaultViewProps {
	/** 当前句（undefined / 过门圆点行 → 显示呼吸圆点） */
	line?: ILyricLine;
	/** 下一句（横向双行的预览行） */
	nextLine?: ILyricLine;
	/** 当前句索引（行元素的稳定 key：下一句晋级当前句时 key 不变，触发平滑过渡） */
	currentIndex: number;
	direction: DesktopLyricOrientation;
	fontSize: number;
	/** 用户配色：当前句用高亮色、预览行用文字色，逐字填充走 --dl-word-fill-color */
	colors: DesktopLyricColors;
	currentTimeMotion: MotionValue<number>;
}

/** 单行槽位高度（与 lineHeight 1.35 一致，钳制超高字符防溢出对侧行） */
const LINE_SLOT_RATIO = 1.35;
/** 下一句预览的字号缩放（EchoMusic 0.8em 层级） */
const NEXT_LINE_SCALE = 0.8;
/** 行槽切换的滑动时长与缓动（EchoMusic lyric-slide 同参数） */
const SLOT_SLIDE: Transition = { duration: 0.22, ease: [0.22, 1, 0.36, 1] };

export function DesktopDefaultView({
	line,
	nextLine,
	currentIndex,
	direction,
	fontSize,
	colors,
	currentTimeMotion,
}: DesktopDefaultViewProps) {
	const isVertical = direction === "vertical";
	// 行色由配色推导（默认白 = 改造前的 rgb(255 255 255 / 0.88) 与 / 0.45）
	const activeLineColor = withAlpha(
		colors.highlightColor,
		DESKTOP_LYRIC_ALPHA.current,
	);
	const previewLineColor = withAlpha(
		colors.textColor,
		DESKTOP_LYRIC_ALPHA.preview,
	);

	if (isVertical) {
		// 竖向双列：**当前句恒左列（列贴左缘 + 内容贴顶）、下一句恒右列
		// （列贴右缘 + 内容贴底）**，两句天然错位成对角。贴边只由槽位决定，
		// 与歌词行号奇偶无关——旧实现按 index % 2 分配列，奇数句时当前句
		// 会跑到右下（用户反馈的「贴边不准确」）。各槽独立进出场（整列沿
		// 水平轴滑动，与书写方向一致；左槽从左缘、右槽从右缘）；当前句整
		// 字号、下一句 0.8 灰。右缘留出工具条安全边距，贴右是「贴到工具条
		// 内侧的右缘」而不是被工具栏压住。
		const columnSlide = fontSize * 1.35;
		const slots = resolveVerticalSlots({ line, nextLine, currentIndex });
		const hasAny = Boolean(slots.left || slots.right);
		const renderColumn = (
			item: ReturnType<typeof resolveVerticalSlots>["left"],
			side: "left" | "right",
		) => {
			const isLeft = side === "left";
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
								key={`col-${item.index}`}
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
									color: item.isCurrent ? activeLineColor : previewLineColor,
								}}
							>
								<DesktopLineText
									line={item.line}
									direction={direction}
									fontSize={
										item.isCurrent
											? fontSize
											: Math.round(fontSize * NEXT_LINE_SCALE)
									}
									currentTimeMotion={currentTimeMotion}
									isCurrent={item.isCurrent}
									lineWindowMs={
										nextLine ? nextLine.lineStart - item.line.lineStart : 10000
									}
								/>
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
					color: activeLineColor,
					paddingTop: DESKTOP_LYRIC_EDGE_INSET / 2,
					paddingBottom: DESKTOP_LYRIC_EDGE_INSET / 2,
					paddingLeft: DESKTOP_LYRIC_EDGE_INSET / 2,
					paddingRight: DESKTOP_LYRIC_VERTICAL_SAFE_INSET,
				}}
			>
				{!hasAny ? (
					<div className="flex h-full w-full items-center justify-center">
						<DesktopLeadDots vertical />
					</div>
				) : (
					<>
						{renderColumn(slots.left, "left")}
						{renderColumn(slots.right, "right")}
					</>
				)}
			</div>
		);
	}

	// 横向双行：**上行（当前句）恒贴左边、下行（下一句）恒贴右边**，与歌词
	// 行号奇偶无关（旧实现按 index % 2 交替，奇数句时上行会跑到右边）。
	// 行元素按索引作 key：下一句晋级当前句时 key 不变，framer 平滑过渡
	// y/缩放/颜色——贴边侧也会在这一刻从右换到左，故文本块挂 `layout`，
	// 让这次换边以位移动画完成而不是瞬移。行槽块恒定挂载（含呼吸圆点
	// 兜底），块自身 overflow-hidden，进出场滑动在块边缘裁切；左右各留
	// 一点边距，避免字形描边被窗口裁掉。
	const slotH = fontSize * LINE_SLOT_RATIO;
	const items: Array<{ line: ILyricLine; index: number; isCurrent: boolean }> =
		line && !line.isLeadDots && currentIndex >= 0
			? [
					{ line, index: currentIndex, isCurrent: true },
					...(nextLine
						? [{ line: nextLine, index: currentIndex + 1, isCurrent: false }]
						: []),
				]
			: [];

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
				{items.length === 0 && (
					<div className="absolute inset-0 flex items-center justify-center">
						<DesktopLeadDots vertical={false} />
					</div>
				)}
				<AnimatePresence initial={false}>
					{items.map(({ line: itemLine, index, isCurrent }) => (
						<motion.div
							key={`line-${index}`}
							initial={{ opacity: 0, y: slotH * 2 }}
							animate={{
								opacity: 1,
								y: isCurrent ? 0 : slotH,
								scale: isCurrent ? 1 : NEXT_LINE_SCALE,
								color: isCurrent ? activeLineColor : previewLineColor,
							}}
							exit={{ opacity: 0, y: -slotH }}
							transition={{
								...SLOT_SLIDE,
								opacity: { duration: 0.18, ease: "easeOut" },
							}}
							className={cn(
								"absolute inset-x-0 top-0 flex items-center overflow-hidden",
								isCurrent ? "justify-start" : "justify-end",
							)}
							style={{
								height: slotH,
								// 缩放原点贴对齐侧（EchoMusic 同款）：下一句 0.8
								// 缩放向贴边侧收紧，不产生反向缩进
								transformOrigin: isCurrent ? "left center" : "right center",
							}}
						>
							{/* layout：贴边侧由槽位决定，下一句晋级当前句时会从右换到左，
							    用位移动画承接这次换边（否则是瞬移） */}
							<motion.div
								layout
								className="relative flex max-w-full items-center"
								transition={SLOT_SLIDE}
							>
								<DesktopLineText
									line={itemLine}
									direction={direction}
									fontSize={fontSize}
									currentTimeMotion={currentTimeMotion}
									isCurrent={isCurrent}
									lineWindowMs={
										nextLine ? nextLine.lineStart - itemLine.lineStart : 10000
									}
								/>
							</motion.div>
						</motion.div>
					))}
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
	isCurrent = true,
	lineWindowMs,
}: {
	line: ILyricLine;
	direction: DesktopLyricOrientation;
	fontSize: number;
	currentTimeMotion: MotionValue<number>;
	/** 下一句预览：静态低亮度文本，不带逐字动效与滚动 */
	isCurrent?: boolean;
	/** 当前句的演唱窗口（下一句 lineStart - 本句 lineStart），无逐字数据时作滚动时窗兜底 */
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

	// —— 超宽句横向滚动（仅当前句）：溢出量实测，滚动窗口对齐 EchoMusic
	//（30% 演唱进度起步、句尾前 2s 收住；无逐字数据按行窗口的 80% 兜底）
	const outerRef = useRef<HTMLSpanElement>(null);
	const innerRef = useRef<HTMLSpanElement>(null);
	const [overflowPx, setOverflowPx] = useState(0);
	useLayoutEffect(() => {
		if (!isCurrent || isVertical) return;
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
	}, [isCurrent, isVertical]);

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
				style={{ x: isCurrent && !isVertical ? scrollX : 0 }}
			>
				{/* 暗色描边背衬层：永远完整描出整行轮廓（任意壁纸下的可读性兜底） */}
				<span
					aria-hidden
					className="desktop-lyric-glyph-backdrop absolute inset-0"
				>
					{line.lineText}
				</span>
				<span className="desktop-lyric-glyph-halo relative">
					{isCurrent && hasWords && line.words ? (
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
