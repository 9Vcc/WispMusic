// scripts/cnb-release-utils.mjs
// CNB 更新仓库 Release 的滚动保留纯函数（无副作用，供 publish-cnb.mjs 与本地测试使用）。

/** Release（含附件）保留的版本数上限 */
export const KEEP_VERSIONS = 5;

/** 从产物文件名提取 semver（WispMusic_0.0.1_x64-setup.exe → 0.0.1；提取失败返回 null） */
export function parseArtifactVersion(name) {
	const m = name.match(/_(\d+\.\d+\.\d+)_/);
	return m ? m[1] : null;
}

/**
 * 从 Release 标签提取 semver（v1.0.2 / 1.0.2 / v1.0.2+build → 1.0.2；提取失败返回 null）。
 * 只取主版本三段，构建号与预发布后缀不参与比较（与 versionCompare 的宽松口径一致）。
 */
export function parseTagVersion(tag) {
	if (typeof tag !== "string") return null;
	const m = tag.match(/v?(\d+)\.(\d+)\.(\d+)/);
	return m ? `${m[1]}.${m[2]}.${m[3]}` : null;
}

function compareVersionDesc(a, b) {
	const pa = a.split(".").map(Number);
	const pb = b.split(".").map(Number);
	for (let i = 0; i < 3; i++) {
		if (pa[i] !== pb[i]) return pb[i] - pa[i];
	}
	return 0;
}

/**
 * 计算应删除的旧 Release 标签（Release 方案：每个版本一个 Release，附件随 Release 存续）。
 * @param {string[]} tags 更新仓库现有的全部 Release 标签名
 * @param {number} keepCount 保留的版本数
 * @param {string} currentTag 本次发布的标签（强制保留，绝不被清理）
 * @returns {string[]} 待删除标签列表（无法解析版本号的标签一律不动）
 */
export function pruneOldReleaseTags(tags, keepCount, currentTag) {
	const currentVersion = parseTagVersion(currentTag);
	const byVersion = new Map();
	for (const tag of tags) {
		const v = parseTagVersion(tag);
		if (!v) continue; // 无法识别版本的标签一律不动
		if (!byVersion.has(v)) byVersion.set(v, tag);
	}
	const sorted = [...byVersion.keys()].sort(compareVersionDesc);
	const keepSet = new Set(sorted.slice(0, keepCount));
	if (currentVersion) keepSet.add(currentVersion);
	return sorted
		.filter((v) => !keepSet.has(v))
		.map((v) => byVersion.get(v));
}
