// scripts/publish-cnb.mjs
// 将 Tauri 构建产物发布到 CNB（cnb.cool）公开更新仓库，完成更新链路的发布侧：
//   1. 从 src-tauri/target/release/bundle/nsis 收集更新工件与签名
//      （Tauri v2 原生格式：*-setup.exe + .exe.sig；v1 兼容格式：*.nsis.zip + .nsis.zip.sig）
//   2. 安装包 + 签名作为「CNB Release 附件」上传（不再进入 git 仓库）：
//      幂等删建 Release（tag = 本次发布 tag）→ asset-upload-url 拿预签名地址
//      → PUT 文件本体 → 确认上传（ttl=0 永久）；最终用户可直接从 Release
//      附件页（或 /-/releases/download/<tag>/<文件名> 直链）下载安装包
//   3. 滚动保留最近 KEEP_VERSIONS 个 Release（含附件），更旧的整 Release 删除
//   4. 覆写仓库根目录 update.json（git push，仅 1KB 清单文件）：
//      platforms.url 指向 Release 附件直链。update.json 必须留在仓库 main 分支 ——
//      客户端 endpoint 编译期固化为 https://cnb.cool/<CNB_REPO>/-/git/raw/main/update.json，
//      已发布版本据此检查更新，此路径在存量用户升级完成前不可移除
//      （注意 raw 直链格式是 /-/git/raw/，不是 /-/raw/——后者会被前端 SPA 接管）
// 由 .github/workflows/release.yml 在 tauri-action 之后调用；也可本地手动运行：
//   CNB_TOKEN=xxx CNB_REPO=MR9vcc/wisp-updater TAG_NAME=v0.0.1 node scripts/publish-cnb.mjs
//   DRY_RUN=1 时只演练：真实执行只读 API 与工件收集，打印将执行的全部写操作
// 依赖：Node 22+ 与 git（runner 自带）；零第三方依赖。
//
// 为什么是 CNB 而不是 Gitee：Gitee 已将 raw 外链判定为「外链滥用」并锁定仓库公开权限
// （2026-09-13 实锤）；CNB 对程序化客户端零拦截（实测匿名 curl 200），且 Release 附件
// 直链 /-/releases/download/<tag>/<文件名> 匿名可下载（已实测）。
// CNB OpenAPI：基础域名 https://api.cnb.cool，鉴权 Authorization: Bearer <CNB_TOKEN>，
// 写 Release 需要 token 具备 repo-release:rw 权限（403 errcode=10023 即缺此权限）。
// 安装包不再 git push 的原因：仓库随版本累加二进制会无限膨胀，Release 附件由平台托管、
// 不占 git 体积，且天然带版本页与下载计数。

import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { readdir, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readChangelogNotes } from "./changelog-notes.mjs";
import {
	KEEP_VERSIONS,
	pruneOldReleaseTags,
	resolveVerifyUrl,
} from "./cnb-release-utils.mjs";

const TOKEN = process.env.CNB_TOKEN;
const REPO = process.env.CNB_REPO;
const TAG = process.env.TAG_NAME;
const DRY_RUN = process.env.DRY_RUN === "1";
const API = "https://api.cnb.cool";

function die(msg) {
	console.error(`✗ ${msg}`);
	process.exit(1);
}

// 演练模式允许无 token（写操作一律不真实执行）；正式运行必须有。
// 演练 + 有 token 时仍会执行只读探测（查同 tag Release / 列 Release），
// 让演练输出尽量贴近真实将执行的动作。
if (!TOKEN && !DRY_RUN) die("缺少环境变量 CNB_TOKEN");
const readonlyOk = Boolean(TOKEN);
if (!REPO || !/^[^/]+\/[^/]+$/.test(REPO)) {
	die("CNB_REPO 必须是 group/repo 形式（如 MR9vcc/wisp-updater）");
}
if (!TAG) die("缺少环境变量 TAG_NAME（CI 中取 github.ref_name）");

const conf = JSON.parse(
	await readFile(
		new URL("../src-tauri/tauri.conf.json", import.meta.url),
		"utf8",
	),
);
const VERSION = conf.version;
if (!VERSION) die("tauri.conf.json 缺少 version 字段");
if (TAG !== `v${VERSION}`) {
	console.warn(
		`⚠ tag(${TAG}) 与应用版本(v${VERSION})不一致；update.json 以应用版本 ${VERSION} 为准，附件挂到 Release ${TAG}`,
	);
}

