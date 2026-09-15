import { describe, expect, it } from "vitest";
import type { ILyricLine } from "@/lib/utils/lyric-parser";
import {
	computeCharIntervalMs,
	computeStrokeCellSize,
	exitDistancePx,
	FLY_SCALE_MIN,
	FLY_TAIL_MIN_MS,
	FLY_TAIL_MS,
	fitFlyTimeline,
	flattenLineChars,
	STROKE_CELL_MAX_PX,
	STROKE_CELL_MIN_PX,
	STROKE_FLY_WINDOW_MS,
	STROKE_INSTANT_CHAR_BUDGET_MS,
	shouldUseInstantDisplay,
	splitStrokeColumns,
} from "./stroke-columns";

function line(overrides: Partial<ILyricLine> = {}): ILyricLine {
	return { lineStart: 1000, lineText: "明月几时有", ...overrides };
}

describe("flattenLineChars", () => {
	it("逐字歌词：词元内逐字摊平，触发点在词元时长内均分", () => {
		const timings = flattenLineChars(
			line({
				lineText: "明月",
				words: [
					{ startTime: 1000, duration: 400, char: "明" },
					{ startTime: 1400, duration: 600, char: "月" },
				],
			}),
		);
		expect(timings).toEqual([
			{ char: "明", triggerAt: 1000, spanMs: 400 },
			{ char: "月", triggerAt: 1400, spanMs: 600 },
		]);
	});

	it("逐字歌词：多字词元（英文单词）逐字错开而不是整词同闪", () => {
		const timings = flattenLineChars(
			line({
				lineText: "love",
				words: [{ startTime: 2000, duration: 400, char: "love" }],
			}),
		);
		expect(timings.map((t) => t.char)).toEqual(["l", "o", "v", "e"]);
		expect(timings[0].triggerAt).toBe(2000);
		expect(timings[3].triggerAt).toBe(2300);
	});

	it("逐字歌词：词元内空白字符不产生触发点", () => {
		const timings = flattenLineChars(
			line({
				lineText: "a b",
				words: [
					{ startTime: 0, duration: 200, char: "a" },
					{ startTime: 200, duration: 200, char: " " },
					{ startTime: 400, duration: 200, char: "b" },
				],
			}),
		);
		expect(timings.map((t) => t.char)).toEqual(["a", "b"]);
	});

	it("逐行歌词：从 lineStart 起按行内均分布进触发点", () => {
		const timings = flattenLineChars(line({ lineText: "明月几时有" }));
		expect(timings).toHaveLength(5);
		expect(timings[0].triggerAt).toBe(1000);
		// 均分步长落在 [140, 420] 内
		const step = timings[1].triggerAt - timings[0].triggerAt;
		expect(step).toBeGreaterThanOrEqual(140);
		expect(step).toBeLessThanOrEqual(420);
	});

	it("逐行歌词：有 lineEnd 时按真实行时长均分（步长封顶 420ms）", () => {
		const timings = flattenLineChars(line({ lineText: "ab", lineEnd: 3000 }));
		expect(timings[0].triggerAt).toBe(1000);
		// 行时长 2000ms / 2 字 = 1000ms，但步长按设计封顶 420ms
		expect(timings[1].triggerAt).toBe(1420);
	});

	it("空白行返回空数组", () => {
		expect(flattenLineChars(line({ lineText: "  " }))).toEqual([]);
	});
});

