import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { StrokeCharCell } from "@/components/lyric/stroke/stroke-char";
import {
	flattenLineChars,
	STROKE_LINE_SCATTER_MS,
	shouldUseInstantDisplay,
	splitStrokeColumns,
} from "@/components/lyric/stroke/stroke-columns";
import { cn } from "@/lib/utils";
import { prefetchHanziChars } from "@/lib/utils/hanzi-strokes";
import type { ILyricLine } from "@/lib/utils/lyric-parser";
import { DesktopLeadDots } from "./desktop-lyric-default-view";

/**
 * 桌面歌词「笔画动效」视图：与歌词页 StrokeLyricView 同一套进入+散开
 * 两段式（逐字按笔画飞入拼合、句尾四散散开），复用 StrokeCharCell 与
 * stroke-columns 的纯逻辑，句切换状态机（先等旧句动画播完再散开）也
 * 保持一致。差异只在舞台形态——桌面歌词窗口小、无翻译副行：
 *
 * - 横向：单行字格居中，字号按「窗口高 × 0.62」与「宽度 ÷ 字数」双钳制；
 * - 竖向：按句长拆 1~3 列右起直书（列序与书写方向一致，先唱的在最右）。
 */

/** 切句等待旧句动画的上限（ms）：兜底防失控（与歌词页一致） */
const SWITCH_WAIT_MAX_MS = 2000;

/** 桌面歌词笔画字格尺寸上下限（px）：窗口小于歌词页舞台，整体略收 */
const CELL_MIN_PX = 20;
const CELL_MAX_PX = 56;

interface DesktopStrokeViewProps {
	lines: ILyricLine[];
	currentIndex: number;
	direction: "horizontal" | "vertical";
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
	direction,
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

	// 预取下一句笔画数据（切句时数据大概率已在缓存）
	const nextLine =
		currentIndex >= 0 && currentIndex + 1 < lines.length
			? lines[currentIndex + 1]
			: undefined;
	useEffect(() => {
		if (nextLine && !nextLine.isLeadDots) prefetchHanziChars(nextLine.lineText);
	}, [nextLine]);

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

	// 舞台测量（窗口随方向/字号缩放时重算字格尺寸）
	const stageRef = useRef<HTMLDivElement | null>(null);
	const [stageSize, setStageSize] = useState({ width: 0, height: 0 });
	useEffect(() => {
		const el = stageRef.current;
		if (!el) return;
		const update = () =>
			setStageSize({ width: el.clientWidth, height: el.clientHeight });
		update();
		const ro = new ResizeObserver(update);
		ro.observe(el);
		return () => ro.disconnect();
	}, []);

	const isVertical = direction === "vertical";
	// 竖排按句长拆列（右起），横排单行
	const columns = useMemo(
		() => (isVertical ? splitStrokeColumns(timings) : []),
		[isVertical, timings],
	);

	// 字格尺寸：横向按高度与宽度均钳；竖向按列长与列数钳
	const cellSize = useMemo(() => {
		const clamp = (v: number) =>
			Math.min(CELL_MAX_PX, Math.max(CELL_MIN_PX, Math.floor(v)));
		if (isVertical) {
			const maxColLength = Math.max(...columns.map((c) => c.items.length), 1);
			const byHeight = (stageSize.height * 0.92) / maxColLength;
			const byWidth =
				(stageSize.width * 0.86) / (Math.max(columns.length, 1) * 1.45);
			return clamp(Math.min(byHeight, byWidth));
		}
		const byHeight = stageSize.height * 0.62;
		const byWidth = (stageSize.width * 0.92) / Math.max(timings.length, 1);
		return clamp(Math.min(byHeight, byWidth));
	}, [columns, isVertical, stageSize, timings.length]);

	const cellGap = Math.max(4, Math.round(cellSize * 0.14));
	const columnGap = Math.max(14, Math.round(cellSize * 0.4));

	const showCells =
		displayLine !== undefined &&
		!displayLine.isLeadDots &&
		(isVertical ? columns.length > 0 : timings.length > 0);

	return (
		<div
			ref={stageRef}
			className="flex h-full w-full items-center justify-center overflow-hidden select-none"
		>
			{showCells && displayLine ? (
				<div
					className={cn(
						"flex h-full w-full items-center justify-center",
						isVertical && "flex-row-reverse",
					)}
					style={{ gap: isVertical ? columnGap : cellGap }}
				>
					{(isVertical ? columns.map((c) => c.items) : [timings]).map(
						(items, ci) => (
							<div
								// biome-ignore lint/suspicious/noArrayIndexKey: 列布局句内静态不重排，索引即稳定标识
								key={`col-${displayLine.lineStart}-${ci}`}
								className={cn(
									"flex items-center",
									isVertical ? "flex-col justify-center" : "flex-row",
								)}
								style={{ gap: cellGap }}
							>
								{items.map((t, i) => (
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
						),
					)}
				</div>
			) : (
				<DesktopLeadDots vertical={isVertical} />
			)}
		</div>
	);
}