// ---- 解析更新说明：CHANGELOG.md 版本段落 → 未发布段落 → 占位文案 ----
const changelogNotes = await readChangelogNotes(
	new URL("../.github/workflows/CHANGELOG.md", import.meta.url),
	VERSION,
);
const notesText = changelogNotes || `WispMusic v${VERSION}`;
if (changelogNotes) {
	console.log("→ 更新说明来源：CHANGELOG.md");
} else {
	console.warn(
		`⚠ CHANGELOG.md 未命中 v${VERSION} 或「未发布」段落，更新说明使用占位文案`,
	);
}

// ---- 收集更新工件 ----
const nsisDir = join(
	process.cwd(),
	"src-tauri",
	"target",
	"release",
	"bundle",
	"nsis",
);
if (!existsSync(nsisDir)) {
	die(`未找到 NSIS 产物目录 ${nsisDir}\n  请在 tauri build 完成后运行本脚本`);
}
const entries = await readdir(nsisDir);
const sigs = entries.filter((f) => f.endsWith(".sig"));
// 先找 v2 原生格式（*-setup.exe），再找 v1 兼容格式（*.nsis.zip），均要求有同名 .sig
const artifactName =
	entries.find((f) => f.endsWith("-setup.exe") && sigs.includes(`${f}.sig`)) ??
	entries.find((f) => f.endsWith(".nsis.zip") && sigs.includes(`${f}.sig`));
if (!artifactName) {
	die(
		"未找到成对的更新工件（*-setup.exe + .exe.sig 或 *.nsis.zip + .nsis.zip.sig）\n" +
			`  目录内容：${entries.join(", ")}`,
	);
}
const sigName = `${artifactName}.sig`;
const sigContent = (await readFile(join(nsisDir, sigName), "utf8")).trim();
console.log(`→ 更新工件：${artifactName} + ${sigName}`);

// ---- CNB OpenAPI 封装（Bearer 鉴权；错误统一带权限排查提示） ----
function authHeader() {
	return { Authorization: `Bearer ${TOKEN}` };
}

function maybePermissionHint(status, bodyText) {
	if (status === 401) {
		return "\n  排查：CNB_TOKEN 无效或已过期（检查 secrets.CNB_TOKEN 的值与有效期）";
	}
	if (status === 403) {
		return (
			"\n  排查：CNB_TOKEN 需包含 repo-release:rw 权限" +
			"（CNB 设置 → 访问令牌 → 勾选「版本 repo-release」读写；CI secrets.CNB_TOKEN 同步更新）" +
			(/10023/.test(bodyText) ? "（当前 token 已确认缺该 scope）" : "")
		);
	}
	return "";
}

/** 提取 undici 网络错误的底层原因（error.cause，如 ENOTFOUND/ECONNRESET），便于定位 */
function netErrDetail(error) {
	const cause = error?.cause;
	if (cause?.code) return `（${cause.code}）`;
	if (typeof cause === "string") return `（${cause}）`;
	if (cause?.message) return `（${String(cause.message).slice(0, 120)}）`;
	return "";
}

async function cnbApi(
	pathname,
	{ method = "GET", body, retry = 2, allowNotFound = false } = {},
) {
	let lastErr;
	for (let attempt = 0; attempt <= retry; attempt++) {
		try {
			// verify_url 等响应会给出完整绝对地址（可能带内部路由前缀，经 resolveVerifyUrl 剥离）；
			// 绝对地址直接请求，相对路径才补 API 前缀——重复拼接会产生坏 URL，DNS 直接失败
			const url = /^https?:\/\//i.test(pathname)
				? pathname
				: `${API}${pathname}`;
			const res = await fetch(url, {
				method,
				headers: {
					Accept: "application/vnd.cnb.api+json",
					...(body ? { "Content-Type": "application/json" } : {}),
					...authHeader(),
				},
				body: body ? JSON.stringify(body) : undefined,
			});
			const text = await res.text();
			if (res.ok) return text ? JSON.parse(text) : null;
			if (allowNotFound && res.status === 404) return null;
			if (res.status >= 500 && attempt < retry) {
				lastErr = new Error(`HTTP ${res.status}: ${text.slice(0, 300)}`);
				continue; // 5xx 重试
			}
			die(
				`CNB API ${method} ${pathname} 失败（HTTP ${res.status}）：${text.slice(0, 500)}` +
					maybePermissionHint(res.status, text),
			);
		} catch (error) {
			lastErr = error;
			if (attempt < retry) {
				const wait = 10_000 * (attempt + 1);
				console.warn(
					`⚠ CNB API ${method} ${pathname} 网络异常（第 ${attempt + 1}/${retry + 1} 次）：${error.message}${netErrDetail(error)}，${wait / 1000}s 后重试…`,
				);
				await new Promise((r) => setTimeout(r, wait));
			}
		}
	}
	die(
		`CNB API ${method} ${pathname} 失败（已重试）：${lastErr?.message}${netErrDetail(lastErr)}` +
			"\n  提示：网络异常若为 ENOTFOUND，多半是请求 URL 被拼坏（对照上方 pathname 检查）或代理/DNS 拦截",
	);
}

