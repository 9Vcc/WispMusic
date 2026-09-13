import { describe, expect, it } from "vitest";
import {
	clampLyricOffset,
	DEFAULT_LYRIC_OFFSET_MS,
	formatLyricOffset,
	LYRIC_OFFSET_MAX_MS,
	LYRIC_OFFSET_NUDGE_MS,
} from "./lyric";

describe("clampLyricOffset", () => {
	it("非法输入落回 0", () => {
		expect(clampLyricOffset(Number.NaN)).toBe(DEFAULT_LYRIC_OFFSET_MS);
		expect(clampLyricOffset(Number.POSITIVE_INFINITY)).toBe(
			DEFAULT_LYRIC_OFFSET_MS,
		);
	});

	it("夹在 ±上限内（EchoMusic 同款 ±10s）", () => {
		expect(LYRIC_OFFSET_MAX_MS).toBe(10000);
		expect(clampLyricOffset(99999)).toBe(LYRIC_OFFSET_MAX_MS);
		expect(clampLyricOffset(-99999)).toBe(-LYRIC_OFFSET_MAX_MS);
	});

	it("区间内四舍五入到整数毫秒", () => {
		expect(clampLyricOffset(120.4)).toBe(120);
		expect(clampLyricOffset(-120.6)).toBe(-121);
	});
});

describe("formatLyricOffset", () => {
	it("EchoMusic 同款读数：带符号的秒", () => {
		expect(formatLyricOffset(0)).toBe("+0.0s");
		expect(formatLyricOffset(1500)).toBe("+1.5s");
		expect(formatLyricOffset(-500)).toBe("-0.5s");
		expect(formatLyricOffset(10000)).toBe("+10.0s");
	});

	it("步进按钮的步长为 0.5s（EchoMusic 默认）", () => {
		expect(LYRIC_OFFSET_NUDGE_MS).toBe(500);
	});
});
