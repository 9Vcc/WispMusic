import { describe, expect, it } from "vitest";
import { isIndexInPair, resolvePairSlots } from "./desktop-lyric-pair";

describe("isIndexInPair", () => {
	it("配对覆盖首句与次句两句，第三句即出对", () => {
		expect(isIndexInPair(3, 3)).toBe(true);
		expect(isIndexInPair(4, 3)).toBe(true); // 次句演唱中仍留在本对
		expect(isIndexInPair(5, 3)).toBe(false); // 两句播完 → 该换对
		expect(isIndexInPair(2, 3)).toBe(false); // seek 回到配对之前
	});

	it("未初始化（pairFirst < 0）时一律不在配对内", () => {
		expect(isIndexInPair(0, -1)).toBe(false);
		expect(isIndexInPair(-1, -1)).toBe(false);
	});

	it("与方向无关：横向双行与竖排双列共用同一判定", () => {
		// 旧实现里竖排用「首行即当前句」的窄口径（当前 = first 才算在配对内），
		// 导致次句演唱时整对就开始轮换；现在两方向都是两句在配对内
		for (const first of [0, 1, 2, 7]) {
			expect(isIndexInPair(first, first)).toBe(true);
			expect(isIndexInPair(first + 1, first)).toBe(true);
			expect(isIndexInPair(first + 2, first)).toBe(false);
		}
	});
});

describe("resolvePairSlots", () => {
	it("正常配对给出首句与次句", () => {
		expect(resolvePairSlots({ first: 2, lineCount: 6 })).toEqual({
			first: 2,
			second: 3,
		});
	});

	it("末句无次句时只给首句（次格留空占位）", () => {
		expect(resolvePairSlots({ first: 5, lineCount: 6 })).toEqual({
			first: 5,
			second: undefined,
		});
	});

	it("未初始化或越界时两格都空", () => {
		expect(resolvePairSlots({ first: -1, lineCount: 6 })).toEqual({});
		expect(resolvePairSlots({ first: 9, lineCount: 6 })).toEqual({});
	});
});