describe("splitStrokeColumns", () => {
	const chars = (n: number) =>
		Array.from({ length: n }, (_, i) => ({
			char: String(i),
			triggerAt: i,
			spanMs: 100,
		}));

	it("短句（≤6 字）：单列居中", () => {
		const cols = splitStrokeColumns(chars(5));
		expect(cols).toHaveLength(1);
		expect(cols[0].align).toBe("center");
		expect(cols[0].items).toHaveLength(5);
		expect(cols[0].items[0].char).toBe("0");
	});

	it("中句（7~12 字）：两列上下交错，前列多一字", () => {
		const cols = splitStrokeColumns(chars(7));
		expect(cols).toHaveLength(2);
		expect(cols[0].align).toBe("start");
		expect(cols[1].align).toBe("end");
		expect(cols[0].items.map((c) => c.char)).toEqual(["0", "1", "2", "3"]);
		expect(cols[1].items.map((c) => c.char)).toEqual(["4", "5", "6"]);
	});

	it("长句（>12 字）：三列上限，交错为 上/下/上", () => {
		const cols = splitStrokeColumns(chars(13));
		expect(cols).toHaveLength(3);
		expect(cols.map((c) => c.align)).toEqual(["start", "end", "start"]);
		expect(cols.map((c) => c.items.length)).toEqual([5, 5, 3]);
		expect(cols[2].items[2].char).toBe("12");
	});

	it("空输入返回空数组", () => {
		expect(splitStrokeColumns([])).toEqual([]);
	});
});

describe("computeCharIntervalMs", () => {
	it("逐字歌词：末字与首字触发点之差 ÷（字数−1）", () => {
		const interval = computeCharIntervalMs([
			{ char: "明", triggerAt: 1000, spanMs: 300 },
			{ char: "月", triggerAt: 1300, spanMs: 300 },
			{ char: "几", triggerAt: 1600, spanMs: 300 },
		]);
		expect(interval).toBe(300);
	});

	it("逐行歌词：triggerAt 由 stagger 均摊，间隔即均摊步长", () => {
		const timings = flattenLineChars(line({ lineText: "明月几时有" }));
		const interval = computeCharIntervalMs(timings);
		expect(interval).toBe(timings[1].triggerAt - timings[0].triggerAt);
	});

	it("单字行取该字 span", () => {
		expect(
			computeCharIntervalMs([{ char: "明", triggerAt: 1000, spanMs: 480 }]),
		).toBe(480);
	});

	it("空行返回 Infinity（永不判快）", () => {
		expect(computeCharIntervalMs([])).toBe(Number.POSITIVE_INFINITY);
	});

	it("triggerAt 全行同刻（数据异常）退回 span 均值", () => {
		const interval = computeCharIntervalMs([
			{ char: "明", triggerAt: 1000, spanMs: 200 },
			{ char: "月", triggerAt: 1000, spanMs: 400 },
		]);
		expect(interval).toBe(300);
	});
});

describe("shouldUseInstantDisplay", () => {
	const paced = (interval: number, count: number) =>
		Array.from({ length: count }, (_, i) => ({
			char: String(i),
			triggerAt: i * interval,
			spanMs: interval,
		}));

	it("急促连唱（间隔 < 250ms/字）：免飞入整句直接显示", () => {
		expect(shouldUseInstantDisplay(paced(150, 8))).toBe(true);
		expect(shouldUseInstantDisplay(paced(249, 8))).toBe(true);
	});

	it("中速一字一拍（300~700ms/字）：正常飞入", () => {
		expect(shouldUseInstantDisplay(paced(300, 6))).toBe(false);
		expect(shouldUseInstantDisplay(paced(500, 6))).toBe(false);
		expect(shouldUseInstantDisplay(paced(700, 6))).toBe(false);
	});

	it("阈值边界（恰好 250）：不判快、正常飞入", () => {
		expect(
			shouldUseInstantDisplay(paced(STROKE_INSTANT_CHAR_BUDGET_MS, 4)),
		).toBe(false);
	});

	it("单字行按 span 判定", () => {
		expect(
			shouldUseInstantDisplay([{ char: "明", triggerAt: 0, spanMs: 200 }]),
		).toBe(true);
		expect(
			shouldUseInstantDisplay([{ char: "明", triggerAt: 0, spanMs: 320 }]),
		).toBe(false);
	});

	it("空行不判快", () => {
		expect(shouldUseInstantDisplay([])).toBe(false);
	});
});

