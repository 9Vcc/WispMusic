import { motion, useReducedMotion } from "framer-motion";
import {
	type CSSProperties,
	useCallback,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import { corePlayer } from "@/lib/player/corePlayer";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { selectActiveLocalLyric } from "@/lib/store/playerStore/playerStoreSlice/createLocalLyricSlice";
import { selectCurrentLyricOffset } from "@/lib/store/playerStore/playerStoreSlice/createLyricOffsetSlice";
import {
	selectActiveLyricVersion,
	selectActiveLyricVersionLines,
} from "@/lib/store/playerStore/playerStoreSlice/createLyricVersionSlice";
import { cn } from "@/lib/utils";
import { prefetchHanziChars } from "@/lib/utils/hanzi-strokes";
import { type ILyricLine, ParseLyric } from "@/lib/utils/lyric-parser";
import { resolveLyricLines } from "@/lib/utils/lyric-source";
import { getSidePanelMotion } from "../../lyric-sheet/lyric-sheet-motion";
import { StrokeCharCell } from "./stroke-char";
import {
	computeStrokeCellSize,
	flattenLineChars,
	STROKE_LINE_SCATTER_MS,
	STROKE_STAGE_SAFE_RATIO,
	shouldUseInstantDisplay,
	splitStrokeColumns,
} from "./stroke-columns";

/** 副行（翻译/罗马音）的统一弱化样式 */
const SUB_TEXT_STYLE: CSSProperties = {
	color: "rgb(255 255 255 / calc(40% * var(--lyric-alpha-scale, 1)))",
};

/** 切句等待旧句动画的上限（ms）：兜底防失控（预算适配正常时等待 ≈ 0） */
const STROKE_SWITCH_WAIT_MAX_MS = 2000;

interface StrokeLyricViewProps {
	className?: string;
	showTrans: boolean;
	showRoma: boolean;
}

/** 整句展示状态：index=展示中的句子，phase=in 进入（飞入/直接显示）/ out 句尾散开 */
interface DisplayState {
	index: number;
	phase: "in" | "out";
}

/**
 * 笔画歌词动效（可切换的第二种歌词页动效）：
 *
 * 进入 + 散开两段式：句子切入时逐字按笔画飞入拼合（字的时机跟播放进度
 * 走，triggerAt 驱动；急促连唱的行免飞入、整句直接显示），句播放结束
 * （下一句开始）时整句笔画四散飞开并渐隐至完全消失，散开结束后再进入
 * 下一句——先进入后散开，句间衔接自然。
 *
 * 混合与入场与默认动效同构：mixBlendMode plus-lighter 挂在根元素上，
 * 入场动画作用于混合元素自身（祖先链保持无 transform/filter 隔离，
 * 见 lyric.tsx 顶部注释）。
 */
export function StrokeLyricView({
	className,
	showTrans,
	showRoma,
}: StrokeLyricViewProps) {
	const reduceMotion = Boolean(useReducedMotion());
	const currentSongLyrics = usePlayerStore((s) => s.currentSongLyrics);
	// 手动选择的歌词版本优先级最高，其次本地歌词文件，再是在线歌词
	const manualVersionLines = usePlayerStore(selectActiveLyricVersionLines);
	const hasManualVersion = usePlayerStore(selectActiveLyricVersion) !== null;
	const activeLocalLyric = usePlayerStore(selectActiveLocalLyric);
	// 歌词偏移（按歌曲持久化）：只进时钟，不改歌词时间戳
	const lyricOffsetMs = usePlayerStore(selectCurrentLyricOffset);
	const seek = usePlayerStore((s) => s.seek);
	const duration = usePlayerStore((s) => s.duration);

	// 与默认动效同一条解析链路（resolveLyricLines 收口）
	const lyricResolution = useMemo(
		() =>
			resolveLyricLines({
				manualLines: manualVersionLines,
				localLines: activeLocalLyric?.lines,
				verbatimSource: currentSongLyrics?.yrc?.lyric,
				plainSource: currentSongLyrics?.lrc?.lyric,
			}),
		[manualVersionLines, activeLocalLyric, currentSongLyrics],
	);
	const lyric = lyricResolution.lines;

	// 翻译 / 罗马音分轨：手动版本与本地歌词没有附加轨，激活时不再叠加
	const transMap = useMemo(() => {
		const map = new Map<number, ILyricLine>();
		if (hasManualVersion || activeLocalLyric) return map;
		const sub =
			ParseLyric(currentSongLyrics?.ytlrc?.lyric) ||
			ParseLyric(currentSongLyrics?.tlyric?.lyric) ||
			[];
		for (const t of sub) map.set(t.lineStart, t);
		return map;
	}, [hasManualVersion, activeLocalLyric, currentSongLyrics]);

	const romaMap = useMemo(() => {
		const map = new Map<number, ILyricLine>();
		if (hasManualVersion || activeLocalLyric) return map;
		const sub =
			ParseLyric(currentSongLyrics?.yromalrc?.lyric) ||
			ParseLyric(currentSongLyrics?.romalrc?.lyric) ||
			[];
		for (const r of sub) map.set(r.lineStart, r);
		return map;
	}, [hasManualVersion, activeLocalLyric, currentSongLyrics]);

	// 句级索引跟踪（绕过 Zustand 直订 corePlayer；歌词偏移只参与句判定）。
	// 同时维护歌词时间基准 ref：逐字飞入的触发时刻以它为「现在」
	const [currentIndex, setCurrentIndex] = useState(-1);
	const lyricNowRef = useRef(0);
	const getLyricNowMs = useCallback(() => lyricNowRef.current, []);
	useEffect(() => {
		const unsubscribe = corePlayer.subscribeTime((t) => {
			const lyricTimeMs = t * 1000 + lyricOffsetMs;
			lyricNowRef.current = lyricTimeMs;
			let idx = -1;
			for (let i = lyric.length - 1; i >= 0; i--) {
				if (lyric[i].lineStart <= lyricTimeMs && lyric[i].lineStart >= 0) {
					idx = i;
					break;
				}
			}
			setCurrentIndex((prev) => (prev !== idx ? idx : prev));
		});
		return unsubscribe;
	}, [lyric, lyricOffsetMs]);

	// 整句展示状态机：句切换时先散开旧句，散开结束后再直接显示新句
	const [display, setDisplay] = useState<DisplayState>({
		index: -1,
		phase: "in",
	});
	const displayRef = useRef(display);
	displayRef.current = display;
	const pendingIndexRef = useRef(-1);
	const swapTimerRef = useRef<number | null>(null);
	// 旧句飞入动画的最晚完成时刻（performance.now 时钟，cell 上报取最大
	// 值）：切句散开前等它到期——动画播完才切换，不被切句截断
	const flyEndAtRef = useRef(0);
	const handleFlyEndAt = useCallback((endAtMs: number) => {
		flyEndAtRef.current = Math.max(flyEndAtRef.current, endAtMs);
	}, []);

	useEffect(() => {
		const cur = displayRef.current;
		if (currentIndex === cur.index) {
			// 跳回同一句（seek 回）：取消待切定时器；已散开则复位重进本句
			//（否则笔画停在散开后的不可见态），仍在等待期则保持进入态
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
			// 初始挂载：无需散开，直接进入
			setDisplay({ index: currentIndex, phase: "in" });
			return;
		}
		if (cur.phase === "out") {
			// 散开已在进行：把散开结束后的落点改道到最新句
			pendingIndexRef.current = currentIndex;
			return;
		}
		pendingIndexRef.current = currentIndex;
		// 先等旧句飞入动画全部播完（cell 上报完成时刻，封顶防失控），
		// 再散开旧句、进入新句——动画不因切句被截断；等待期间保持旧句
		// 展示（display 独立于 currentIndex，播放照常推进）
		const waitMs = Math.min(
			Math.max(0, flyEndAtRef.current - performance.now()),
			STROKE_SWITCH_WAIT_MAX_MS,
		);
		if (swapTimerRef.current !== null) clearTimeout(swapTimerRef.current);
		swapTimerRef.current = window.setTimeout(() => {
			swapTimerRef.current = null;
			setDisplay({ index: displayRef.current.index, phase: "out" });
			swapTimerRef.current = window.setTimeout(() => {
				swapTimerRef.current = null;
				// 新句进入：旧句动画完成记录作废（下一轮等待按新句计算）
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

	// 预取下一句笔画数据（基于真实播放进度，让切句时数据大概率已在缓存）
	const nextLine =
		currentIndex >= 0 && currentIndex + 1 < lyric.length
			? lyric[currentIndex + 1]
			: undefined;
	useEffect(() => {
		if (nextLine && !nextLine.isLeadDots) prefetchHanziChars(nextLine.lineText);
	}, [nextLine]);

	// 展示中的句子（散开期间仍保持旧句直到散开结束）
	const displayLine = display.index >= 0 ? lyric[display.index] : undefined;
	// 展示句的时间窗口（ms）：到下一句切句点为止（末行无下一句给长窗口
	// 兜底；窗口倒挂的异常数据也保底 600ms）。减去 120ms 尾部余量（留出
	// 落位与句尾散开的缓冲）与末字触发偏移后，即整句飞入动画的预算
	const displayNextLine =
		display.index + 1 < lyric.length ? lyric[display.index + 1] : undefined;
	const lineWindowMs =
		displayLine && displayNextLine
			? Math.max(600, displayNextLine.lineStart - displayLine.lineStart)
			: 5000;

	// 舞台高度测量（窗口缩放 / 开合播放列表时重算单元格尺寸）
	const stageRef = useRef<HTMLDivElement | null>(null);
	const [stageHeight, setStageHeight] = useState(0);
	useEffect(() => {
		const el = stageRef.current;
		if (!el) return;
		const update = () => setStageHeight(el.clientHeight);
		update();
		const ro = new ResizeObserver(update);
		ro.observe(el);
		return () => ro.disconnect();
	}, []);

	// 逐字时序摊平 + 对联式拆列（前半句靠上、后半句靠下）
	const timings = useMemo(
		() =>
			displayLine && !displayLine.isLeadDots
				? flattenLineChars(displayLine)
				: [],
		[displayLine],
	);
	// 急促连唱的行免飞入（飞入跟不上演唱节奏），整句直接显示
	const instantLine = useMemo(
		() => shouldUseInstantDisplay(timings),
		[timings],
	);
	const columns = useMemo(() => splitStrokeColumns(timings), [timings]);

	// 句级飞入预算：句窗口 − 120ms 尾部余量 − 末字触发偏移，传给每个字
	// 求同一条时间线（句级统一缩放，节奏均匀、字间过渡平滑）；追赶场景
	// 由 stroke-char 按已流逝量再压，保证切句前播完
	const lastCharOffsetMs =
		timings.length > 0
			? timings[timings.length - 1].triggerAt - (displayLine?.lineStart ?? 0)
			: 0;
	const flyBudgetMs = Math.max(0, lineWindowMs - 120 - lastCharOffsetMs);

	const maxColLength = Math.max(...columns.map((c) => c.items.length), 1);
	const cellSize = computeStrokeCellSize(stageHeight, maxColLength);
	const cellGap = Math.max(6, Math.round(cellSize * 0.16));
	const columnGap = Math.max(28, Math.round(cellSize * 0.5));
	// 双列上下安全区：给副行与画面呼吸让位，避免内容贴边
	const safePad = Math.round(stageHeight * STROKE_STAGE_SAFE_RATIO);

	const handleSeekToCurrentLine = () => {
		if (!displayLine || duration <= 0) return;
		seek((displayLine.lineStart / (duration * 1000)) * 100);
	};

	const transLine =
		displayLine && showTrans ? transMap.get(displayLine.lineStart) : undefined;
	const romaLine =
		displayLine && showRoma ? romaMap.get(displayLine.lineStart) : undefined;

	return (
		<motion.div
			className={cn("h-full w-full relative", className)}
			// 入场动效与混合模式同默认歌词视图：动画作用于混合元素自身，
			// plus-lighter 从第一帧起就保持生效
			{...getSidePanelMotion(reduceMotion)}
			style={{
				mixBlendMode: "plus-lighter",
				// 歌词字体与默认动效同源（--app-lyric-font-family），副行一并继承
				fontFamily: "var(--app-lyric-font-family, inherit)",
			}}
		>
			<div ref={stageRef} className="h-full w-full relative select-none">
				{/* 翻译 / 罗马音副行（弱化，置于底部安全区内） */}
				{(transLine || romaLine) && (
					<div className="absolute inset-x-0 bottom-[10%] flex flex-col items-center gap-1.5 px-8 text-center pointer-events-none">
						{transLine && (
							<motion.span
								key={`trans-${displayLine?.lineStart}`}
								initial={{ opacity: 0 }}
								animate={{ opacity: 1 }}
								className="truncate text-base font-medium"
								style={SUB_TEXT_STYLE}
							>
								{transLine.lineText}
							</motion.span>
						)}
						{romaLine && (
							<motion.span
								key={`roma-${displayLine?.lineStart}`}
								initial={{ opacity: 0 }}
								animate={{ opacity: 1 }}
								className="truncate text-base font-medium"
								style={SUB_TEXT_STYLE}
							>
								{romaLine.lineText}
							</motion.span>
						)}
					</div>
				)}

				{/* 中央多列舞台（1~3 列，按字数自适应）：逐字飞入或整句直接显示，句尾散开由相位驱动 */}
				<div className="absolute inset-0 flex items-center justify-center">
					{displayLine && !displayLine.isLeadDots && columns.length > 0 ? (
						<motion.div
							key={`line-${displayLine.lineStart}-${displayLine.lineText}`}
							className="flex h-full w-full cursor-pointer items-stretch justify-center"
							style={{ gap: columnGap }}
							onClick={handleSeekToCurrentLine}
						>
							{columns.map((column, ci) => (
								<div
									// biome-ignore lint/suspicious/noArrayIndexKey: 列布局句内静态不重排，索引即稳定标识
									key={`col-${ci}`}
									className="flex h-full flex-col items-center"
									style={{
										justifyContent:
											column.align === "start"
												? "flex-start"
												: column.align === "end"
													? "flex-end"
													: "center",
										gap: cellGap,
										paddingTop: column.align === "start" ? safePad : undefined,
										paddingBottom: column.align === "end" ? safePad : undefined,
									}}
								>
									{column.items.map((t, i) => (
										<StrokeCharCell
											// biome-ignore lint/suspicious/noArrayIndexKey: 列内字符列表静态不重排，索引即稳定标识
											key={`col-${ci}-${i}-${t.char}`}
											char={t.char}
											cellSize={cellSize}
											phase={display.phase}
											enter={instantLine ? "instant" : "fly"}
											triggerAt={t.triggerAt}
											flyBudgetMs={flyBudgetMs}
											onFlyEndAt={handleFlyEndAt}
											getNowMs={getLyricNowMs}
										/>
									))}
								</div>
							))}
						</motion.div>
					) : displayLine?.isLeadDots ? (
						<div className="flex h-full w-full items-center justify-center">
							<StrokeLeadDots />
						</div>
					) : null}
				</div>
			</div>
		</motion.div>
	);
}

/** 过门（无演唱行）的呼吸圆点，替代默认动效的 LyricLeadDots */
function StrokeLeadDots() {
	return (
		<div className="flex items-center justify-center gap-3">
			{[0, 1, 2].map((i) => (
				<motion.span
					key={i}
					className="rounded-full bg-white/55"
					style={{
						width: "calc(0.8rem * var(--app-lyric-scale, 1))",
						height: "calc(0.8rem * var(--app-lyric-scale, 1))",
					}}
					animate={{ opacity: [0.25, 0.8, 0.25] }}
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
