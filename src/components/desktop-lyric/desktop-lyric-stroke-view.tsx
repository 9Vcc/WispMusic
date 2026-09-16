import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { StrokeCharCell } from "@/components/lyric/stroke/stroke-char";
import {
	flattenLineChars,
	STROKE_LINE_SCATTER_MS,
	type StrokeCharTiming,
	shouldUseInstantDisplay,
} from "@/components/lyric/stroke/stroke-columns";
import { cn } from "@/lib/utils";
import {
	buildStrokeRowColorVars,
	type DesktopLyricColors,
} from "@/lib/utils/desktop-lyric-colors";
import {
	computeDesktopLyricGeometry,
	DESKTOP_LYRIC_EDGE_INSET,
	DESKTOP_LYRIC_HEADER_RESERVE,
	DESKTOP_LYRIC_VERTICAL_SAFE_INSET,
} from "@/lib/utils/desktop-lyric-geometry";
import { prefetchHanziChars } from "@/lib/utils/hanzi-strokes";
import type { ILyricLine } from "@/lib/utils/lyric-parser";
import { DesktopLeadDots } from "./desktop-lyric-default-view";

/**
 * 桌面歌词「笔画动效」视图：与歌词页 StrokeLyricView 同一套进入+散开
 * 两段式（逐字按笔画飞入拼合、句尾四散散开），复用 StrokeCharCell 与
 * stroke-columns 的纯逻辑。差异只在舞台形态——桌面歌词窗口小、无翻译副行。
 *
 * **两种方向都是「配对双格」结构，只是格子的排列轴不同**（保持一致的两
 * 格显示效果）：
 * - 横向：双行——上行=当前句（贴左）、下行=下一句（贴右）；
 * - 竖向：双列——左列=当前句（贴左 + 列内贴顶）、右列=下一句（贴右 +
 *   列内贴底，右侧避让工具条）。
 *
 * 配对 = 连续两句歌词 (first, first+1)，两格都是**正常播放**（非预览、同
 * 亮度）：各自在自己的演唱窗口内字到演唱点起飞、铺满整句。当前句先播，
 * 播完下一句接播（此时它成为当前句）；两句都播完、当前句推进到配对之外
 * 时轮换——整对散开、换进下两句歌词（换的是下两行，不是两行互换）。
 * 末句无下一句时留空占位。
 */

/** 切句等待动画的上限（ms）：兜底防失控（与歌词页一致） */
const SWITCH_WAIT_MAX_MS = 2000;

/** 桌面歌词笔画字格尺寸上下限（px）：窗口小于歌词页舞台，整体略收 */
const CELL_MIN_PX = 20;
const CELL_MAX_PX = 56;

/**
 * 竖排两列的字格宽度占位系数：每个字格占「格宽 × 该系数」的横向额度
 * （含列间距），两列即 2 × 该值，用于把字格钳进可用宽度
 */
const VERTICAL_COLUMN_SLOT_RATIO = 1.4;

interface DesktopStrokeViewProps {
	lines: ILyricLine[];
	currentIndex: number;
	direction: "horizontal" | "vertical";
	/** 字号设置（推导设计几何，字格尺寸按设计坐标计算） */
	fontSize: number;
	/** 用户配色：当前句字格走高亮色，同屏的另一句走普通文字色 */
	colors: DesktopLyricColors;
	getNowMs: () => number;
}

/** 配对展示状态：first=配对首行索引（横向次行为 first+1；竖向 first 即当前句），phase=in 进入（飞入/直接显示）/ out 句尾散开 */
interface PairState {
	first: number;
	phase: "in" | "out";
}

