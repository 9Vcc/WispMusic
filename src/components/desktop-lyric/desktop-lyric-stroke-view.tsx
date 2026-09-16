import { useCallback, useEffect, useMemo, useRef } from "react";
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
import { useLyricPair } from "./use-lyric-pair";

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

/** 桌面歌词笔画字格最小尺寸（px）：字格跟随有效字号，仅在长句放不下时收缩，最小值保证字形可辨 */
const CELL_MIN_PX = 12;

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
	const isVertical = direction === "vertical";

	// 旧行飞入动画的最晚完成时刻（performance.now 时钟）：整对散开前等它到期
	const flyEndAtRef = useRef(0);
	const handleFlyEndAt = useCallback((endAtMs: number) => {
		flyEndAtRef.current = Math.max(flyEndAtRef.current, endAtMs);
	}, []);
	const getPendingWaitMs = useCallback(
		() => Math.max(0, flyEndAtRef.current - performance.now()),
		[],
	);

	// 配对轮换状态机与默认动效共用（第一句播完再播第二句，两句播完才换对）；
	// 差异只在这里：笔画要等字格飞入动画播完再散开，退出时长用散开时长
	const { pairFirst, phase } = useLyricPair({
		currentIndex,
		exitMs: STROKE_LINE_SCATTER_MS,
		getPendingWaitMs,
		maxWaitMs: SWITCH_WAIT_MAX_MS,
	});
	// 换进新配对后清空旧的飞入完成时刻（新字格会重新登记）
	// biome-ignore lint/correctness/useExhaustiveDependencies: pairFirst 是刻意的触发源——body 只写 ref，但换对必须复位
	useEffect(() => {
		flyEndAtRef.current = 0;
	}, [pairFirst]);
	const display = { first: pairFirst, phase };

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

	// 字格尺寸：**目标尺寸 = 有效字号**（与默认动效同一自适应口径——字号本身
	// 已按窗口尺寸自适应），仅当整句放不下时才按窗口/内容收缩。旧实现用
	// 绝对上限 56px 夹住，最大字号下字格明显小于默认动效的文字。
	// 两种方向共用同一套推导：横向两行受宽度约束（字数并排一行）、竖向两列
	// 受高度约束（字数叠一列），另一维给出两格所需的余量
	const cellSize = useMemo(() => {
		const toCellSize = (v: number) =>
			Math.max(CELL_MIN_PX, Math.floor(Number.isFinite(v) ? v : fontSize));
		const maxChars = Math.max(firstTimings.length, secondTimings.length, 1);
		if (isVertical) {
			const availableWidth = Math.max(
				0,
				design.width -
					DESKTOP_LYRIC_EDGE_INSET / 2 -
					DESKTOP_LYRIC_VERTICAL_SAFE_INSET,
			);
			const byWidth = availableWidth / (2 * VERTICAL_COLUMN_SLOT_RATIO);
			const byHeight = (design.height * 0.94) / maxChars;
			return toCellSize(Math.min(fontSize, byWidth, byHeight));
		}
		const byHeight = stageDesignHeight * 0.42;
		const byWidth = (design.width * 0.92) / maxChars;
		return toCellSize(Math.min(fontSize, byHeight, byWidth));
	}, [
		design,
		fontSize,
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
