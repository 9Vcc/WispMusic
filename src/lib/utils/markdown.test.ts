import { describe, expect, it } from "vitest";
import { type BlockToken, parseInline, parseMarkdown } from "./markdown";

/** 测试用类型收窄：断言块类型并返回具体 token（避免 as 滥用） */
function expectKind<K extends BlockToken["kind"]>(
	block: BlockToken,
	kind: K,
): Extract<BlockToken, { kind: K }> {
	if (block.kind !== kind) {
		throw new Error(`expected ${kind}, got ${block.kind}`);
	}
	return block as Extract<BlockToken, { kind: K }>;
}

describe("parseInline", () => {
	it("纯文本无记号时输出单个 text token", () => {
		expect(parseInline("普通更新说明")).toEqual([
			{ kind: "text", value: "普通更新说明" },
		]);
	});

	it("解析加粗 / 斜体 / 删除线 / 行内代码", () => {
		expect(parseInline("**加粗** 与 *斜体* 与 ~~删除~~ 与 `code`")).toEqual([
			{ kind: "strong", spans: [{ kind: "text", value: "加粗" }] },
			{ kind: "text", value: " 与 " },
			{ kind: "em", spans: [{ kind: "text", value: "斜体" }] },
			{ kind: "text", value: " 与 " },
			{ kind: "del", spans: [{ kind: "text", value: "删除" }] },
			{ kind: "text", value: " 与 " },
			{ kind: "code", value: "code" },
		]);
	});

	it("解析链接并保留地址与文字", () => {
		expect(
			parseInline("查看 [发布页](https://example.com/a) 了解更多"),
		).toEqual([
			{ kind: "text", value: "查看 " },
			{
				kind: "link",
				href: "https://example.com/a",
				spans: [{ kind: "text", value: "发布页" }],
			},
			{ kind: "text", value: " 了解更多" },
		]);
	});

	it("行内代码内部不解析其他记号", () => {
		expect(parseInline("`**不是加粗**`")).toEqual([
			{ kind: "code", value: "**不是加粗**" },
		]);
	});

	it("加粗内部继续解析行内记号", () => {
		expect(parseInline("**重要 `fix`**")).toEqual([
			{
				kind: "strong",
				spans: [
					{ kind: "text", value: "重要 " },
					{ kind: "code", value: "fix" },
				],
			},
		]);
	});

	it("未闭合记号按原文输出不吞字", () => {
		expect(parseInline("这里有 ** 未闭合星号")).toEqual([
			{ kind: "text", value: "这里有 ** 未闭合星号" },
		]);
		expect(parseInline("坏链接 [text](no url here) 保持原文")).toEqual([
			{ kind: "text", value: "坏链接 [text](no url here) 保持原文" },
		]);
	});

	it("同段多个链接与文本混合", () => {
		const tokens = parseInline("[A](https://a.com) 和 [B](https://b.com)");
		expect(tokens).toHaveLength(3);
		expect(tokens[0]).toMatchObject({ kind: "link", href: "https://a.com" });
		expect(tokens[1]).toEqual({ kind: "text", value: " 和 " });
		expect(tokens[2]).toMatchObject({ kind: "link", href: "https://b.com" });
	});
});