/** 配对行（横向）：一行歌词的笔画字格行——正常跟播（非预览），与歌词页同式预算 */
function PairStrokeRow({
	line,
	timings,
	windowMs,
	cellSize,
	cellGap,
	phase,
	align,
	colorVars,
	getNowMs,
	onFlyEndAt,
}: {
	line: ILyricLine | undefined;
	timings: StrokeCharTiming[];
	/** 本行演唱窗口（下一行 lineStart − 本行 lineStart，末行兜底） */
	windowMs: number;
	cellSize: number;
	cellGap: number;
	phase: "in" | "out";
	align: "start" | "end";
	/** 本行字格填充色变量（当前句=高亮色 / 其余=文字色），覆盖根层默认值 */
	colorVars: Record<string, string>;
	getNowMs: () => number;
	onFlyEndAt: (endAtMs: number) => void;
}) {
	const instantLine = useMemo(
		() => shouldUseInstantDisplay(timings),
		[timings],
	);
	// 行级飞入预算：本行窗口 − 120ms 尾部余量 − 末字触发偏移（与歌词页同式）
	const lastCharOffsetMs =
		timings.length > 0
			? timings[timings.length - 1].triggerAt - (line?.lineStart ?? 0)
			: 0;
	const flyBudgetMs = Math.max(0, windowMs - 120 - lastCharOffsetMs);
	// 末句无次行：右下占位，保持双行槽位稳定
	if (!line) return <div className="flex min-h-0 flex-1" />;
	return (
		<div
			className={cn(
				"flex min-h-0 flex-1 items-center",
				// 贴边：首行（当前句）恒贴左、次行（下一句）恒贴右，与歌词行号
				// 奇偶无关（默认动效视图同规则）；边距只留字形描边不被裁的余量
				align === "start" ? "justify-start" : "justify-end",
			)}
			style={
				{
					paddingLeft: DESKTOP_LYRIC_EDGE_INSET,
					paddingRight: DESKTOP_LYRIC_EDGE_INSET,
					...colorVars,
				} as React.CSSProperties
			}
		>
			{/* 不加 overflow-hidden——笔画要从窗口外飞入，裁切会让动画只覆盖
			    半窗（字格尺寸已按宽度钳制，内容本就不溢出） */}
			{line.isLeadDots ? (
				<DesktopLeadDots vertical={false} />
			) : (
				<div className="flex items-center" style={{ gap: cellGap }}>
					{timings.map((t, i) => (
						<div
							// biome-ignore lint/suspicious/noArrayIndexKey: 句内字符列表静态不重排，索引即稳定标识
							key={`cell-${line.lineStart}-${i}-${t.char}`}
							className="desktop-lyric-stroke-shadow shrink-0"
							style={{ width: cellSize, height: cellSize }}
						>
							<StrokeCharCell
								char={t.char}
								cellSize={cellSize}
								phase={phase}
								enter={instantLine ? "instant" : "fly"}
								triggerAt={t.triggerAt}
								flyBudgetMs={flyBudgetMs}
								onFlyEndAt={onFlyEndAt}
								getNowMs={getNowMs}
							/>
						</div>
					))}
				</div>
			)}
		</div>
	);
}

/**
 * 配对列（竖向）：一行歌词的笔画字格列——与 PairStrokeRow 同构，只是
 * 字格沿纵向堆叠（书写方向向下），并且：
 * - 贴边：当前句列恒贴左（列内贴顶）、下一句列恒贴右（列内贴底），与
 *   横向双行同规则（默认动效视图一致）；
 * - 列宽由外层两等分槽位给出，列内字格横向贴自己的边（items-start/end），
 *   纵向按 align 贴顶 / 贴底。
 */