describe("fitFlyTimeline（句级飞入时间线适配）", () => {
	it("预算充足：不压缩，返回自然时间线（提速段 + 收尾段）", () => {
		expect(fitFlyTimeline(3000)).toEqual({
			scale: 1,
			fastMs: 1240,
			tailMs: 260,
		});
	});

	it("等比段：提速段与收尾段同比例压缩，总时长贴合预算", () => {
		const t = fitFlyTimeline(1200);
		expect(t.scale).toBeCloseTo(0.8, 5);
		expect(t.fastMs).toBeCloseTo(992, 5);
		expect(t.tailMs).toBeCloseTo(208, 5);
	});

	it("收尾触保底：收尾固定不压缩，提速段吃剩余预算", () => {
		const t = fitFlyTimeline(1000);
		expect(t.tailMs).toBe(FLY_TAIL_MIN_MS);
		expect(t.fastMs + t.tailMs).toBeCloseTo(1000, 5);
	});

	it("两段解边界连续：分界两侧总时长都贴合预算（字间速度无突兀）", () => {
		const boundary = STROKE_FLY_WINDOW_MS * (FLY_TAIL_MIN_MS / FLY_TAIL_MS);
		const below = fitFlyTimeline(boundary - 1);
		const above = fitFlyTimeline(boundary + 1);
		expect(below.fastMs + below.tailMs).toBeCloseTo(boundary - 1, 0);
		expect(above.fastMs + above.tailMs).toBeCloseTo(boundary + 1, 0);
	});

	it("极端小预算：压缩钳在下限（散开动画从当前位姿平滑接续兜底）", () => {
		expect(fitFlyTimeline(0).scale).toBe(FLY_SCALE_MIN);
		expect(fitFlyTimeline(-100).scale).toBe(FLY_SCALE_MIN);
	});
});

// 页面矩形 (0,0)-(1000,800)，字格中心 (500,400)
const EXIT_RECT = { left: 0, top: 0, right: 1000, bottom: 800 };
const EXIT_CX = 500;
const EXIT_CY = 400;

describe("exitDistancePx（飞入起点的出页面距离）", () => {
	const d = (cx: number, cy: number, dx: number, dy: number) =>
		exitDistancePx(
			cx,
			cy,
			dx,
			dy,
			EXIT_RECT.left,
			EXIT_RECT.top,
			EXIT_RECT.right,
			EXIT_RECT.bottom,
		);

	it("正方向：沿轴向飞到右边界", () => {
		expect(d(EXIT_CX, EXIT_CY, 1, 0)).toBe(500);
	});

	it("负方向：沿轴向飞到左/上边界", () => {
		expect(d(EXIT_CX, EXIT_CY, -1, 0)).toBe(500);
		expect(d(EXIT_CX, EXIT_CY, 0, -1)).toBe(400);
	});

	it("斜向：取 x/y 两轴先出界的那一段", () => {
		// 单位方向 (0.6, -0.8)：tx≈833，ty=500 → 先撞上边界
		expect(d(EXIT_CX, EXIT_CY, 0.6, -0.8)).toBeCloseTo(500, 5);
	});

	it("起点在页面外且方向朝外：兜底 600（保证起点在页面外）", () => {
		expect(d(1200, 400, 1, 0)).toBe(600);
	});

	it("起点在页面外但方向朝内：正常求交", () => {
		expect(d(1200, 400, -1, 0)).toBe(200);
	});

	it("零向量（退化方向）：兜底 600", () => {
		expect(d(EXIT_CX, EXIT_CY, 0, 0)).toBe(600);
	});
});

describe("computeStrokeCellSize", () => {
	it("长列时收缩单元格但不低于下限", () => {
		// 700 * 0.68 = 476；476 / 20 = 23.8 → 收敛到下限
		expect(computeStrokeCellSize(700, 20)).toBe(STROKE_CELL_MIN_PX);
	});

	it("短列时单元格不超过上限", () => {
		expect(computeStrokeCellSize(900, 1)).toBe(STROKE_CELL_MAX_PX);
	});

	it("中间态超过上限时收敛到上限（字号整体缩小后更常触顶）", () => {
		// 700 * 0.68 = 476；476 / 7 = 68 > 上限 46 → 收敛到上限
		expect(computeStrokeCellSize(700, 7)).toBe(STROKE_CELL_MAX_PX);
	});

	it("舞台未测量（0）时给上限兜底", () => {
		expect(computeStrokeCellSize(0, 5)).toBe(STROKE_CELL_MAX_PX);
	});
});
