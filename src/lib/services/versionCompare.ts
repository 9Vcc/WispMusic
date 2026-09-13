/**
 * 宽松 semver 版本比较（仅用于更新检查的防御性校验）。
 *
 * updater 插件自身会完成主比较，这里兜的是边界情况：
 * - 前导 v/V（"v1.0.2"）
 * - 缺失的数字段（"1.2" 等价 "1.2.0"）
 * - build 元数据（"1.0.1+build.5" 忽略 + 号后内容）
 * - 预发布版本（"1.0.1-beta.1" 低于 "1.0.1"，按点分段、数字段按数值比较，
 *   数字段恒低于字符串段，遵循 semver 优先级规则）
 */

interface ParsedVersion {
	core: number[];
	prerelease: (number | string)[];
}

function parseVersion(input: string): ParsedVersion | null {
	if (typeof input !== "string") return null;
	let text = input.trim().replace(/^[vV]/, "");

	const plusIndex = text.indexOf("+");
	if (plusIndex >= 0) text = text.slice(0, plusIndex);

	const dashIndex = text.indexOf("-");
	const coreText = dashIndex >= 0 ? text.slice(0, dashIndex) : text;
	const prereleaseText = dashIndex >= 0 ? text.slice(dashIndex + 1) : "";

	if (coreText.length === 0) return null;
	const core: number[] = [];
	for (const segment of coreText.split(".")) {
		const value = Number.parseInt(segment, 10);
		if (Number.isNaN(value)) return null;
		core.push(value);
	}

	const prerelease: (number | string)[] = [];
	if (prereleaseText.length > 0) {
		for (const segment of prereleaseText.split(".")) {
			prerelease.push(
				/^\d+$/.test(segment) ? Number.parseInt(segment, 10) : segment,
			);
		}
	}

	return { core, prerelease };
}

/**
 * 比较两个版本号：a 大于 b 返回正数，小于返回负数，相等（含无法解析）返回 0。
 * 无法解析时返回 0，由上层 isVersionNewer 决定兜底策略。
 */
export function compareVersions(a: string, b: string): number {
	const left = parseVersion(a);
	const right = parseVersion(b);
	if (!left || !right) return 0;

	const segmentCount = Math.max(left.core.length, right.core.length);
	for (let i = 0; i < segmentCount; i++) {
		const x = left.core[i] ?? 0;
		const y = right.core[i] ?? 0;
		if (x !== y) return x - y;
	}

	if (left.prerelease.length === 0 && right.prerelease.length === 0) return 0;
	if (left.prerelease.length === 0) return 1;
	if (right.prerelease.length === 0) return -1;

	const prereleaseCount = Math.max(
		left.prerelease.length,
		right.prerelease.length,
	);
	for (let i = 0; i < prereleaseCount; i++) {
		const x = left.prerelease[i];
		const y = right.prerelease[i];
		if (x === undefined) return -1;
		if (y === undefined) return 1;
		if (typeof x === "number" && typeof y === "number") {
			if (x !== y) return x - y;
		} else if (typeof x === "number") {
			return -1;
		} else if (typeof y === "number") {
			return 1;
		} else if (x !== y) {
			return x < y ? -1 : 1;
		}
	}
	return 0;
}

/**
 * candidate 是否比 current 更新。任一版本号无法解析（理论上不该发生，
 * update.json 由发布脚本生成、当前版本来自 getVersion()）时按"有更新"兜底，
 * 信任 updater 插件自身的判断，避免解析失误漏掉真实更新。
 */
export function isVersionNewer(candidate: string, current: string): boolean {
	const parsedCandidate = parseVersion(candidate);
	const parsedCurrent = parseVersion(current);
	if (!parsedCandidate || !parsedCurrent) return true;
	return compareVersions(candidate, current) > 0;
}
