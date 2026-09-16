/**
 * 零依赖 Markdown 解析器（纯函数、无 DOM，可被 node 环境单测）。
 *
 * 用途：应用内更新弹窗 / 更新日志弹窗渲染 CHANGELOG 片段（update.json 的
 * notes 由发布脚本从 .github/workflows/CHANGELOG.md 按 `## vX.Y.Z` 段落提取）。
 * 项目没有 markdown 依赖、内容源可信（自家维护的 CHANGELOG），故实现一个
 * 覆盖常见语法的小型解析器而非引入 react-markdown：
 *
 * - 块级：ATX 标题（#~######）、段落（连续行）、无序/有序列表（支持缩进
 *   嵌套）、围栏代码块（```lang）、引用（>）、分隔线（---）；
 * - 行内：`行内代码`、**加粗**、*斜体*、~~删除线~~、[文本](链接)；
 * - 明确不解析：表格、图片、HTML 标签、脚注（CHANGELOG 用不到，遇之按
 *   普通文本展示，不会丢内容）。
 *
 * 安全边界：解析产物是结构化 token 而非 HTML 字符串，由 React 组件渲染，
 * 文本节点自动转义；链接协议白名单在渲染层校验（见 markdown-view.tsx）。
 */

/** 行内 token：text/code 为纯文本，其余含可继续递归的行内 spans */
export type InlineToken =
	| { kind: "text"; value: string }
	| { kind: "code"; value: string }
	| { kind: "strong"; spans: InlineToken[] }
	| { kind: "em"; spans: InlineToken[] }
	| { kind: "del"; spans: InlineToken[] }
	| { kind: "link"; href: string; spans: InlineToken[] };

/** 列表项：spans 为项内文字，children 为缩进嵌套出的子块（子列表等） */
export interface MarkdownListItem {
	spans: InlineToken[];
	children: BlockToken[];
}

/** 块级 token */
export type BlockToken =
	| { kind: "heading"; level: number; spans: InlineToken[] }
	| { kind: "paragraph"; lines: InlineToken[][] }
	| { kind: "list"; ordered: boolean; start: number; items: MarkdownListItem[] }
	| { kind: "code"; lang: string; value: string }
	| { kind: "quote"; blocks: BlockToken[] }
	| { kind: "hr" };

/**
 * 行内扫描：顺序即优先级——行内代码最先（内部一切按原文），其后加粗、
 * 删除线、斜体（`*` 单星必须在 `**` 之后尝试）、链接。未闭合的记号不匹配，
 * 原样按普通文本输出，不会吞字。
 *
 * 注意：parseInline 会递归（加粗/链接内部继续解析），因此这里只存
 * pattern 源串、每次调用 new 独立实例——共享 /g 正则的 lastIndex 会被
 * 递归调用重置归零，外层循环从同位置反复匹配直至内存耗尽。
 */
const INLINE_PATTERN_SOURCE =
	"(`[^`\\n]+`)|(\\*\\*[^*\\n]+\\*\\*)|(~~[^~\\n]+~~)|(\\*\\S(?:[^*\\n]*\\S)?\\*)|(\\[[^\\]\\n]+\\]\\(\\s*[^()\\s]+\\s*\\))";

/** 解析一行内的行内格式（不含块级结构） */
export function parseInline(text: string): InlineToken[] {
	const tokens: InlineToken[] = [];
	let cursor = 0;
	const pattern = new RegExp(INLINE_PATTERN_SOURCE, "g");
	for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
		if (match.index > cursor) {
			tokens.push({ kind: "text", value: text.slice(cursor, match.index) });
		}
		const [raw, code, strong, del, em, link] = match;
		if (code !== undefined) {
			tokens.push({ kind: "code", value: code.slice(1, -1) });
		} else if (strong !== undefined) {
			tokens.push({ kind: "strong", spans: parseInline(strong.slice(2, -2)) });
		} else if (del !== undefined) {
			tokens.push({ kind: "del", spans: parseInline(del.slice(2, -2)) });
		} else if (em !== undefined) {
			tokens.push({ kind: "em", spans: parseInline(em.slice(1, -1)) });
		} else if (link !== undefined) {
			const parsed = /^\[([^\]\n]+)\]\(\s*([^()\s]+)\s*\)$/.exec(link);
			if (parsed) {
				tokens.push({
					kind: "link",
					href: parsed[2],
					spans: parseInline(parsed[1]),
				});
			} else {
				tokens.push({ kind: "text", value: link });
			}
		}
		cursor = match.index + raw.length;
	}
	if (cursor < text.length) {
		tokens.push({ kind: "text", value: text.slice(cursor) });
	}
	return tokens;
}

/** 原始列表行（缩进/类型/序号/内容已在收集阶段提取） */
interface RawListItem {
	indent: number;
	ordered: boolean;
	marker: string;
	text: string;
}

