import { describe, expect, it } from "vitest";
import { ParseLyric, ParseVerbatimLyric } from "./lyric-parser";
import { resolveLyricLines } from "./lyric-source";

/** 网易云 yrc：JSON 版权头 + 逐字行（真实响应形状） */
const YRC = [
	'{"t":0,"c":[{"tx":"作词: "},{"tx":"测试作者"}]}',
	"[0,1200](0,300,0)你(300,300,0)好(600,600,0)呀",
	"[1200,2600](1200,600,0)世(1800,800,0)界(2600,1200,0)啊",
].join("\n");

/** 同一首歌的逐行 lrc：行首比 yrc 早约 200ms（真实响应的常态） */
const LRC = [
	'{"t":0,"c":[{"tx":"作词: "},{"tx":"测试作者"}]}',
	"[00:00.000]你好呀",
	"[00:01.000]世界啊",
	"[00:03.800]",
].join("\n");

/** 本地歌词文件常见的逐行 .lrc（无 yrc 对应歌曲） */
const LOCAL_PLAIN_LRC = ["[00:00.00]一二三四五", "[00:02.00]六七八九十"].join(
	"\n",
);

function tokensOf(words: readonly { char: string }[] | undefined) {
	return (words ?? []).map((word) => word.char).join("");
}

