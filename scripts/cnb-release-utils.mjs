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
	return sorted.filter((v) => !keepSet.has(v)).map((v) => byVersion.get(v));
}

/**
 * 解析 CNB asset-upload-url 返回的 verify_url 为可直接请求的绝对地址。
 * 实测（2026-09）CNB 的链接渲染层会给地址附加内部路由前缀，形如：
 *   "@share-html#<namespace>:" + "https://..."
 * （Release 附件页复制的下载直链同样带此前缀）；真正的请求地址是前缀之后的
 * 完整 URL。处理规则：
 *   1. 取最后一个 "https://" 起的尾部作为候选（无前缀时即原值）；
 *   2. 缺少 ttl 参数时补 ?ttl=0（官方上限 180 天，0 特指永久保留）；
 *   3. 仍非绝对地址时按相对路径补 apiBase。
 * 若不剥离前缀直接拼接 API 基地址，会得到 "https://api.cnb.coolhttps://..."
 * 之类的坏 URL —— WHATWG 解析后主机名为 "share-html"，DNS 必然失败，
 * fetch 表现为与网络无关的 "fetch failed"（2026-09-14 v0.0.1 首发实锤）。
 * @param {string} raw verify_url 原始值
 * @param {string} [apiBase] API 基地址（默认 https://api.cnb.cool）
 * @returns {string|null} 绝对 URL；入参非法返回 null
 */
export function resolveVerifyUrl(raw, apiBase = "https://api.cnb.cool") {
	if (typeof raw !== "string" || !raw.trim()) return null;
	const idx = raw.lastIndexOf("https://");
	const candidate = (idx > 0 ? raw.slice(idx) : raw).trim();
	const withTtl = /([?&])ttl=/.test(candidate)
		? candidate
		: `${candidate}${candidate.includes("?") ? "&" : "?"}ttl=0`;
	if (/^https?:\/\//i.test(withTtl)) return withTtl;
	const base = apiBase.replace(/\/+$/, "");
	return `${base}${withTtl.startsWith("/") ? withTtl : `/${withTtl}`}`;
}
