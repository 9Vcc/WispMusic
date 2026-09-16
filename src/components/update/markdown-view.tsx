import { openUrl } from "@tauri-apps/plugin-opener";
import { useMemo } from "react";
import { cn } from "@/lib/utils";
import {
	type BlockToken,
	type InlineToken,
	parseMarkdown,
} from "@/lib/utils/markdown";

/**
 * Markdown 渲染（更新弹窗 / 更新日志弹窗共用）。
 *
 * 解析走 utils/markdown 的结构化 token，由 React 元素渲染——文本节点自动
 * 转义，不存在注入 HTML 的面。视觉基线与弹窗原有排版一致：容器继承调用方
 * 的文字颜色字号（text-muted-foreground text-sm），正文块不再各自设色；
 * 加粗/标题提升到 foreground 制造层级。
 *
 * 链接行为：仅放行 http/https/mailto（其余协议按纯文本展示），Tauri 环境
 * 用 plugin-opener 交给系统浏览器打开，避免 WebView 内导航毁掉应用；非
 * Tauri 环境（纯浏览器调试）回退 window.open。
 */

/** 链接协议白名单：更新说明里的外链只可能是网页与邮件 */
function isSafeLinkHref(href: string): boolean {
	return /^(https?:\/\/|mailto:)/i.test(href);
}

async function openExternal(href: string): Promise<void> {
	if (!isSafeLinkHref(href)) return;
	if (href.startsWith("mailto:")) {
		window.location.href = href;
		return;
	}
	const isTauri = "__TAURI_INTERNALS__" in window;
	if (!isTauri) {
		window.open(href, "_blank", "noopener,noreferrer");
		return;
	}
	try {
		await openUrl(href);
	} catch (error) {
		console.warn("[markdown-view] 打开外链失败，回退 window.open:", error);
		window.open(href, "_blank", "noopener,noreferrer");
	}
}

/**
 * 行内 token → React 节点（链接点击拦截后交给系统浏览器）。
 * 用经典 for 循环而非 map：token 列表由解析器生成、永不重排，计数器作 key
 * 即稳定标识（map 回调的 index 会被 lint 判定为不安全 key）。
 */
function renderInlineSpans(spans: InlineToken[]): React.ReactNode[] {
	const nodes: React.ReactNode[] = [];
	for (let index = 0; index < spans.length; index++) {
		const span = spans[index];
		switch (span.kind) {
			case "text":
				nodes.push(<span key={index}>{span.value}</span>);
				break;
			case "code":
				nodes.push(
					<code
						key={index}
						className="rounded bg-muted px-1 py-0.5 font-mono text-[0.85em] text-foreground"
					>
						{span.value}
					</code>,
				);
				break;
			case "strong":
				nodes.push(
					<strong key={index} className="font-semibold text-foreground">
						{renderInlineSpans(span.spans)}
					</strong>,
				);
				break;
			case "em":
				nodes.push(<em key={index}>{renderInlineSpans(span.spans)}</em>);
				break;
			case "del":
				nodes.push(<del key={index}>{renderInlineSpans(span.spans)}</del>);
				break;
			case "link":
				if (isSafeLinkHref(span.href)) {
					nodes.push(
						<a
							key={index}
							href={span.href}
							className="text-primary underline decoration-primary/50 underline-offset-2 transition-colors hover:decoration-primary"
							onClick={(event) => {
								event.preventDefault();
								void openExternal(span.href);
							}}
						>
							{renderInlineSpans(span.spans)}
						</a>,
					);
				} else {
					// 非白名单协议（如 javascript:）：降级为纯文本，只展示不跳转
					nodes.push(<span key={index}>{renderInlineSpans(span.spans)}</span>);
				}
				break;
		}
	}
	return nodes;
}

/** 渲染一行内已解析好的行内内容（供结构化弹窗按条目复用） */
export function InlineSpans({ spans }: { spans: InlineToken[] }) {
	return <>{renderInlineSpans(spans)}</>;
}

/** 列表项：圆点/序号 + 文字 + 缩进的子列表 */
function renderListItem(
	item: { spans: InlineToken[]; children: BlockToken[] },
	ordered: boolean,
	marker: number,
	keyPrefix: string,
): React.ReactNode {
	return (
		<div key={keyPrefix} className="flex items-start gap-2 leading-relaxed">
			{ordered ? (
				<span className="shrink-0 tabular-nums">{marker}.</span>
			) : (
				<span className="mt-[0.55em] h-1.5 w-1.5 shrink-0 rounded-full bg-primary/60" />
			)}
			<div className="min-w-0 flex-1">
				<div>{renderInlineSpans(item.spans)}</div>
				{item.children.length > 0 && (
					<div className="mt-1.5 flex flex-col gap-1.5 pl-2">
						{item.children.map((child, ci) =>
							renderBlock(child, `${keyPrefix}-c${ci}`),
						)}
					</div>
				)}
			</div>
		</div>
	);
}

/** 标题字号阶梯：与弹窗信息区层级匹配（h1 > h2 > h3 > 其余同 h4） */
const HEADING_CLASS: Record<number, string> = {
	1: "text-foreground font-semibold text-base",
	2: "text-foreground font-semibold text-sm",
	3: "text-foreground font-semibold text-xs",
};

function renderBlock(block: BlockToken, key: string): React.ReactNode {
	switch (block.kind) {
		case "heading":
			return (
				<div
					key={key}
					className={cn(
						"mt-1.5 first:mt-0",
						HEADING_CLASS[block.level] ?? HEADING_CLASS[3],
					)}
				>
					{renderInlineSpans(block.spans)}
				</div>
			);
		case "paragraph":
			return (
				<p key={key} className="leading-relaxed">
					{block.lines.map((line, li) => (
						// biome-ignore lint/suspicious/noArrayIndexKey: 段落行静态不重排，索引即稳定标识
						<span key={li} className="block">
							{renderInlineSpans(line)}
						</span>
					))}
				</p>
			);
		case "list":
			return (
				<div key={key} className="flex flex-col gap-1.5">
					{block.items.map((item, ii) =>
						renderListItem(
							item,
							block.ordered,
							block.start + ii,
							`${key}-${ii}`,
						),
					)}
				</div>
			);
		case "code":
			return (
				<pre
					key={key}
					className="overflow-x-auto rounded-md bg-muted/60 p-2.5 font-mono text-xs leading-relaxed text-foreground/90"
				>
					<code>{block.value}</code>
				</pre>
			);
		case "quote":
			return (
				<blockquote
					key={key}
					className="border-primary/40 border-l-2 pl-3 opacity-90"
				>
					{block.blocks.map((child, bi) => renderBlock(child, `${key}-q${bi}`))}
				</blockquote>
			);
		case "hr":
			return <div key={key} className="bg-border h-px" />;
	}
}

interface MarkdownViewProps {
	/** Markdown 源文本（notes 等）；空串渲染为空 */
	source: string;
	className?: string;
}

/**
 * 整篇 Markdown 渲染。解析结果按 source 记忆化，弹窗重渲染（下载进度
 * 高频 setState）不会重复解析。
 */
export function MarkdownView({ source, className }: MarkdownViewProps) {
	const blocks = useMemo(() => parseMarkdown(source), [source]);
	if (blocks.length === 0) return null;
	return (
		<div className={cn("flex flex-col gap-2", className)}>
			{blocks.map((block, index) => renderBlock(block, `b${index}`))}
		</div>
	);
}
