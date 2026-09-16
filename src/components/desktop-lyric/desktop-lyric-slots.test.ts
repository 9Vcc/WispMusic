import { describe, expect, it } from "vitest";
import type { ILyricLine } from "@/lib/utils/lyric-parser";
import { resolveLyricEdge, resolveVerticalSlots } from "./desktop-lyric-slots";

const line = (lineStart: number, lineText: string, isLeadDots = false) =>
	({ lineStart, lineText, isLeadDots }) as ILyricLine;

describe("resolveLyricEdge", () => {
	it("当前句（上行）恒贴 start，下一句（下行）恒贴 end", () => {
		expect(resolveLyricEdge(true)).toBe("start");
		expect(resolveLyricEdge(false)).toBe("end");
	});

	it("与行号奇偶无关（旧实现按 index % 2 交替，是贴边跑偏的根因）", () => {
		// 同一槽位在任意行号下结果一致：上行永远 start
		for (const index of [0, 1, 2, 3, 17, 100]) {
			const isCurrent = index % 2 === 0;
			expect(resolveLyricEdge(isCurrent)).toBe(isCurrent ? "start" : "end");
		}
	});
});

describe("resolveVerticalSlots", () => {
	it("当前句恒左列、下一句恒右列", () => {
		const cur = line(0, "当前句");
		const next = line(6000, "下一句");
		const { left, right } = resolveVerticalSlots({
			line: cur,
			nextLine: next,
			currentIndex: 0,
		});
		expect(left?.line.lineText).toBe("当前句");
		expect(left?.isCurrent).toBe(true);
		expect(right?.line.lineText).toBe("下一句");
		expect(right?.isCurrent).toBe(false);
	});

	it("行号奇偶不影响列归属（奇数句当前行仍在左列）", () => {
		for (const index of [0, 1, 2, 3]) {
			const slots = resolveVerticalSlots({
				line: line(index * 1000, `第${index}句`),
				nextLine: line(index * 1000 + 1000, `第${index + 1}句`),
				currentIndex: index,
			});
			expect(slots.left?.line.lineText).toBe(`第${index}句`);
			expect(slots.right?.line.lineText).toBe(`第${index + 1}句`);
		}
	});

	it("末句无下一句时右列留空", () => {
		const { left, right } = resolveVerticalSlots({
			line: line(0, "末句"),
			nextLine: undefined,
			currentIndex: 5,
		});
		expect(left?.isCurrent).toBe(true);
		expect(right).toBeUndefined();
	});

	it("过门圆点行 / 未定位时不占列（交给呼吸圆点）", () => {
		expect(
			resolveVerticalSlots({
				line: line(0, "···", true),
				nextLine: line(1000, "下一句"),
				currentIndex: 1,
			}),
		).toEqual({});
		expect(
			resolveVerticalSlots({
				line: line(0, "某句"),
				nextLine: undefined,
				currentIndex: -1,
			}),
		).toEqual({});
	});

	it("下一句是圆点行时右列留空（不把圆点当歌词）", () => {
		const { left, right } = resolveVerticalSlots({
			line: line(0, "当前句"),
			nextLine: line(6000, "···", true),
			currentIndex: 0,
		});
		expect(left?.line.lineText).toBe("当前句");
		expect(right).toBeUndefined();
	});
});