describe("parseMarkdown: 块级结构", () => {
	it("解析标题层级", () => {
		const blocks = parseMarkdown("# 大标题\n## 中标题\n### 小节");
		expect(blocks).toEqual([
			{ kind: "heading", level: 1, spans: [{ kind: "text", value: "大标题" }] },
			{ kind: "heading", level: 2, spans: [{ kind: "text", value: "中标题" }] },
			{ kind: "heading", level: 3, spans: [{ kind: "text", value: "小节" }] },
		]);
	});

	it("标题行内记号照常解析", () => {
		const blocks = parseMarkdown("## v0.0.2 **正式版**");
		expect(blocks[0]).toEqual({
			kind: "heading",
			level: 2,
			spans: [
				{ kind: "text", value: "v0.0.2 " },
				{ kind: "strong", spans: [{ kind: "text", value: "正式版" }] },
			],
		});
	});

	it("连续文本行合并为段落且逐行保留", () => {
		const blocks = parseMarkdown("第一行\n第二行\n\n新段落");
		expect(blocks).toEqual([
			{
				kind: "paragraph",
				lines: [
					[{ kind: "text", value: "第一行" }],
					[{ kind: "text", value: "第二行" }],
				],
			},
			{
				kind: "paragraph",
				lines: [[{ kind: "text", value: "新段落" }]],
			},
		]);
	});

	it("无序列表：多项与混合记号", () => {
		const blocks = parseMarkdown("- 第一条\n- 第二条\n* 星号项\n+ 加号项");
		const list = expectKind(blocks[0], "list");
		expect(list.ordered).toBe(false);
		expect(list.items.map((item) => item.spans)).toEqual([
			[{ kind: "text", value: "第一条" }],
			[{ kind: "text", value: "第二条" }],
			[{ kind: "text", value: "星号项" }],
			[{ kind: "text", value: "加号项" }],
		]);
	});

	it("有序列表：起始序号保留", () => {
		const blocks = parseMarkdown("3. 三\n4. 四");
		const list = expectKind(blocks[0], "list");
		expect(list.ordered).toBe(true);
		expect(list.start).toBe(3);
		expect(list.items).toHaveLength(2);
	});

	it("嵌套列表：缩进两空格挂到上级条目", () => {
		const blocks = parseMarkdown("- 外层\n  - 内层一\n  - 内层二\n- 外层二");
		const list = expectKind(blocks[0], "list");
		expect(list.items).toHaveLength(2);
		const first = list.items[0];
		expect(first.spans).toEqual([{ kind: "text", value: "外层" }]);
		expect(first.children).toHaveLength(1);
		const child = expectKind(first.children[0], "list");
		expect(child.ordered).toBe(false);
		expect(child.items.map((i) => i.spans)).toEqual([
			[{ kind: "text", value: "内层一" }],
			[{ kind: "text", value: "内层二" }],
		]);
	});

	it("列表与段落交替：列表被段落打断后各自成块", () => {
		const blocks = parseMarkdown("- 甲\n\n说明文字\n- 乙");
		expect(blocks).toHaveLength(3);
		expect(blocks.map((b) => b.kind)).toEqual(["list", "paragraph", "list"]);
	});

	it("围栏代码块：语言标注与内容原文保留", () => {
		const blocks = parseMarkdown('```json\n{\n  "a": 1\n}\n```');
		expect(blocks).toEqual([
			{ kind: "code", lang: "json", value: '{\n  "a": 1\n}' },
		]);
	});

	it("代码块内部不解析块级/行内记号，未闭合时吃到末尾", () => {
		const blocks = parseMarkdown("```\n- 不是列表\n**不是加粗**");
		expect(blocks).toEqual([
			{ kind: "code", lang: "", value: "- 不是列表\n**不是加粗**" },
		]);
	});

	it("分隔线与列表项区分", () => {
		const blocks = parseMarkdown("---\n- 列表项");
		expect(blocks.map((b) => b.kind)).toEqual(["hr", "list"]);
	});

	it("引用块：连续 > 行合并且内部递归解析", () => {
		const blocks = parseMarkdown("> 第一行\n> **加粗行**");
		const quote = expectKind(blocks[0], "quote");
		expect(quote.blocks).toHaveLength(1);
		expect(quote.blocks[0].kind).toBe("paragraph");
	});

	it("空输入与空白输入返回空数组", () => {
		expect(parseMarkdown("")).toEqual([]);
		expect(parseMarkdown("\n  \n\r\n")).toEqual([]);
	});

	it("CHANGELOG 版本段落形态端到端", () => {
		const source = [
			"### 新增功能",
			"",
			"- 桌面歌词悬浮控件：支持 **横向 / 竖向** 切换",
			"- 修复 `check()` 报错，详见 [发布页](https://example.com)",
			"",
			"### 修复问题",
			"",
			"- 修复播放条歌名闪动",
		].join("\n");
		const blocks = parseMarkdown(source);
		expect(blocks.map((b) => b.kind)).toEqual([
			"heading",
			"list",
			"heading",
			"list",
		]);
	});
});