function PairStrokeColumn({
	line,
	timings,
	windowMs,
	cellSize,
	cellGap,
	phase,
	align,
	colorVars,
	getNowMs,
	onFlyEndAt,
}: {
	line: ILyricLine | undefined;
	timings: StrokeCharTiming[];
	/** 本列演唱窗口（下一行 lineStart − 本行 lineStart，末行兜底） */
	windowMs: number;
	cellSize: number;
	cellGap: number;
	phase: "in" | "out";
	align: "start" | "end";
	/** 本列字格填充色变量（当前句=高亮色 / 其余=文字色），覆盖根层默认值 */
	colorVars: Record<string, string>;
	getNowMs: () => number;
	onFlyEndAt: (endAtMs: number) => void;
}) {
	const instantLine = useMemo(
		() => shouldUseInstantDisplay(timings),
		[timings],
	);
	// 列级飞入预算：本列窗口 − 120ms 尾部余量 − 末字触发偏移（与横向同式）
	const lastCharOffsetMs =
		timings.length > 0
			? timings[timings.length - 1].triggerAt - (line?.lineStart ?? 0)
			: 0;
	const flyBudgetMs = Math.max(0, windowMs - 120 - lastCharOffsetMs);
	// 末句无下一句：右列占位，保持双列槽位稳定
	if (!line) return <div className="flex min-w-0 flex-1" />;
	const isStart = align === "start";
	return (
		<div
			className={cn(
				"flex min-w-0 flex-1 flex-col",
				isStart ? "items-start justify-start" : "items-end justify-end",
			)}
			style={colorVars as React.CSSProperties}
		>
			{line.isLeadDots ? (
				<DesktopLeadDots vertical />
			) : (
				<div
					className={cn("flex flex-col", isStart ? "items-start" : "items-end")}
					style={{ gap: cellGap }}
				>
					{timings.map((t, i) => (
						<div
							// biome-ignore lint/suspicious/noArrayIndexKey: 句内字符列表静态不重排，索引即稳定标识
							key={`cell-${line.lineStart}-${i}-${t.char}`}
							className="desktop-lyric-stroke-shadow shrink-0"
							style={{ width: cellSize, height: cellSize }}
						>
							<StrokeCharCell
								char={t.char}
								cellSize={cellSize}
								phase={phase}
								enter={instantLine ? "instant" : "fly"}
								triggerAt={t.triggerAt}
								flyBudgetMs={flyBudgetMs}
								onFlyEndAt={onFlyEndAt}
								getNowMs={getNowMs}
							/>
						</div>
					))}
				</div>
			)}
		</div>
	);
}

