import { describe, expect, it } from "vitest";
import {
	buildDesktopLyricColorVars,
	buildStrokeRowColorVars,
	clampBackgroundOpacity,
	DESKTOP_LYRIC_ALPHA,
	DESKTOP_LYRIC_COLOR_SCHEMES,
	DESKTOP_LYRIC_DEFAULT_COLORS,
	isDefaultDesktopLyricColors,
	normalizeDesktopLyricColors,
	normalizeLyricColor,
	tryNormalizeLyricColor,
	withAlpha,
} from "./desktop-lyric-colors";

describe("normalizeLyricColor", () => {
	it("接受 #rrggbb / #rgb，可省略 # 且大小写不限", () => {
		expect(normalizeLyricColor("#FFB733", "#000000")).toBe("#ffb733");
		expect(normalizeLyricColor("ffb733", "#000000")).toBe("#ffb733");
		expect(normalizeLyricColor("#f0a", "#000000")).toBe("#ff00aa");
		expect(normalizeLyricColor("  #AbC  ", "#000000")).toBe("#aabbcc");
	});

	it("非法输入回落 fallback（不抛错、不返回空串）", () => {
		for (const bad of [
			"",
			"   ",
			"red",
			"#12345",
			"#gggggg",
			"rgb(1,2,3)",
			null,
			undefined,
			42,
			{},
		]) {
			expect(normalizeLyricColor(bad, "#ffffff")).toBe("#ffffff");
		}
	});
});

describe("tryNormalizeLyricColor", () => {
	it("合法输入返回规范化结果，非法返回 null（输入框校验用）", () => {
		expect(tryNormalizeLyricColor("#FFB733")).toBe("#ffb733");
		expect(tryNormalizeLyricColor("f0a")).toBe("#ff00aa");
		expect(tryNormalizeLyricColor("bogus")).toBeNull();
		expect(tryNormalizeLyricColor("")).toBeNull();
		expect(tryNormalizeLyricColor(null)).toBeNull();
	});
});

describe("clampBackgroundOpacity", () => {
	it("取整并钳在 0~100", () => {
		expect(clampBackgroundOpacity(42.6, 0)).toBe(43);
		expect(clampBackgroundOpacity(-10, 0)).toBe(0);
		expect(clampBackgroundOpacity(180, 0)).toBe(100);
	});

	it("非数值回落 fallback", () => {
		expect(clampBackgroundOpacity("50", 0)).toBe(0);
		expect(clampBackgroundOpacity(Number.NaN, 7)).toBe(7);
		expect(clampBackgroundOpacity(undefined, 7)).toBe(7);
	});
});

describe("normalizeDesktopLyricColors", () => {
	it("空配置即出厂配色", () => {
		expect(normalizeDesktopLyricColors()).toEqual(DESKTOP_LYRIC_DEFAULT_COLORS);
		expect(normalizeDesktopLyricColors(null)).toEqual(
			DESKTOP_LYRIC_DEFAULT_COLORS,
		);
	});

	it("脏字段逐项回落，其余字段仍生效", () => {
		const colors = normalizeDesktopLyricColors({
			textColor: "#f0a",
			highlightColor: "bogus",
			strokeColor: undefined,
			backgroundOpacity: 999,
		});
		expect(colors).toEqual({
			textColor: "#ff00aa",
			highlightColor: DESKTOP_LYRIC_DEFAULT_COLORS.highlightColor,
			strokeColor: DESKTOP_LYRIC_DEFAULT_COLORS.strokeColor,
			backgroundOpacity: 100,
		});
	});
});

describe("withAlpha", () => {
	it("拼出 rgb(r g b / a) 且钳制 alpha", () => {
		expect(withAlpha("#ffb733", 0.52)).toBe("rgb(255 183 51 / 0.52)");
		expect(withAlpha("#ffffff", 1)).toBe("rgb(255 255 255 / 1)");
		expect(withAlpha("#000000", -3)).toBe("rgb(0 0 0 / 0)");
		expect(withAlpha("#000000", 9)).toBe("rgb(0 0 0 / 1)");
	});

	it("非法颜色回落默认高亮色而不是变成透明", () => {
		expect(withAlpha("nope", 1)).toBe("rgb(255 255 255 / 1)");
	});
});