describe("resolveLyricLines 来源解析", () => {
	it("在线逐字歌词优先于逐行歌词，并保留逐字时间轴", () => {
		const result = resolveLyricLines({
			verbatimSource: YRC,
			plainSource: LRC,
		});

		expect(result.origin).toBe("verbatim");
		expect(result.hasWordTiming).toBe(true);
		expect(result.lines).toHaveLength(2);
		expect(result.lines[0].words?.map((word) => word.char)).toEqual([
			"你",
			"好",
			"呀",
		]);
	});

	it("yrc 解析不出任何行时回落到 lrc（空数组不再挡掉逐行歌词）", () => {
		// 只有 JSON 版权头、没有任何 [start,dur] 行的 yrc：旧逻辑会返回空数组，
		// 而空数组是真值，`ParseVerbatimLyric(yrc) || ParseLyric(lrc)` 于是整首空白
		const emptyVerbatim = '{"t":0,"c":[{"tx":"作词: "},{"tx":"测试作者"}]}';
		expect(ParseVerbatimLyric(emptyVerbatim)).toEqual([]);

		const result = resolveLyricLines({
			verbatimSource: emptyVerbatim,
			plainSource: LRC,
		});

		expect(result.origin).toBe("plain");
		expect(result.lines.map((line) => line.lineText)).toEqual([
			"你好呀",
			"世界啊",
		]);
	});

	it("本地逐行歌词 + 在线逐字歌词：逐字时间轴按最近行贴回，动效一致", () => {
		const local = ParseLyric(LOCAL_PLAIN_LRC);
		if (!local) throw new Error("fixture 解析失败");

		const result = resolveLyricLines({
			localLines: local,
			verbatimSource: YRC,
		});

		expect(result.origin).toBe("local");
		expect(result.hasWordTiming).toBe(true);
		// 文本仍是本地文件的内容，动效时间轴来自在线 yrc
		expect(result.lines[0].lineText).toBe("一二三四五");
		expect(tokensOf(result.lines[0].words)).toBe("一二三四五");
		expect(result.lines[0].words?.[0].duration).toBeGreaterThan(0);
	});

	it("本地逐字歌词不会被在线歌词覆盖", () => {
		const localVerbatim = ParseVerbatimLyric(
			"[0,900](0,300,0)本(300,300,0)地(600,300,0)字",
		);
		if (!localVerbatim) throw new Error("fixture 解析失败");

		const result = resolveLyricLines({
			localLines: localVerbatim,
			verbatimSource: YRC,
		});

		expect(result.origin).toBe("local");
		expect(result.lines[0].words?.map((word) => word.char)).toEqual([
			"本",
			"地",
			"字",
		]);
	});

	it("两份歌词文本不一致时不套用另一份的逐字时间轴", () => {
		const local = ParseLyric("[00:00.00]完全不同的歌词内容");
		if (!local) throw new Error("fixture 解析失败");

		const result = resolveLyricLines({
			localLines: local,
			verbatimSource: YRC,
		});

		expect(result.lines[0].lineText).toBe("完全不同的歌词内容");
		// 已改为按行时长生成的兜底时间轴，词元拼出来必须还是原文
		expect(tokensOf(result.lines[0].words)).toBe("完全不同的歌词内容");
	});

	it("本地歌词为 .lrc 时按行时长生成兜底时间轴（不再只有整行淡入）", () => {
		const local = ParseLyric(LOCAL_PLAIN_LRC);
		if (!local) throw new Error("fixture 解析失败");

		const result = resolveLyricLines({ localLines: local });

		expect(result.hasWordTiming).toBe(true);
		for (const line of result.lines) {
			const words = line.words ?? [];
			expect(words.length).toBeGreaterThan(1);
			expect(words.map((word) => word.char).join("")).toBe(line.lineText);
			// 时间单调不减，且每个词元都有正时长（useTransform 的输入区间不能退化）
			for (let i = 0; i < words.length; i++) {
				expect(words[i].duration).toBeGreaterThan(0);
				if (i > 0) {
					expect(words[i].startTime).toBeGreaterThanOrEqual(
						words[i - 1].startTime + words[i - 1].duration,
					);
				}
			}
			// 首字落在行首
			expect(words[0].startTime).toBe(line.lineStart);
		}
	});

	it("中英混排的行也能原样还原文本", () => {
		const local = ParseLyric("[00:00.00]Hello world 你好");
		if (!local) throw new Error("fixture 解析失败");

		const result = resolveLyricLines({ localLines: local });
		const words = result.lines[0].words ?? [];

		expect(words.map((word) => word.char).join("")).toBe("Hello world 你好");
	});

	it("没有任何歌词时返回空结果", () => {
		const result = resolveLyricLines({});
		expect(result.origin).toBe("none");
		expect(result.lines).toEqual([]);
		expect(result.hasWordTiming).toBe(false);
	});
	it("手动选择的歌词版本优先于本地歌词与在线歌词", () => {
		const manual = ParseLyric("[00:00.50]手动版本第一句");
		if (!manual) throw new Error("fixture 解析失败");

		const local = ParseLyric("[00:00.00]本地歌词第一句");
		if (!local) throw new Error("fixture 解析失败");

		const result = resolveLyricLines({
			manualLines: manual,
			localLines: local,
			verbatimSource: "[0,1200](0,600,0)在(600,600,0)线",
			plainSource: "[00:00.000]在线歌词第一句",
		});

		expect(result.origin).toBe("manual");
		expect(result.lines[0].lineText).toBe("手动版本第一句");
	});

	it("不修改 store 里的本地歌词对象（解析结果只读共享）", () => {
		const local = ParseLyric(LOCAL_PLAIN_LRC);
		if (!local) throw new Error("fixture 解析失败");
		const before = JSON.stringify(local);

		resolveLyricLines({ localLines: local });

		expect(JSON.stringify(local)).toBe(before);
	});

	it("逐字歌词没覆盖到的行（如制作人员名单）用兜底时间轴补齐", () => {
		const local = ParseLyric(
			[
				"[00:00.150]一二三",
				"[00:02.120]四五六",
				"[00:03.900]制作人 : 测试",
				"[00:10.000]",
			].join("\n"),
		);
		if (!local) throw new Error("fixture 解析失败");

		const result = resolveLyricLines({
			localLines: local,
			verbatimSource: YRC,
		});

		expect(result.lines).toHaveLength(3);
		// 前两行按最近行贴到在线逐字时间轴（行首沿用本地文件的时间）
		expect(result.lines[0].lineStart).toBe(150);
		expect(result.lines[0].words?.map((word) => word.char)).toEqual([
			"一",
			"二",
			"三",
		]);
		expect(result.lines[1].words?.map((word) => word.char)).toEqual([
			"四",
			"五",
			"六",
		]);
		// 逐字歌词里没有的一行：文本必须原样保留，动效走兜底时间轴
		const creditLine = result.lines[2];
		expect(creditLine.lineText).toBe("制作人 : 测试");
		expect(creditLine.words?.length).toBeGreaterThan(1);
		expect(tokensOf(creditLine.words)).toBe("制作人 : 测试");
		expect(result.lines.every((line) => (line.words?.length ?? 0) > 0)).toBe(
			true,
		);
	});
});
