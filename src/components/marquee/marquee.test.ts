import { describe, expect, it } from "vitest";
import { computeMarqueeState } from "./marquee-metrics";

/**
 * 底栏文字抖动回归：溢出判定必须只依赖「文本自然宽度 + 容器可用宽度」，
 * 且同样的输入恒得同样的输出（判据与动画状态解耦）。
 *
 * 真实抖动（实测 ~19Hz 翻转）来自「判据 ↔ 副本挂载」互馈：副本会把容器
 * flex 基准宽度翻倍，容器一变宽判据就翻面。修复后副本绝对定位、宽度由独立
 * 量算节点提供，因此这里锁定的不变量是：**判据是纯函数，无内部状态**。
 */
describe("computeMarqueeState", () => {
	const base = { availableWidth: 210, reducedMotion: false, loopGapPx: 32 };

	it("放得下则不滚动", () => {
		expect(computeMarqueeState({ ...base, textWidth: 160 }).shouldAnimate).toBe(
			false,
		);
		expect(computeMarqueeState({ ...base, textWidth: 210 }).shouldAnimate).toBe(
			false,
		);
	});

	it("1px 容差内的贴边文本不触发滚动（防边界抖动）", () => {
		expect(
			computeMarqueeState({ ...base, textWidth: 210.5 }).shouldAnimate,
		).toBe(false);
		expect(computeMarqueeState({ ...base, textWidth: 211 }).shouldAnimate).toBe(
			false,
		);
		expect(computeMarqueeState({ ...base, textWidth: 212 }).shouldAnimate).toBe(
			true,
		);
	});

	it("未量算（宽度 0 或负）不滚动", () => {
		expect(computeMarqueeState({ ...base, textWidth: 0 }).shouldAnimate).toBe(
			false,
		);
		expect(computeMarqueeState({ ...base, textWidth: -5 }).shouldAnimate).toBe(
			false,
		);
	});

	it("系统减动效偏好下永不滚动", () => {
		expect(
			computeMarqueeState({ ...base, textWidth: 800, reducedMotion: true })
				.shouldAnimate,
		).toBe(false);
	});

	it("时长按「文本宽 + 循环间距」换算巡航速度", () => {
		// (300 + 32) / 50 × 3.192
		expect(
			computeMarqueeState({ ...base, textWidth: 300 }).duration,
		).toBeCloseTo((332 / 50) * 3.192, 4);
	});

	it("同输入恒同输出——判据无状态，不存在「判据↔挂载」互馈", () => {
		const input = { ...base, textWidth: 240 };
		const first = computeMarqueeState(input);
		// 模拟历史上会出现的一轮翻转：容器因副本变宽后再次判定
		const widened = computeMarqueeState({ ...input, availableWidth: 320 });
		const again = computeMarqueeState(input);
		expect(again).toEqual(first);
		expect(widened.shouldAnimate).toBe(false);
		expect(first.shouldAnimate).toBe(true);
	});
});
