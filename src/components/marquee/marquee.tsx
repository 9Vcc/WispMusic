import {
	useCallback,
	useEffect,
	useLayoutEffect,
	useRef,
	useState,
} from "react";
import { cn } from "@/lib/utils";
import styles from "./marquee.module.css";
import {
	computeMarqueeState,
	MARQUEE_HOLD_MS,
	MARQUEE_LOOP_GAP_PX,
} from "./marquee-metrics";

const FADE_WIDTH = "24px";

export function Marquee({
	text,
	textClassName,
	containerClassName,
}: {
	text: string;
	textClassName?: string;
	containerClassName?: string;
}) {
	const containerRef = useRef<HTMLDivElement>(null);
	const measureRef = useRef<HTMLSpanElement>(null);
	const holdTimerRef = useRef<number>(undefined);
	const [shouldAnimate, setShouldAnimate] = useState(false);
	const [scrolling, setScrolling] = useState(false);
	const [duration, setDuration] = useState(30);

	// 量算 + 复位：走 useLayoutEffect，切歌换文本时首帧就是正确状态
	//（useEffect 会先画一帧旧状态再纠正，观感上就是「闪一下」）
	// biome-ignore lint/correctness/useExhaustiveDependencies: text 是刻意保留的重算触发源——量算读的是 DOM 实测宽度，正文里不出现 text，但文本一换必须重新量算与复位
	useLayoutEffect(() => {
		const container = containerRef.current;
		const measureEl = measureRef.current;
		if (!container || !measureEl) return;

		// 文本已换：立即停掉上一轮的滚动动画与停留定时器，
		// 回到起点重新计时（否则旧动画会带着新文本继续跑）
		setScrolling(false);
		window.clearTimeout(holdTimerRef.current);

		const reducedMotion = window.matchMedia(
			"(prefers-reduced-motion: reduce)",
		).matches;

		const checkOverflow = () => {
			const { shouldAnimate: next, duration: nextDuration } =
				computeMarqueeState({
					textWidth: measureEl.getBoundingClientRect().width,
					availableWidth: container.clientWidth,
					reducedMotion,
					loopGapPx: MARQUEE_LOOP_GAP_PX,
				});
			setShouldAnimate(next);
			if (nextDuration > 0) setDuration(nextDuration);
		};

		checkOverflow();
		// 容器宽度只随布局变化（不再随动画状态变化），因此在窗口缩放时才回调
		const observer = new ResizeObserver(checkOverflow);
		observer.observe(container);
		return () => observer.disconnect();
	}, [text]);

	const scheduleScroll = useCallback(() => {
		window.clearTimeout(holdTimerRef.current);
		holdTimerRef.current = window.setTimeout(
			() => setScrolling(true),
			MARQUEE_HOLD_MS,
		);
	}, []);

	// 起点停留 → 滚动一轮 → 回到起点停留的循环
	// biome-ignore lint/correctness/useExhaustiveDependencies: text 变化意味着「上一轮动画已作废」，必须重新排停留定时器（滚动类名与定时器一起被复位）
	useEffect(() => {
		if (!shouldAnimate) {
			window.clearTimeout(holdTimerRef.current);
			return;
		}
		scheduleScroll();
		return () => window.clearTimeout(holdTimerRef.current);
	}, [shouldAnimate, text, scheduleScroll]);

	const handleAnimationEnd = () => {
		// 滚满一组（ROUNDS 轮）后才触发；-100% 与 0 像素级重合，归位不会跳动
		setScrolling(false);
		scheduleScroll();
	};

	return (
		<div
			ref={containerRef}
			className={cn(
				"relative overflow-hidden",
				containerClassName,
				styles.marqueeWrapper,
			)}
			style={
				{
					// 右侧遮罩只要溢出就显示；左侧遮罩仅在滚动时淡入，
					// 停在起点时隐藏，避免盖住行首文字
					"--marquee-fade-right": shouldAnimate ? FADE_WIDTH : "0px",
					"--marquee-fade-left": scrolling ? FADE_WIDTH : "0px",
				} as React.CSSProperties
			}
		>
			{/* 量算节点：绝对定位 + invisible，不参与布局也不可见，
			    因此它的宽度与「副本是否挂载」完全无关 —— 这正是切断
			    量算↔动画反馈环的关键（见 marquee-metrics.ts） */}
			<span
				ref={measureRef}
				aria-hidden
				className={cn(
					"pointer-events-none invisible absolute top-0 left-0 whitespace-nowrap",
					textClassName,
				)}
			>
				{text}
			</span>

			{/* 轨道：恒定只放一份在流内，宽度 = 单份文本 + 循环间距，
			    因此容器的 flex 基准宽度不随滚动状态变化。
			    间距恒定保留才能保证「平移一整个轨道宽 = 恰好接上下一份」
			    的像素级无缝（关键帧按轨道宽的 100% 平移） */}
			<div
				className={cn(
					"relative flex w-max",
					scrolling && styles.marqueeScrolling,
				)}
				style={
					{
						paddingRight: MARQUEE_LOOP_GAP_PX,
						"--marquee-duration": `${duration}s`,
					} as React.CSSProperties
				}
				onAnimationEnd={handleAnimationEnd}
			>
				<span className={cn("whitespace-nowrap", textClassName)}>{text}</span>
				{/* 副本：绝对定位贴着轨道右缘（left:100% 落点在循环间距之后），
				    既保证视觉连续，又不给容器贡献 max-content */}
				{shouldAnimate && (
					<span
						aria-hidden
						className={cn(
							"absolute top-0 left-full whitespace-nowrap",
							textClassName,
						)}
					>
						{text}
					</span>
				)}
			</div>
		</div>
	);
}
