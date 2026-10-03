import { invoke, isTauri } from "@tauri-apps/api/core";

/**
 * 把前端的 warn/error 落到安装目录的日志文件里。
 *
 * release 包没有 devtools，也不跑 vite：切歌失败时前端只往 console 喊一声就消失了，
 * 而实测主日志里连一行播放相关的记录都没有（原生侧只有「命令跑完且超过 2s」这类事后
 * 日志，卡住期间一行不响）。这一层是「点了没反应」唯一能留下的现场记录。
 *
 * 只转 warn/error，不转 log：console.log 在渲染热路径上很多，落地会把 5MB 的滚动
 * 窗口冲光、把有用的行挤出去。
 */
const MAX_LINE_CHARS = 600;

/**
 * 每秒最多转这么多条。
 *
 * 限流而不是全量：一处写在 rAF 里的 console.error 就能在几十秒里刷掉整份滚动日志，
 * 反而把要看的那几条冲没了。超限的直接丢弃。
 */
const MAX_LINES_PER_SECOND = 20;

function toText(args: unknown[]): string {
	return args
		.map((arg) => {
			if (typeof arg === "string") return arg;
			if (arg instanceof Error) return `${arg.name}: ${arg.message}`;
			try {
				return JSON.stringify(arg);
			} catch {
				return String(arg);
			}
		})
		.join(" ")
		.slice(0, MAX_LINE_CHARS);
}

let windowStarted = 0;
let windowCount = 0;

function withinRateLimit(now: number): boolean {
	if (now - windowStarted >= 1000) {
		windowStarted = now;
		windowCount = 0;
	}
	windowCount += 1;
	return windowCount <= MAX_LINES_PER_SECOND;
}

export function installLogFileConsoleBridge() {
	if (!isTauri()) return;

	// invoke 自身的失败也会走 console.error，再转发就是无限循环；一旦断了就整体停用
	let broken = false;
	let forwarding = false;

	for (const level of ["warn", "error"] as const) {
		const original = console[level].bind(console);
		console[level] = (...args: unknown[]) => {
			original(...args);
			if (broken || forwarding || !withinRateLimit(Date.now())) return;
			forwarding = true;
			try {
				invoke("log_from_frontend", { level, line: toText(args) }).catch(
					() => {
						broken = true;
					},
				);
			} catch {
				broken = true;
			}
			forwarding = false;
		};
	}
}