/** 用栈把同一段连续列表行按缩进装成嵌套列表（首行缩进为基准层级） */
function buildListBlock(raws: RawListItem[]): BlockToken {
	const baseIndent = raws[0].indent;
	interface Frame {
		indent: number;
		items: MarkdownListItem[];
		block: Extract<BlockToken, { kind: "list" }>;
	}
	const stack: Frame[] = [];
	for (const raw of raws) {
		const indent = raw.indent - baseIndent;
		// 退栈：比当前行更深的层级全部弹出
		while (stack.length > 0 && indent < stack[stack.length - 1].indent) {
			stack.pop();
		}
		const top = stack[stack.length - 1];
		const item: MarkdownListItem = {
			spans: parseInline(raw.text),
			children: [],
		};
		if (top && indent === top.indent) {
			// 同级条目：直接追加（同一缩进混排有序/无序时并入栈顶，列表类型取首行，保证不丢内容）
			top.items.push(item);
			continue;
		}
		const start = raw.ordered ? Number.parseInt(raw.marker, 10) || 1 : 1;
		const block: Extract<BlockToken, { kind: "list" }> = {
			kind: "list",
			ordered: raw.ordered,
			start,
			items: [item],
		};
		if (!top) {
			// 顶层列表（首行缩进即基准，整个 raws 只会有一个顶层列表）
			stack.push({ indent, items: block.items, block });
			continue;
		}
		// 更深层级：挂到栈顶最后一个条目的 children（数组引用共享，后续同级条目继续追加）
		top.items[top.items.length - 1].children.push(block);
		stack.push({ indent, items: block.items, block });
	}
	return stack[0].block;
}

/**
 * 解析整篇 Markdown 为块级 token 数组。
 * CRLF 归一为 LF；围栏代码块内部内容不做任何行内解析、按原文保留。
 */
export function parseMarkdown(source: string): BlockToken[] {
	const lines = String(source ?? "")
		.replace(/\r\n?/g, "\n")
		.split("\n");
	const blocks: BlockToken[] = [];
	let paragraphLines: string[] = [];

	const flushParagraph = () => {
		if (paragraphLines.length > 0) {
			blocks.push({
				kind: "paragraph",
				lines: paragraphLines.map((line) => parseInline(line)),
			});
			paragraphLines = [];
		}
	};

	let i = 0;
	while (i < lines.length) {
		const line = lines[i];

		// 围栏代码块：``` 或 ~~~ 开启（首个字符决定闭栏记号），未闭合时吃到末尾
		const fenceOpen = /^\s*(`{3,}|~{3,})\s*(\S*)\s*$/.exec(line);
		if (fenceOpen) {
			flushParagraph();
			const fenceChar = fenceOpen[1][0];
			const lang = fenceOpen[2] ?? "";
			const buffer: string[] = [];
			i += 1;
			while (
				i < lines.length &&
				!new RegExp(`^\\s*\\${fenceChar}{3,}\\s*$`).test(lines[i])
			) {
				buffer.push(lines[i]);
				i += 1;
			}
			i += 1; // 跳过闭栏行（缺失时越过末尾）
			blocks.push({ kind: "code", lang, value: buffer.join("\n") });
			continue;
		}

		// ATX 标题（行尾的 # 闭合序列剥掉）
		const heading = /^(#{1,6})\s+(.+?)\s*#*\s*$/.exec(line);
		if (heading) {
			flushParagraph();
			blocks.push({
				kind: "heading",
				level: heading[1].length,
				spans: parseInline(heading[2]),
			});
			i += 1;
			continue;
		}

		// 分隔线：--- / *** / ___ 且整行只有该记号（与 `- ` 列表天然区分）
		if (/^\s*(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
			flushParagraph();
			blocks.push({ kind: "hr" });
			i += 1;
			continue;
		}

		// 引用：连续的 > 行合并为一个 quote 块，内部递归按普通块解析
		if (/^\s*>/.test(line)) {
			flushParagraph();
			const buffer: string[] = [];
			while (i < lines.length) {
				const quoted = /^\s*>\s?(.*)$/.exec(lines[i]);
				if (!quoted) break;
				buffer.push(quoted[1]);
				i += 1;
			}
			blocks.push({ kind: "quote", blocks: parseMarkdown(buffer.join("\n")) });
			continue;
		}

		// 列表：收集连续列表行（缩进嵌套；空行即结束，CHANGELOG 不用松散列表）
		const listLine = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/.exec(line);
		if (listLine) {
			flushParagraph();
			const raws: RawListItem[] = [];
			while (i < lines.length) {
				const m = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/.exec(lines[i]);
				if (!m) break;
				raws.push({
					indent: m[1].length,
					ordered: /\d/.test(m[2]),
					marker: m[2],
					text: m[3].trim(),
				});
				i += 1;
			}
			blocks.push(buildListBlock(raws));
			continue;
		}

		if (line.trim() === "") {
			flushParagraph();
			i += 1;
			continue;
		}

		// 其余：段落连续行（逐行保留，渲染层以换行呈现）
		paragraphLines.push(line.trim());
		i += 1;
	}
	flushParagraph();
	return blocks;
}