/** Release 附件下载直链（匿名可访问，已实测；{tag} 为 Release 标签） */
function assetDownloadUrl(asset) {
	return `https://cnb.cool/${REPO}/-/releases/download/${encodeURIComponent(TAG)}/${encodeURIComponent(asset)}`;
}

// ---- 发布 Release：幂等删建 + 附件三步上传 ----
async function findReleaseByTag(tag) {
	// 首次发布时该 tag 必然不存在（404），属正常路径而非错误
	return await cnbApi(`/${REPO}/-/releases/tags/${encodeURIComponent(tag)}`, {
		retry: 1,
		allowNotFound: true,
	});
}

/**
 * 上传单个附件：拿预签名地址 → PUT 文件本体 → 确认入库（ttl=0 永久）。
 * 官方流程：POST asset-upload-url 返回 {upload_url, verify_url}；
 * verify_url 为确认端点，实测可能带 CNB 内部路由前缀（"@share-html#<namespace>:"），
 * 必须经 resolveVerifyUrl 剥离成绝对地址后再请求——2026-09-14 v0.0.1 首发时曾因
 * 直接拼接 API 前缀产生 "https://api.cnb.coolhttps://..." 坏 URL（DNS 失败，
 * 表现为重试 3 次全是 fetch failed）。
 */
async function uploadAsset(releaseId, filePath, assetName, size) {
	if (DRY_RUN) {
		console.log(
			`→ [演练] 将上传附件：${assetName}（${(size / 1024 / 1024).toFixed(1)} MB，预签名三步：upload-url → PUT → confirmation?ttl=0）`,
		);
		return;
	}
	const up = await cnbApi(`/${REPO}/-/releases/${releaseId}/asset-upload-url`, {
		method: "POST",
		body: { asset_name: assetName, size, overwrite: true },
	});
	const uploadUrl = up?.upload_url;
	const verifyUrl = up?.verify_url;
	if (!uploadUrl || !verifyUrl) {
		die(
			`asset-upload-url 响应缺少 upload_url/verify_url：${JSON.stringify(up).slice(0, 400)}`,
		);
	}
	let uploadHost = "?";
	try {
		uploadHost = new URL(uploadUrl).host;
	} catch {}
	// 把两个地址的原始形态打进日志：verify_url 格式再变时可立即从 CI 日志定位
	console.log(
		`→ asset-upload-url：upload_url 主机=${uploadHost}；verify_url 原始值=${String(verifyUrl).slice(0, 400)}`,
	);

	const content = await readFile(filePath);
	for (let attempt = 1; ; attempt++) {
		try {
			const res = await fetch(uploadUrl, {
				method: "PUT",
				headers: { "Content-Type": "application/octet-stream" },
				body: content,
			});
			if (res.ok) break;
			if (attempt >= 3) {
				die(
					`附件 ${assetName} 上传失败（HTTP ${res.status}）：${(await res.text()).slice(0, 300)}`,
				);
			}
			console.warn(
				`⚠ 附件 ${assetName} 上传 HTTP ${res.status}，重试 ${attempt}/3…`,
			);
		} catch (error) {
			if (attempt >= 3) {
				die(`附件 ${assetName} 上传网络异常（已重试）：${error.message}`);
			}
			console.warn(
				`⚠ 附件 ${assetName} 上传网络异常（${error.message}），重试 ${attempt}/3…`,
			);
		}
		await new Promise((r) => setTimeout(r, 5000 * attempt));
	}

	// 确认上传：verify_url 可能带 "@share-html#<namespace>:" 内部路由前缀，须剥离后再请求
	const confirmUrl = resolveVerifyUrl(verifyUrl, API);
	if (!confirmUrl) {
		die(
			`verify_url 无法解析为可请求地址：${JSON.stringify(verifyUrl).slice(0, 200)}`,
		);
	}
	if (confirmUrl !== verifyUrl) {
		console.log(`→ verify_url 已规范化，确认端点：${confirmUrl}`);
	}
	await cnbApi(confirmUrl, { method: "POST" });
	console.log(
		`✓ 附件已上传：${assetName}（${(size / 1024 / 1024).toFixed(1)} MB）`,
	);
}

