import { useEffect, useRef, useState } from "react";
import { isIndexInPair } from "./desktop-lyric-pair";

/**
 * 桌面歌词「配对轮换」状态机（横向双行 / 竖排双列、默认动效 / 笔画动效共用）。
 *
 * 语义（见 desktop-lyric-pair.ts）：配对固定挂载，**第一句播完再播第二句**
 * （两格各自在自己的演唱窗口内做动画），当前句在配对内推进不换对；只有当前句
 * 推进到配对之外才整对轮换——先整对退出（exitMs），再换进以新当前句为首的新配对。
 *
 * 两种动效的差异通过参数吸收：
 * - 退出前是否要等动画播完：笔画动效要等字格飞入结束（getPendingWaitMs），
 *   默认动效的退出本身就是 CSS 过渡，无需等待；
 * - 退出时长：笔画用散开时长，默认动效用行槽滑动时长。
 */
export function useLyricPair({
	currentIndex,
	exitMs,
	getPendingWaitMs,
	maxWaitMs = 2000,
}: {
	currentIndex: number;
	/** 整对退出时长（散开 / 滑出）：退出结束后才换进新配对 */
	exitMs: number;
	/** 退出前的额外等待（毫秒，如笔画飞入未完成的剩余时间）；缺省不等 */
	getPendingWaitMs?: () => number;
	/** 额外等待的上限，兜底防失控 */
	maxWaitMs?: number;
}): { pairFirst: number; phase: "in" | "out" } {
	const [pairFirst, setPairFirst] = useState(-1);
	const [phase, setPhase] = useState<"in" | "out">("in");
	const firstRef = useRef(pairFirst);
	const phaseRef = useRef(phase);
	firstRef.current = pairFirst;
	phaseRef.current = phase;
	const pendingFirstRef = useRef(-1);
	const timerRef = useRef<number | null>(null);

	useEffect(() => {
		if (isIndexInPair(currentIndex, firstRef.current)) {
			// 跳回配对内（seek 回）：取消待轮换定时器；已退出则复位重进本对
			if (firstRef.current !== -1 && timerRef.current !== null) {
				clearTimeout(timerRef.current);
				timerRef.current = null;
				if (phaseRef.current === "out") setPhase("in");
			}
			return;
		}
		if (firstRef.current === -1) {
			setPairFirst(currentIndex);
			setPhase("in");
			return;
		}
		if (phaseRef.current === "out") {
			pendingFirstRef.current = currentIndex;
			return;
		}
		pendingFirstRef.current = currentIndex;
		const waitMs = Math.min(Math.max(0, getPendingWaitMs?.() ?? 0), maxWaitMs);
		if (timerRef.current !== null) clearTimeout(timerRef.current);
		timerRef.current = window.setTimeout(() => {
			timerRef.current = null;
			setPhase("out");
			timerRef.current = window.setTimeout(() => {
				timerRef.current = null;
				setPairFirst(pendingFirstRef.current);
				setPhase("in");
			}, exitMs);
		}, waitMs);
	}, [currentIndex, exitMs, maxWaitMs, getPendingWaitMs]);

	// 卸载时清理轮换定时器
	useEffect(() => {
		return () => {
			if (timerRef.current !== null) clearTimeout(timerRef.current);
		};
	}, []);

	return { pairFirst, phase };
}
