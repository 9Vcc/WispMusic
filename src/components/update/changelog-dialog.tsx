import { useMemo } from "react";
import {
	Dialog,
	DialogBody,
	DialogCancel,
	DialogContent,
	DialogFooter,
	DialogTitle,
} from "@/components/ui/dialog";
import changelogRaw from "../../../.github/workflows/CHANGELOG.md?raw";

interface ChangelogSection {
	/** 小节标题（### 新增功能）；无标题的散落文本为 null */
	heading: string | null;
	items: string[];
}

interface ChangelogGroup {
	/** 版本段落标题（## v0.0.2 → "v0.0.2"） */
	version: string;
	sections: ChangelogSection[];
}

/**
 * 解析 CHANGELOG.md（构建期以 ?raw 打包，零网络依赖）。
 *
 * 只保留版本段落：第一个 ## 之前的文件头说明（面向维护者的解析规则描述）
 * 全部跳过；没有任何内容的版本段落（如空的「未发布」）被过滤。
 */
function parseChangelog(raw: string): ChangelogGroup[] {
	const groups: ChangelogGroup[] = [];
	let current: ChangelogGroup | null = null;
	for (const line of raw.split(/\r?\n/)) {
		const text = line.trim();
		if (!text) continue;
		if (text.startsWith("## ")) {
			current = { version: text.slice(3).trim(), sections: [] };
			groups.push(current);
			continue;
		}
		if (!current) continue;
		if (text.startsWith("# ")) continue; // 文件主标题
		if (text.startsWith("### ")) {
			current.sections.push({ heading: text.slice(4).trim(), items: [] });
			continue;
		}
		if (text.startsWith("- ")) {
			const section =
				current.sections.length > 0
					? current.sections[current.sections.length - 1]
					: undefined;
			if (!section) {
				// 条目前无小节：兜底建一个无标题分组
				const created: ChangelogSection = { heading: null, items: [] };
				current.sections.push(created);
				created.items.push(text.slice(2).trim());
				continue;
			}
			section.items.push(text.slice(2).trim());
			continue;
		}
		// 非条目正文（固定结构中不会出现在版本段内）：独立无标题分组兜底
		current.sections.push({ heading: null, items: [text] });
	}
	// 过滤空版本段落（如空的「未发布」）
	return groups.filter((g) => g.sections.some((s) => s.items.length > 0));
}

interface ChangelogDialogProps {
	open: boolean;
	onOpenChange: (open: boolean) => void;
}

/**
 * 「更新日志」弹窗：展示 CHANGELOG.md 全部版本记录。
 * 内容随应用打包（?raw 导入），离线可用、无需请求更新源。
 */
export function ChangelogDialog({ open, onOpenChange }: ChangelogDialogProps) {
	const groups = useMemo(() => parseChangelog(changelogRaw), []);

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className="max-w-md">
				<DialogTitle>更新日志</DialogTitle>
				<DialogBody>
					<div className="no-scrollbar max-h-96 overflow-y-auto rounded-md bg-fill-subtle p-3">
						{groups.map((group) => (
							<div
								key={group.version}
								className="mt-3 first:mt-0 flex flex-col gap-1.5"
							>
								<div className="text-foreground font-medium text-sm">
									{group.version}
								</div>
								{group.sections.map((section, si) => (
									<div
										// 每个版本内小节标题唯一，version+heading+序号作 key
										key={`${group.version}-${section.heading ?? "misc"}-${si}`}
										className="flex flex-col gap-1"
									>
										{section.heading && (
											<div className="text-muted-foreground text-xs">
												{section.heading}
											</div>
										)}
										{section.items.map((item, ii) => (
											<div
												key={`${ii}-${item}`}
												className="flex items-start gap-2 text-muted-foreground text-sm"
											>
												<span className="mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full bg-primary/60" />
												<span>{item}</span>
											</div>
										))}
									</div>
								))}
							</div>
						))}
					</div>
				</DialogBody>
				<DialogFooter>
					<DialogCancel>关闭</DialogCancel>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	);
}