async function listAllReleases() {
	if (!readonlyOk) return [];
	const all = [];
	for (let page = 1; page <= 20; page++) {
		const body = await cnbApi(
			`/${REPO}/-/releases?page=${page}&page_size=100`,
			{ retry: 1 },
		);
		const list = Array.isArray(body) ? body : (body?.data ?? body?.list ?? []);
		all.push(...list);
		if (!Array.isArray(list) || list.length < 100) break;
	}
	return all;
}

if (DRY_RUN)
	console.log("== DRY_RUN 演练模式：以下写操作仅打印，不会真实执行 ==\n");

// 1) 幂等清理：同 tag 的旧 Release（上次重跑残留）先删除，保证本次产物即最终状态
const existing = readonlyOk ? await findReleaseByTag(TAG) : null;
if (existing?.id != null) {
	console.log(`→ 发现已存在的 Release ${TAG}（id=${existing.id}），删除重建…`);
	DRY_RUN ||
		(await cnbApi(`/${REPO}/-/releases/${existing.id}`, { method: "DELETE" }));
} else if (DRY_RUN) {
	console.log(
		`→ [演练] 未发现已存在的 Release ${TAG}（或无 token 跳过探测），将直接创建`,
	);
}

// 2) 创建 Release（tag 不存在时由平台按 target_commitish 自动建 tag，与 GitHub 行为一致）
const release = DRY_RUN
	? { id: "<dry-run-release-id>" }
	: await cnbApi(`/${REPO}/-/releases`, {
			method: "POST",
			body: {
				tag_name: TAG,
				name: `WispMusic v${VERSION}`,
				body: notesText,
				target_commitish: "main",
				prerelease: false,
				draft: false,
				// make_latest：latest 语义显式锚定到本次 Release（否则按平台默认排序，
				// 删除重建的旧 Release 可能占据 /-/releases/latest/... 直链）
				make_latest: "true",
			},
		});
const releaseId = release?.id;
if (releaseId == null) {
	die(
		`创建 Release 失败：响应缺少 id：${JSON.stringify(release).slice(0, 400)}`,
	);
}
console.log(
	`✓ Release 就绪：${TAG}（id=${releaseId}）「WispMusic v${VERSION}」${DRY_RUN ? "（演练）" : ""}`,
);

// 3) 上传安装包 + 签名（附件名与既有产物命名保持一致）
// 先传极小的 .sig 走通三步链路（确认端点若有问题立刻暴露，不必等 100MB 的安装包传完）
const exePath = join(nsisDir, artifactName);
const sigPath = join(nsisDir, sigName);
await uploadAsset(
	releaseId,
	sigPath,
	sigName,
	(await readFile(sigPath)).length,
);
await uploadAsset(
	releaseId,
	exePath,
	artifactName,
	(await readFile(exePath)).length,
);

// 4) 滚动保留：删除超出 KEEP_VERSIONS 的旧 Release（当前版本强制保留）
const allReleases = await listAllReleases();
const tagOf = (r) => r?.tag_name ?? r?.tag ?? r?.tagName;
const tags = allReleases.map(tagOf).filter(Boolean);
const doomedTags = pruneOldReleaseTags(tags, KEEP_VERSIONS, TAG);
if (doomedTags.length > 0) {
	const idOf = new Map(allReleases.map((r) => [tagOf(r), r?.id]));
	for (const tag of doomedTags) {
		const id = idOf.get(tag);
		console.log(
			`→ 滚动清理旧 Release：${tag}（保留最近 ${KEEP_VERSIONS} 个版本）`,
		);
		if (id != null)
			DRY_RUN ||
				(await cnbApi(`/${REPO}/-/releases/${id}`, { method: "DELETE" }));
	}
} else {
	console.log(`→ Release 数未超过 ${KEEP_VERSIONS} 个，无需清理`);
}

// ---- 构造 update.json ----
// 安装包直链：Release 附件固定 URL（/releases/download/<tag>/<文件名>，匿名可访问）
const artifactUrl = assetDownloadUrl(artifactName);
const manifest = {
	version: VERSION,
	notes: notesText,
	pub_date: new Date().toISOString(),
	platforms: {
		"windows-x86_64": {
			signature: sigContent,
			url: artifactUrl,
		},
	},
};
const manifestText = `${JSON.stringify(manifest, null, 2)}\n`;
console.log(`→ 安装包下载地址：${artifactUrl}`);

