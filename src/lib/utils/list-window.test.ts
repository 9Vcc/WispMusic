import { describe, expect, it } from "vitest";
import { computeListWindow } from "./list-window";

const ITEM_H = 32;

/** 窗口化后「占位 + 已渲染行」的总高必须恒等于整表高度 */
function assertTotalHeight(options: {
	total: number;
	itemHeight: number;
	scrollTop: number;
	viewportHeight: number;
	overscan?: number;
}) {
	const window = computeListWindow(options);
	const filled =
		window.topPad + (window.end - window.start) * options.itemHeight + window.bottomPad;
	expect(filled).toBe(options.total * options.itemHeight);
	return window;
}

describe("computeListWindow", () => {
	it("空列表返回空窗口", () => {
		expect(
			computeListWindow({
				total: 0,
				itemHeight: ITEM_H,
				scrollTop: 0,
				viewportHeight: 400,
			}),
		).toEqual({ start: 0, end: 0, topPad: 0, bottomPad: 0 });
	});

	it("列表比视口短时全部渲染且无占位", () => {
		const window = assertTotalHeight({
			total: 5,
			itemHeight: ITEM_H,
			scrollTop: 0,
			viewportHeight: 400,
		});
		expect(window).toEqual({ start: 0, end: 5, topPad: 0, bottomPad: 0 });
	});

	it("滚到中间时只渲染视口附近的行，总高不变", () => {
		const window = assertTotalHeight({
			total: 1000,
			itemHeight: ITEM_H,
			scrollTop: 3200,
			viewportHeight: 400,
			overscan: 10,
		});
		// 首个可见行 = 3200 / 32 = 100
		expect(window.start).toBe(90);
		expect(window.end).toBe(124);
		expect(window.end - window.start).toBeLessThan(40);
		expect(window.topPad).toBe(90 * ITEM_H);
	});

	it("顶部无上占位、底部无下占位", () => {
		const top = assertTotalHeight({
			total: 1000,
			itemHeight: ITEM_H,
			scrollTop: 0,
			viewportHeight: 400,
			overscan: 10,
		});
		expect(top.start).toBe(0);
		expect(top.topPad).toBe(0);

		const bottom = assertTotalHeight({
			total: 1000,
			itemHeight: ITEM_H,
			scrollTop: 1000 * ITEM_H - 400,
			viewportHeight: 400,
			overscan: 10,
		});
		expect(bottom.end).toBe(1000);
		expect(bottom.bottomPad).toBe(0);
	});

	it("滚动越界或尺寸异常时不会产生越界窗口", () => {
		for (const scrollTop of [-500, 999999, Number.NaN]) {
			for (const viewportHeight of [0, -10, Number.NaN]) {
				const window = assertTotalHeight({
					total: 10,
					itemHeight: ITEM_H,
					scrollTop,
					viewportHeight,
					overscan: 4,
				});
				expect(window.start).toBeGreaterThanOrEqual(0);
				expect(window.end).toBeLessThanOrEqual(10);
				expect(window.end).toBeGreaterThan(window.start);
			}
		}
	});

	it("视口高度为 0 时仍至少渲染一行，避免展开瞬间空白", () => {
		const window = computeListWindow({
			total: 100,
			itemHeight: ITEM_H,
			scrollTop: 0,
			viewportHeight: 0,
		});
		expect(window.end - window.start).toBe(1);
	});

	it("行高非法时按空窗口处理，不抛错", () => {
		expect(
			computeListWindow({
				total: 10,
				itemHeight: 0,
				scrollTop: 0,
				viewportHeight: 400,
			}),
		).toEqual({ start: 0, end: 0, topPad: 0, bottomPad: 0 });
	});
});
