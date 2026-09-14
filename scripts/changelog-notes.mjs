// scripts/changelog-notes.mjs
// 从 .github/workflows/CHANGELOG.md 提取指定版本的更新说明，供 publish-cnb.mjs
// 写入 update.json 的 notes 字段（Tauri updater 清单的官方可选字段）。
//
// 约定的段落格式（## 级标题）：
//   ## v1.0.2   —— 精确版本（v 前缀可省略；构建号 +x 在匹配时忽略，
//                  即应用版本为 1.0.2+1 时也能命中 "## v1.0.2"）
//   ## 未发布   —— 尚未定版的变更；发版前把标题改成对应版本号即可
//                （也兼容 "## Unreleased"）
// 匹配顺序：归一化版本精确匹配 → 未发布/Unreleased 段落 → 空串
//（空串由调用方决定占位文案）。
// 本模块为纯函数 + 文件读取薄封装，无其他副作用，可被测试直接导入。

import { readFile } from "node:fs/promises";

const UNRELEASED_TITLES = new Set(["未发布", "unreleased"]);

/** 归一化用于匹配的版本 key：去前导 v/V、去掉构建号（+ 及之后内容） */
function normalizeVersionKey(text) {
	return String(text ?? "")
		.trim()
		.replace(/^[vV]/, "")
		.replace(/\+.*$/, "");
}

/** 按 ## 标题切分 markdown，返回 { title, body } 数组（标题保留原文） */
function splitSections(markdown) {
	const sections = [];
	let title = null;
	let buffer = [];
	const flush = () => {
		if (title !== null) sections.push({ title, body: buffer.join("\n").trim() });
	};
	for (const line of String(markdown ?? "").split(/\r?\n/)) {
		const match = /^##\s+(.+?)\s*$/.exec(line);
		if (match) {
			flush();
			title = match[1];
			buffer = [];
		} else if (title !== null) {
			buffer.push(line);
		}
	}
	flush();
	return sections;
}

/**
 * 提取版本对应的更新说明。
 * 命中顺序：归一化版本精确匹配 → 未发布/Unreleased 段落 → 空串。
 */
export function extractChangelogNotes(markdown, version) {
	const sections = splitSections(markdown);
	const target = normalizeVersionKey(version);

	let notes = sections.find(
		(section) => normalizeVersionKey(section.title) === target,
	)?.body;
	if (!notes) {
		const unreleased = sections.find((section) =>
			UNRELEASED_TITLES.has(section.title.toLowerCase()),
		);
		notes = unreleased?.body;
	}
	return (notes ?? "").trim();
}

/** 读取 CHANGELOG.md 并提取；文件缺失/读取失败时告警并返回空串（不阻断发布） */
export async function readChangelogNotes(changelogUrl, version) {
	try {
		const markdown = await readFile(changelogUrl, "utf8");
		return extractChangelogNotes(markdown, version);
	} catch (err) {
		console.warn(
			`⚠ 读取 CHANGELOG.md 失败（将使用占位更新说明）：${err?.message ?? err}`,
		);
		return "";
	}
}