// ---- git push 更新 update.json（仅 1KB 清单；安装包已走 Release 附件，不再进仓库） ----
// CNB 认证：HTTPS Basic，用户名固定 cnb，密码=访问令牌（docs.cnb.cool/zh/guide/git-access.html）
const authUrl = `https://cnb:${encodeURIComponent(TOKEN ?? "dry-run")}@cnb.cool/${REPO}.git`;

function git(cwd, args) {
	const label = `git ${args.join(" ")}`;
	const res = spawnSync("git", args, {
		cwd,
		encoding: "utf8",
		// 令牌嵌在 remote URL 里，绝不能进日志
		stdio: ["ignore", "pipe", "pipe"],
	});
	if (res.status !== 0) {
		const errOutput = `${res.stderr ?? ""}${res.stdout ?? ""}`.trim();
		die(`${label} 失败（exit ${res.status}）：\n${errOutput.slice(0, 800)}`);
	}
	return res;
}

function gitAllowFail(cwd, args) {
	const res = spawnSync("git", args, { cwd, encoding: "utf8" });
	return res.status === 0;
}

const workDir = mkdtempSync(join(tmpdir(), "wisp-updater-"));
const repoDir = join(workDir, "repo");

// 演练模式：不 clone / 不 push（本地往往没有仓库写凭证），直接展示将写入的清单
if (DRY_RUN) {
	console.log("→ [演练] 将写入仓库根目录 update.json：");
	console.log(manifestText);
	console.log(`✓ [演练] 完成。Release 页：https://cnb.cool/${REPO}/-/releases`);
	process.exit(0);
}

try {
	// 浅克隆：只拉最近一次提交（仓库里已无二进制，体积稳定）
	console.log(`→ clone 更新仓库：cnb.cool/${REPO}`);
	let cloned = false;
	for (let attempt = 1; attempt <= 3 && !cloned; attempt++) {
		const res = spawnSync("git", ["clone", "--depth", "1", authUrl, repoDir], {
			encoding: "utf8",
		});
		if (res.status === 0) {
			cloned = true;
		} else if (attempt < 3) {
			const wait = 10_000 * attempt;
			console.warn(
				`⚠ clone 失败（第 ${attempt}/3 次）：${String(res.stderr).trim().slice(0, 300)}，${wait / 1000}s 后重试…`,
			);
			await new Promise((r) => setTimeout(r, wait));
		} else {
			die(
				`clone 更新仓库失败（已重试 3 次）：\n${String(res.stderr).trim().slice(0, 600)}\n` +
					"  排查：CNB_TOKEN 是否有效、是否勾选该仓库写权限（repo-code:rw）、CNB_REPO 是否为 group/repo",
			);
		}
	}

	// 提交身份（CI bot）
	git(repoDir, ["config", "user.name", "wisp-updater-bot"]);
	git(repoDir, [
		"config",
		"user.email",
		"wisp-updater-bot@users.noreply.cnb.cool",
	]);

	// update.json → 根目录（客户端固定读取）
	writeFileSync(join(repoDir, "update.json"), manifestText, "utf8");

	// 一次性清理：旧「安装包进仓库」方案遗留的 releases/ 二进制目录。
	// 历史安装包已由 Release 附件承载，git 仓库从此只保留 update.json 清单
	const legacyDir = join(repoDir, "releases");
	if (existsSync(legacyDir)) {
		console.log(
			"→ 清理仓库遗留的 releases/ 二进制目录（历史安装包已由 Release 附件承载）",
		);
		rmSync(legacyDir, { recursive: true, force: true });
	}

	git(repoDir, ["add", "-A"]);
	if (!gitAllowFail(repoDir, ["diff", "--cached", "--quiet"])) {
		git(repoDir, [
			"commit",
			"-m",
			`chore(updater): publish v${VERSION} (${TAG}) via release assets`,
		]);
		// 推到当前分支（= 远端默认分支）
		console.log("→ push update.json 到 CNB…");
		git(repoDir, ["push", "origin", "HEAD"]);
		console.log("✓ 已推送（update.json）");
	} else {
		console.log("✓ 内容无变化（同版本重跑），跳过 push");
	}

	console.log(`✓ 完成。Release 页：https://cnb.cool/${REPO}/-/releases`);
	console.log(
		`  客户端 endpoint = https://cnb.cool/${REPO}/-/git/raw/main/update.json（不变）`,
	);
} finally {
	rmSync(workDir, { recursive: true, force: true });
}
