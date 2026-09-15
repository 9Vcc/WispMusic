import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { StrokeCharCell } from "@/components/lyric/stroke/stroke-char";
import {
	fitFlyTimeline,
	flattenLineChars,
	STROKE_LINE_SCATTER_MS,
	shouldUseInstantDisplay,
	splitStrokeColumns,
} from "@/components/lyric/stroke/stroke-columns";
import { cn } from "@/lib/utils";
import {
	computeDesktopLyricGeometry,
	DESKTOP_LYRIC_HEADER_RESERVE,
} from "@/lib/utils/desktop-lyric-geometry";
import { prefetchHanziChars } from "@/lib/utils/hanzi-strokes";
import type { ILyricLine } from "@/lib/utils/lyric-parser";
import { DesktopLeadDots } from "./desktop-lyric-default-view";

/**
 * 桌面歌词「笔画动效」视图：与歌词页 StrokeLyricView 同一套进入+散开
 * 两段式（逐字按笔画飞入拼合、句尾四散散开），复用 StrokeCharCell 与
 * stroke-columns 的纯逻辑，句切换状态机（先等动画播完再散开）也保持
 * 一致。差异只在舞台形态——桌面歌词窗口小、无翻译副行：
 *
 * - 横向：双行布局（按行索引奇偶一左一右交替）——左行当前句跟播笔画
 *   飞入；左行动画播完后，右行**提前演示**下一句的飞入（时间轴平移
 *   复用同一套飞入逻辑，无静态预览）；两行动画全部上报完成后才轮换；
 * - 竖向：按句长拆 1~3 列右起直书（列序与书写方向一致，先唱的在最右），
 *   右侧预留工具栏安全边距。
 */

/** 切句等待动画的上限（ms）：兜底防失控（与歌词页一致） */
const SWITCH_WAIT_MAX_MS = 2000;

/** 桌面歌词笔画字格尺寸上下限（px）：窗口小于歌词页舞台，整体略收 */
const CELL_MIN_PX = 20;
const CELL_MAX_PX = 56;

/** 右行演示的启动间隙（ms）：左行动画估算完成后稍作停顿再开始演示 */
const DEMO_START_GAP_MS = 150;

interface DesktopStrokeViewProps {
	lines: ILyricLine[];
	currentIndex: number;
	/** 下一句（横向双行的演示行，与默认动效视图同源跳过过门圆点行） */
	nextLine?: ILyricLine;
	direction: "horizontal" | "vertical";
	/** 字号设置（推导设计几何，字格尺寸按设计坐标计算） */
	fontSize: number;
	getNowMs: () => number;
}

/** 整句展示状态：index=展示中的句子，phase=in 进入（飞入/直接显示）/ out 句尾散开 */
interface DisplayState {
	index: number;
	phase: "in" | "out";
}

