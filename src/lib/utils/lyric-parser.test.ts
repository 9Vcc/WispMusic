import { describe, expect, it } from "vitest";
import {
	applyFallbackWordTiming,
	type ILyricLine,
	ParseLyric,
	ParseVerbatimLyric,
} from "./lyric-parser";

const spanOf = (line: ILyricLine) => {
	const words = line.words ?? [];
	const last = words[words.length - 1];
	return last ? last.startTime + last.duration - line.lineStart : 0;
};
const joined = (line: ILyricLine) =>
	(line.words ?? []).map((word) => word.char).join("");

describe("applyFallbackWordTiming", () => {
	it("有行尾标记时按标记取跨度（标记本身就是演唱结束点）", () => {
		const lines = ParseLyric(
			[
				"[00:00.00]短的",
				"[00:02.00]",
				"[00:02.00]很长的尾音一直唱到最后",
				"[00:12.00]",
				"[00:12.00]下一句",
				"[00:14.00]",
			].join("\n"),
		);
		if (!lines) throw new Error("fixture 解析失败");

		const result = applyFallbackWordTiming(lines);
		// 同曲已知跨度中位数 = 6000（→ 封顶 8400），但这一行有标记 10000，必须原样保留
		expect(result[1].lineText).toBe("很长的尾音一直唱到最后");
		expect(spanOf(result[1])).toBe(10000);
		expect(spanOf(result[0])).toBe(2000);
	});

	it("没有行尾标记时用同曲中位跨度封顶，不把伴奏算进演唱时长", () => {
		const lines = ParseLyric(
			[
				"[00:00.00]短句一",
				"[00:02.00]",
				"[00:02.00]短句二",
				"[00:04.00]",
				"[00:04.00]长过门前的行",
				"[00:20.00]下一句",
			].join("\n"),
		);
		if (!lines) throw new Error("fixture 解析失败");

		// 间隔中位数 = 2000（2000/2000/16000）→ 封顶 3000
		const result = applyFallbackWordTiming(lines);
		expect(spanOf(result[2])).toBe(3000);
		// 下一句没有行尾也没有下一行：同样被封顶，而不是顶着 3000ms 默认值
		expect(spanOf(result[3])).toBe(3000);
	});

	it("没有可用参照时保持原行为：拉到下一行 / 用兜底时长", () => {
		const lines = ParseLyric(
			["[00:00.00]第一句", "[00:02.00]第二句"].join("\n"),
		);
		if (!lines) throw new Error("fixture 解析失败");

		const result = applyFallbackWordTiming(lines);
		expect(spanOf(result[0])).toBe(2000);
		expect(spanOf(result[1])).toBe(3000);
	});

	it("已有逐字时间的行原样返回（不覆盖真逐字数据）", () => {
		const verbatim = ParseVerbatimLyric("[0,1200](0,300,0)你(300,900,0)好");
		if (!verbatim) throw new Error("fixture 解析失败");

		const result = applyFallbackWordTiming(verbatim);
		expect(result[0].words).toBe(verbatim[0].words);
	});

	it("生成的时间轴自洽：首字贴行首、单调不减、正时长、文本逐字还原", () => {
		const lines = ParseLyric(
			[
				"[00:00.00]一二三四五",
				"[00:02.00]",
				"[00:02.00]六七八 Hello world",
				"[00:08.00]",
				"[00:08.00]九",
			].join("\n"),
		);
		if (!lines) throw new Error("fixture 解析失败");

		for (const line of applyFallbackWordTiming(lines)) {
			const words = line.words ?? [];
			expect(words.length).toBeGreaterThan(0);
			expect(words[0].startTime).toBe(line.lineStart);
			expect(joined(line)).toBe(line.lineText);
			for (let i = 0; i < words.length; i++) {
				expect(words[i].duration).toBeGreaterThan(0);
				if (i > 0) {
					expect(words[i].startTime).toBeGreaterThanOrEqual(
						words[i - 1].startTime + words[i - 1].duration,
					);
				}
			}
		}
	});

	it("不修改入参（本地歌词解析结果在 store 里是共享引用）", () => {
		const lines = ParseLyric(
			["[00:00.00]一二三", "[00:02.00]四五六"].join("\n"),
		);
		if (!lines) throw new Error("fixture 解析失败");
		const before = JSON.stringify(lines);

		applyFallbackWordTiming(lines);

		expect(JSON.stringify(lines)).toBe(before);
	});
});