describe("buildDesktopLyricColorVars", () => {
	it("默认配色复现改造前的硬编码取值（外观零变化）", () => {
		const vars = buildDesktopLyricColorVars(DESKTOP_LYRIC_DEFAULT_COLORS);
		expect(vars["--dl-current-color"]).toBe(
			`rgb(255 255 255 / ${DESKTOP_LYRIC_ALPHA.current})`,
		);
		expect(vars["--dl-text-color"]).toBe(
			`rgb(255 255 255 / ${DESKTOP_LYRIC_ALPHA.preview})`,
		);
		expect(vars["--dl-stroke-color"]).toBe("rgb(8 10 14 / 0.52)");
		expect(vars["--dl-shadow-soft"]).toBe("rgb(8 10 14 / 0.45)");
		expect(vars["--dl-shadow-strong"]).toBe("rgb(8 10 14 / 0.55)");
		expect(vars["--dl-halo-shadow"]).toBe("rgb(8 10 14 / 0.32)");
		expect(vars["--dl-stroke-drop-shadow"]).toBe("rgb(8 10 14 / 0.5)");
		expect(vars["--dl-word-fill-color"]).toBe("rgb(255 255 255 / 1)");
		expect(vars["--dl-glyph-fill-color"]).toBe("rgb(255 255 255 / 1)");
		expect(vars["--dl-glyph-fill-soft"]).toBe("rgb(255 255 255 / 1)");
		expect(vars["--dl-bg-alpha"]).toBe("0");
		expect(vars["--dl-bg-alpha-hover"]).toBe("0.6");
	});

	it("自定义配色分别驱动文字 / 高亮 / 描边 / 背景", () => {
		const vars = buildDesktopLyricColorVars({
			textColor: "#cfe6ff",
			highlightColor: "#54c8ff",
			strokeColor: "#04182b",
			backgroundOpacity: 42,
		});
		expect(vars["--dl-text-color"]).toBe("rgb(207 230 255 / 0.45)");
		expect(vars["--dl-current-color"]).toBe("rgb(84 200 255 / 0.88)");
		expect(vars["--dl-word-fill-color"]).toBe("rgb(84 200 255 / 1)");
		expect(vars["--dl-stroke-color"]).toBe("rgb(4 24 43 / 0.52)");
		expect(vars["--dl-glyph-fill-color"]).toBe("rgb(84 200 255 / 1)");
		expect(vars["--dl-bg-alpha"]).toBe("0.42");
	});

	it("背景不透明度高于悬浮下限时 hover 不再压低背景", () => {
		const vars = buildDesktopLyricColorVars({
			...DESKTOP_LYRIC_DEFAULT_COLORS,
			backgroundOpacity: 80,
		});
		expect(vars["--dl-bg-alpha"]).toBe("0.8");
		expect(vars["--dl-bg-alpha-hover"]).toBe("0.8");
	});
});

describe("buildStrokeRowColorVars", () => {
	it("当前句用高亮色，其余行用普通文字色（笔画字格均为纯色不透明）", () => {
		const colors = {
			textColor: "#cfe6ff",
			highlightColor: "#54c8ff",
			strokeColor: "#04182b",
			backgroundOpacity: 0,
		};
		expect(buildStrokeRowColorVars(colors, true)["--dl-glyph-fill-color"]).toBe(
			"rgb(84 200 255 / 1)",
		);
		expect(
			buildStrokeRowColorVars(colors, false)["--dl-glyph-fill-color"],
		).toBe("rgb(207 230 255 / 1)");
	});
});

describe("内置配色方案", () => {
	it("每套方案都是合法配色且含「经典白」默认项", () => {
		for (const scheme of DESKTOP_LYRIC_COLOR_SCHEMES) {
			expect(scheme.name.length).toBeGreaterThan(0);
			expect(normalizeDesktopLyricColors(scheme.colors)).toEqual(scheme.colors);
		}
		const classic = DESKTOP_LYRIC_COLOR_SCHEMES.find(
			(s) => s.name === "经典白",
		);
		if (!classic) throw new Error("缺少「经典白」方案");
		expect(classic.colors).toEqual(DESKTOP_LYRIC_DEFAULT_COLORS);
		expect(isDefaultDesktopLyricColors(classic.colors)).toBe(true);
	});

	it("非默认配色不会被误判为默认", () => {
		expect(
			isDefaultDesktopLyricColors({
				...DESKTOP_LYRIC_DEFAULT_COLORS,
				backgroundOpacity: 1,
			}),
		).toBe(false);
		expect(
			isDefaultDesktopLyricColors({
				...DESKTOP_LYRIC_DEFAULT_COLORS,
				highlightColor: "#ffb733",
			}),
		).toBe(false);
	});
});