export function DesktopStrokeView({
	lines,
	currentIndex,
	nextLine,
	direction,
	fontSize,
	getNowMs,
}: DesktopStrokeViewProps) {
	// 整句展示状态机：句切换时先散开旧句，散开结束后再直接显示新句
	const [display, setDisplay] = useState<DisplayState>({
		index: -1,
		phase: "in",
	});
	const displayRef = useRef(display);
	displayRef.current = display;
	const pendingIndexRef = useRef(-1);
	const swapTimerRef = useRef<number | null>(null);
	// 旧句飞入动画的最晚完成时刻（performance.now 时钟）：切句散开前等它到期
	const flyEndAtRef = useRef(0);
	const handleFlyEndAt = useCallback((endAtMs: number) => {
		flyEndAtRef.current = Math.max(flyEndAtRef.current, endAtMs);
	}, []);

	useEffect(() => {
		const cur = displayRef.current;
		if (currentIndex === cur.index) {
			// 跳回同一句（seek 回）：取消待切定时器；已散开则复位重进本句
			if (cur.index !== -1 && swapTimerRef.current !== null) {
				clearTimeout(swapTimerRef.current);
				swapTimerRef.current = null;
				if (cur.phase === "out") {
					setDisplay({ index: cur.index, phase: "in" });
				}
			}
			return;
		}
		if (cur.index === -1) {
			setDisplay({ index: currentIndex, phase: "in" });
			return;
		}
		if (cur.phase === "out") {
			pendingIndexRef.current = currentIndex;
			return;
		}
		pendingIndexRef.current = currentIndex;
		const waitMs = Math.min(
			Math.max(0, flyEndAtRef.current - performance.now()),
			SWITCH_WAIT_MAX_MS,
		);
		if (swapTimerRef.current !== null) clearTimeout(swapTimerRef.current);
		swapTimerRef.current = window.setTimeout(() => {
			swapTimerRef.current = null;
			setDisplay({ index: displayRef.current.index, phase: "out" });
			swapTimerRef.current = window.setTimeout(() => {
				swapTimerRef.current = null;
				flyEndAtRef.current = 0;
				setDisplay({ index: pendingIndexRef.current, phase: "in" });
			}, STROKE_LINE_SCATTER_MS);
		}, waitMs);
	}, [currentIndex]);

	// 卸载时清理散开→换句的定时器
	useEffect(() => {
		return () => {
			if (swapTimerRef.current !== null) clearTimeout(swapTimerRef.current);
		};
	}, []);

	// 预取下一句笔画数据（切句时数据大概率已在缓存；预取跟随播放索引，
	// 与 props.nextLine（跳过圆点后的预览行）职责不同）
	const prefetchLine =
		currentIndex >= 0 && currentIndex + 1 < lines.length
			? lines[currentIndex + 1]
			: undefined;
	useEffect(() => {
		if (prefetchLine && !prefetchLine.isLeadDots)
			prefetchHanziChars(prefetchLine.lineText);
	}, [prefetchLine]);

	// 展示中的句子（散开期间仍保持旧句直到散开结束）
	const displayLine = display.index >= 0 ? lines[display.index] : undefined;
	const displayNextLine =
		display.index + 1 < lines.length ? lines[display.index + 1] : undefined;
	// 展示句的时间窗口（ms）：到下一句切句点为止（末行/倒挂数据给兜底窗口）
	const lineWindowMs =
		displayLine && displayNextLine
			? Math.max(600, displayNextLine.lineStart - displayLine.lineStart)
			: 5000;

	// 逐字时序摊平 + 急促连唱免飞入判定
	const timings = useMemo(
		() =>
			displayLine && !displayLine.isLeadDots
				? flattenLineChars(displayLine)
				: [],
		[displayLine],
	);
	const instantLine = useMemo(
		() => shouldUseInstantDisplay(timings),
		[timings],
	);
	// 句级飞入预算：句窗口 − 120ms 尾部余量 − 末字触发偏移
	const lastCharOffsetMs =
		timings.length > 0
			? timings[timings.length - 1].triggerAt - (displayLine?.lineStart ?? 0)
			: 0;
	const flyBudgetMs = Math.max(0, lineWindowMs - 120 - lastCharOffsetMs);

	// 设计几何（与 DesktopLyric 舞台的缩放基准一致）：字格尺寸按设计坐
	// 标计算，缩放比放大/缩小时视觉尺寸随之等比变化，永不溢出裁切
	const isVertical = direction === "vertical";
	const design = computeDesktopLyricGeometry(direction, fontSize);
	const stageDesignHeight =
		design.height - (isVertical ? 0 : DESKTOP_LYRIC_HEADER_RESERVE);

	// 竖排按句长拆列（右起），横排单行
	const columns = useMemo(
		() => (isVertical ? splitStrokeColumns(timings) : []),
		[isVertical, timings],
	);

	// 字格尺寸：横向双行按半高与宽度均钳；竖向按列长与列数钳
	const cellSize = useMemo(() => {
		const clamp = (v: number) =>
			Math.min(CELL_MAX_PX, Math.max(CELL_MIN_PX, Math.floor(v)));
		if (isVertical) {
			const maxColLength = Math.max(...columns.map((c) => c.items.length), 1);
			const byHeight = (design.height * 0.92) / maxColLength;
			const byWidth =
				(design.width * 0.86) / (Math.max(columns.length, 1) * 1.45);
			return clamp(Math.min(byHeight, byWidth));
		}
		// 双行布局：每行槽占设计舞台高一半，字格占行槽的 ~84%
		const byHeight = stageDesignHeight * 0.42;
		const byWidth = (design.width * 0.92) / Math.max(timings.length, 1);
		return clamp(Math.min(byHeight, byWidth));
	}, [columns, design, isVertical, stageDesignHeight, timings.length]);

	const cellGap = Math.max(4, Math.round(cellSize * 0.14));
	const columnGap = Math.max(14, Math.round(cellSize * 0.4));

	// —— 右行演示时序（横向双行）：左行动画完成后提前演示下一句飞入 ——
	// 启动时刻（歌词时间轴）= 左行末字触发 + 其飞入总时长（fitFlyTimeline
	// 渲染期估算：fastMs+tailMs，稳定无抖动）+ 间隙；已过点（seek 深入
	// 句中）则立即启动。右行 cell 的时钟 = 真实歌词时间 − (演示启动时刻
	// − 下一句 lineStart)（负偏移，演示时钟超前于歌词时钟），下一句首字
	// triggerAt 恰好在演示启动时刻到期——飞入/追赶/预算压缩逻辑完全复用，
	// 无静态预览。
	const demoTimings = useMemo(
		() => (nextLine && !nextLine.isLeadDots ? flattenLineChars(nextLine) : []),
		[nextLine],
	);
	const demoStartLyricMs = useMemo(() => {
		const last = timings[timings.length - 1];
		if (!last || demoTimings.length === 0) return Number.NaN;
		const timeline = fitFlyTimeline(flyBudgetMs);
		const leftDone = last.triggerAt + timeline.fastMs + timeline.tailMs;
		return Math.max(leftDone, getNowMs()) + DEMO_START_GAP_MS;
		// getNowMs 为稳定回调：启动时刻钉在歌词时间轴，仅随句数据/预算重估
	}, [timings, flyBudgetMs, demoTimings.length, getNowMs]);
	const hasDemo = demoTimings.length > 0 && !Number.isNaN(demoStartLyricMs);
	const demoOffsetMs =
		hasDemo && nextLine
			? demoStartLyricMs - nextLine.lineStart
			: Number.POSITIVE_INFINITY;
	const demoWindowMs =
		hasDemo && nextLine
			? Math.max(600, nextLine.lineStart - demoStartLyricMs)
			: 0;
	const demoNowMs = useCallback(
		() => getNowMs() - demoOffsetMs,
		[getNowMs, demoOffsetMs],
	);

	const showCells =
		displayLine !== undefined &&
		!displayLine.isLeadDots &&
		(isVertical ? columns.length > 0 : timings.length > 0);

	return (
		<div
			className={cn(
				"flex h-full w-full select-none",
				isVertical ? "items-center justify-center overflow-hidden" : "flex-col",
			)}
		>
			{isVertical ? (
				showCells && displayLine ? (
					// 竖排右起多列，右侧预留工具栏安全边距（pr-12）
					<div
						className="flex h-full w-full flex-row-reverse items-center justify-center pr-12"
						style={{ gap: columnGap }}
					>
						{columns.map((c, ci) => (
							<div
								// biome-ignore lint/suspicious/noArrayIndexKey: 列布局句内静态不重排，索引即稳定标识
								key={`col-${displayLine.lineStart}-${ci}`}
								className="flex flex-col items-center justify-center"
								style={{ gap: cellGap }}
							>
								{c.items.map((t, i) => (
									<div
										// biome-ignore lint/suspicious/noArrayIndexKey: 列内字符列表静态不重排，索引即稳定标识
										key={`cell-${displayLine.lineStart}-${ci}-${i}-${t.char}`}
										className="desktop-lyric-stroke-shadow shrink-0"
										style={{ width: cellSize, height: cellSize }}
									>
										<StrokeCharCell
											char={t.char}
											cellSize={cellSize}
											phase={display.phase}
											enter={instantLine ? "instant" : "fly"}
											triggerAt={t.triggerAt}
											flyBudgetMs={flyBudgetMs}
											onFlyEndAt={handleFlyEndAt}
											getNowMs={getNowMs}
										/>
									</div>
								))}
							</div>
						))}
					</div>
				) : (
					<DesktopLeadDots vertical />
				)
			) : showCells && displayLine ? (
				<>
					{/* 左行：当前句跟播笔画飞入（完成时刻上报驱动轮换等待与右行演示） */}
					<div className="flex min-h-0 flex-1 items-center justify-start overflow-hidden px-6">
						<div className="flex items-center" style={{ gap: cellGap }}>
							{timings.map((t, i) => (
								<div
									// biome-ignore lint/suspicious/noArrayIndexKey: 句内字符列表静态不重排，索引即稳定标识
									key={`cell-${displayLine.lineStart}-${i}-${t.char}`}
									className="desktop-lyric-stroke-shadow shrink-0"
									style={{ width: cellSize, height: cellSize }}
								>
									<StrokeCharCell
										char={t.char}
										cellSize={cellSize}
										phase={display.phase}
										enter={instantLine ? "instant" : "fly"}
										triggerAt={t.triggerAt}
										flyBudgetMs={flyBudgetMs}
										onFlyEndAt={handleFlyEndAt}
										getNowMs={getNowMs}
									/>
								</div>
							))}
						</div>
					</div>
					{/* 右行：下一句飞入提前演示（左行完成后启动，时钟平移复用
					    同一套飞入逻辑）；完成时刻同样上报，两行都播完才轮换 */}
					<div className="flex min-h-0 flex-1 items-center justify-end overflow-hidden px-6">
						{hasDemo && (
							<div className="flex items-center" style={{ gap: cellGap }}>
								{demoTimings.map((t, i) => (
									<div
										// biome-ignore lint/suspicious/noArrayIndexKey: 演示字符列表静态不重排，索引即稳定标识
										key={`demo-${nextLine?.lineStart ?? 0}-${i}-${t.char}`}
										className="desktop-lyric-stroke-shadow shrink-0"
										style={{ width: cellSize, height: cellSize }}
									>
										<StrokeCharCell
											char={t.char}
											cellSize={cellSize}
											phase="in"
											enter="fly"
											triggerAt={t.triggerAt}
											flyBudgetMs={demoWindowMs}
											onFlyEndAt={handleFlyEndAt}
											getNowMs={demoNowMs}
										/>
									</div>
								))}
							</div>
						)}
					</div>
				</>
			) : (
				<div className="flex h-full w-full items-center justify-center">
					<DesktopLeadDots vertical={false} />
				</div>
			)}
		</div>
	);
}