export function DesktopStrokeView({
	lines,
	currentIndex,
	direction,
	fontSize,
	colors,
	getNowMs,
}: DesktopStrokeViewProps) {
	// 配对展示状态机：当前句在配对内不动；推进到配对外时轮换——先等两格
	// 动画播完（封顶防失控），整对散开后再换进以新当前句为首的下两句。
	// 横向双行与竖向双列都覆盖 (first, first+1) 两句，判定完全一致
	const [display, setDisplay] = useState<PairState>({ first: -1, phase: "in" });
	const displayRef = useRef(display);
	displayRef.current = display;
	const pendingFirstRef = useRef(-1);
	const swapTimerRef = useRef<number | null>(null);
	// 旧行飞入动画的最晚完成时刻（performance.now 时钟）：轮换散开前等它到期
	const flyEndAtRef = useRef(0);
	const handleFlyEndAt = useCallback((endAtMs: number) => {
		flyEndAtRef.current = Math.max(flyEndAtRef.current, endAtMs);
	}, []);

	const isVertical = direction === "vertical";

	useEffect(() => {
		const cur = displayRef.current;
		// 当前句仍在配对内（首句或次句）——不轮换
		const inPair = currentIndex === cur.first || currentIndex === cur.first + 1;
		if (inPair) {
			// 跳回配对内（seek 回）：取消待轮换定时器；已散开则复位重进本配对
			if (cur.first !== -1 && swapTimerRef.current !== null) {
				clearTimeout(swapTimerRef.current);
				swapTimerRef.current = null;
				if (cur.phase === "out") {
					setDisplay({ first: cur.first, phase: "in" });
				}
			}
			return;
		}
		if (cur.first === -1) {
			setDisplay({ first: currentIndex, phase: "in" });
			return;
		}
		if (cur.phase === "out") {
			pendingFirstRef.current = currentIndex;
			return;
		}
		pendingFirstRef.current = currentIndex;
		const waitMs = Math.min(
			Math.max(0, flyEndAtRef.current - performance.now()),
			SWITCH_WAIT_MAX_MS,
		);
		if (swapTimerRef.current !== null) clearTimeout(swapTimerRef.current);
		swapTimerRef.current = window.setTimeout(() => {
			swapTimerRef.current = null;
			setDisplay({ first: displayRef.current.first, phase: "out" });
			swapTimerRef.current = window.setTimeout(() => {
				swapTimerRef.current = null;
				flyEndAtRef.current = 0;
				setDisplay({ first: pendingFirstRef.current, phase: "in" });
			}, STROKE_LINE_SCATTER_MS);
		}, waitMs);
	}, [currentIndex]);

	// 卸载时清理散开→换句的定时器
	useEffect(() => {
		return () => {
			if (swapTimerRef.current !== null) clearTimeout(swapTimerRef.current);
		};
	}, []);

	// 预取笔画数据：下一配对两句（当前对的次行随本对挂载自取，无需预取）。
	// 轮换时数据大概率已在缓存；横向双行与竖向双列的预取口径一致
	const prefetchTexts = useMemo(() => {
		const texts: string[] = [];
		if (display.first >= 0) {
			for (const idx of [display.first + 2, display.first + 3]) {
				if (idx < lines.length) texts.push(lines[idx].lineText);
			}
		}
		return texts;
	}, [display.first, lines]);
	useEffect(() => {
		for (const text of prefetchTexts) prefetchHanziChars(text);
	}, [prefetchTexts]);

	// 配对两行（散开期间仍保持旧对直到散开结束）
	const firstLine =
		display.first >= 0 && display.first < lines.length
			? lines[display.first]
			: undefined;
	const secondLine =
		display.first >= 0 && display.first + 1 < lines.length
			? lines[display.first + 1]
			: undefined;
	const thirdLine =
		display.first >= 0 && display.first + 2 < lines.length
			? lines[display.first + 2]
			: undefined;
	// 字格颜色变量：横向配对里**正在演唱的那一行**用高亮色，另一行用普通
	// 文字色（随 currentIndex 在配对内推进而实时互换，与默认动效的
	// 「当前句高亮」语义一致）；竖向只展示当前句，恒用高亮色
	const firstRowColorVars = useMemo(
		() => buildStrokeRowColorVars(colors, currentIndex === display.first),
		[colors, currentIndex, display.first],
	);
	const secondRowColorVars = useMemo(
		() => buildStrokeRowColorVars(colors, currentIndex === display.first + 1),
		[colors, currentIndex, display.first],
	);
	// 各行演唱窗口（ms）：到各自下一行切句点为止（末行给兜底窗口）
	const firstWindowMs = firstLine
		? secondLine
			? Math.max(600, secondLine.lineStart - firstLine.lineStart)
			: 5000
		: 5000;
	const secondWindowMs = secondLine
		? thirdLine
			? Math.max(600, thirdLine.lineStart - secondLine.lineStart)
			: 5000
		: 5000;

	// 逐字时序摊平 + 急促连唱免飞入判定
	const firstTimings = useMemo(
		() =>
			firstLine && !firstLine.isLeadDots ? flattenLineChars(firstLine) : [],
		[firstLine],
	);
	const secondTimings = useMemo(
		() =>
			secondLine && !secondLine.isLeadDots ? flattenLineChars(secondLine) : [],
		[secondLine],
	);

	// 设计几何（与 DesktopLyric 舞台的缩放基准一致）：字格尺寸按设计坐
	// 标计算，缩放比放大/缩小时视觉尺寸随之等比变化，永不溢出裁切
	const design = computeDesktopLyricGeometry(direction, fontSize);
	const stageDesignHeight =
		design.height - (isVertical ? 0 : DESKTOP_LYRIC_HEADER_RESERVE);

	// 字格尺寸：两种方向都是「配对双格共享同一尺寸」（同字号观感）——
	// 横向受宽度约束（两行各自的字数并排在一行里）、竖向受高度约束
	// （两列各自的字数叠在一列里），另一维给出两格所需的余量
	const cellSize = useMemo(() => {
		const clamp = (v: number) =>
			Math.min(CELL_MAX_PX, Math.max(CELL_MIN_PX, Math.floor(v)));
		const maxChars = Math.max(firstTimings.length, secondTimings.length, 1);
		if (isVertical) {
			// 竖向：两列并排 → 宽度除以「两列 + 列间距」，高度留给最长的一列
			const availableWidth = Math.max(
				0,
				design.width -
					DESKTOP_LYRIC_EDGE_INSET / 2 -
					DESKTOP_LYRIC_VERTICAL_SAFE_INSET,
			);
			const byWidth = availableWidth / (2 * VERTICAL_COLUMN_SLOT_RATIO);
			const byHeight = (design.height * 0.94) / maxChars;
			return clamp(Math.min(byHeight, byWidth));
		}
		const byHeight = stageDesignHeight * 0.42;
		const byWidth = (design.width * 0.92) / maxChars;
		return clamp(Math.min(byHeight, byWidth));
	}, [
		design,
		isVertical,
		stageDesignHeight,
		firstTimings.length,
		secondTimings.length,
	]);

	const cellGap = Math.max(4, Math.round(cellSize * 0.14));

	return (
		<div
			className={cn(
				"flex h-full w-full select-none",
				isVertical ? "items-stretch" : "flex-col",
			)}
		>
			{isVertical ? (
				// 竖排配对双列：左列=当前句（贴左 + 列内贴顶）、右列=下一句
				// （贴右 + 列内贴底，避让工具条）——与横向双行同为「配对双格」，
				// 只是排列轴从行换成列；两列同亮度、同为正常播放
				<div
					className="flex h-full w-full"
					style={{
						paddingTop: DESKTOP_LYRIC_EDGE_INSET / 2,
						paddingBottom: DESKTOP_LYRIC_EDGE_INSET / 2,
						paddingLeft: DESKTOP_LYRIC_EDGE_INSET / 2,
						paddingRight: DESKTOP_LYRIC_VERTICAL_SAFE_INSET,
					}}
				>
					<PairStrokeColumn
						line={firstLine}
						timings={firstTimings}
						windowMs={firstWindowMs}
						cellSize={cellSize}
						cellGap={cellGap}
						phase={display.phase}
						align="start"
						colorVars={firstRowColorVars}
						getNowMs={getNowMs}
						onFlyEndAt={handleFlyEndAt}
					/>
					<PairStrokeColumn
						line={secondLine}
						timings={secondTimings}
						windowMs={secondWindowMs}
						cellSize={cellSize}
						cellGap={cellGap}
						phase={display.phase}
						align="end"
						colorVars={secondRowColorVars}
						getNowMs={getNowMs}
						onFlyEndAt={handleFlyEndAt}
					/>
				</div>
			) : firstLine ? (
				<>
					{/* 左上：配对首行——当前句，先播（字到演唱点起飞、铺满整句） */}
					<PairStrokeRow
						line={firstLine}
						timings={firstTimings}
						windowMs={firstWindowMs}
						cellSize={cellSize}
						cellGap={cellGap}
						phase={display.phase}
						align="start"
						colorVars={firstRowColorVars}
						getNowMs={getNowMs}
						onFlyEndAt={handleFlyEndAt}
					/>
					{/* 右下：配对次行——首行播完接播（成为当前句后正常跟播）。
					    两行同亮度、同为正常播放（非预览）；不加 overflow-hidden，
					    动画覆盖整个窗口 */}
					<PairStrokeRow
						line={secondLine}
						timings={secondTimings}
						windowMs={secondWindowMs}
						cellSize={cellSize}
						cellGap={cellGap}
						phase={display.phase}
						align="end"
						colorVars={secondRowColorVars}
						getNowMs={getNowMs}
						onFlyEndAt={handleFlyEndAt}
					/>
				</>
			) : (
				<div className="flex h-full w-full items-center justify-center">
					<DesktopLeadDots vertical={false} />
				</div>
			)}
		</div>
	);
}
