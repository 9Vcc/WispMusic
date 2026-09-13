// @unblockneteasemusic/server 的本地 HTTP 包装：由 Tauri (Rust) 作为 sidecar 拉起，
// 监听 127.0.0.1:$UNM_PORT（默认 25863），供渲染进程解灰调用。
// 环境变量与 VutronMusic 的 getAudioSourceFromUnblock 完全一致。
// 接口：
//   GET /healthz                          → 200 "ok"
//   GET /match?songmid=<id>&source=a,b&flac=1&orderFirst=1&qqCookie=..&jooxCookie=..
//     → { code: 200, data: { url, source, br, size, md5 } }
//     → { code: 500, message: "..." }
const http = require("node:http");
const match = require("@unblockneteasemusic/server");

const PORT = Number(process.env.UNM_PORT) || 25863;

// 与 VutronMusic 相同的候选音源（youtube/ytdlp/pyncmd 需外部二进制，不启用）
const DEFAULT_SOURCES = ["bodian", "kuwo", "kugou", "migu", "qq", "bilibili"];

process.env.ENABLE_LOCAL_VIP = process.env.ENABLE_LOCAL_VIP || "true";

const server = http.createServer(async (req, res) => {
	const url = new URL(req.url, `http://127.0.0.1:${PORT}`);
	const send = (status, body) => {
		res.writeHead(status, { "Content-Type": "application/json" });
		res.end(JSON.stringify(body));
	};

	if (url.pathname === "/healthz") {
		return send(200, { code: 200 });
	}

	if (url.pathname === "/match") {
		const songmid = url.searchParams.get("songmid");
		if (!songmid) return send(400, { code: 400, message: "missing songmid" });

		const sourceParam = url.searchParams.get("source");
		const sourceList = (
			sourceParam ? sourceParam.split(",") : DEFAULT_SOURCES
		)
			.map((s) => s.trim().toLowerCase())
			.filter(Boolean);

		// 环境变量与 VutronMusic 的 getAudioSourceFromUnblock 完全一致
		const enableFlac = url.searchParams.get("flac") === "1";
		const orderFirst = url.searchParams.get("orderFirst") === "1";
		process.env.ENABLE_LOCAL_VIP = "true";
		process.env.QQ_COOKIE = url.searchParams.get("qqCookie") || "";
		process.env.JOOX_COOKIE = url.searchParams.get("jooxCookie") || "";
		process.env.ENABLE_FLAC = enableFlac ? "true" : "false";
		process.env.FOLLOW_SOURCE_ORDER = orderFirst ? "true" : "false";

		try {
			const data = await match(songmid, sourceList);
			if (data && typeof data.url === "string" && data.url.startsWith("http")) {
				return send(200, {
					code: 200,
					data: {
						url: data.url,
						source: data.source,
						br: data.br ?? null,
						size: data.size ?? 0,
						md5: data.md5 ?? null,
					},
				});
			}
			return send(200, { code: 500, message: "no available url" });
		} catch (err) {
			return send(200, {
				code: 500,
				message: (err && err.message) || String(err),
			});
		}
	}

	send(404, { code: 404, message: "not found" });
});

// 空闲自动退出：Tauri 侧异常退出时避免遗留孤儿进程
const IDLE_TIMEOUT_MS = 10 * 60 * 1000;
let idleTimer = null;
function resetIdleTimer() {
	if (idleTimer) clearTimeout(idleTimer);
	idleTimer = setTimeout(() => process.exit(0), IDLE_TIMEOUT_MS);
}
resetIdleTimer();

server.on("request", () => {
	// 注意：resetIdleTimer 内部已完整处理 clearTimeout + 重新计时，这里不能重赋值，
	// 否则 idleTimer 会被覆盖为 undefined，clearTimeout 失效导致进程在活跃使用中被误杀
	resetIdleTimer();
});

server.listen(PORT, "127.0.0.1", () => {
	// 就绪标记：Tauri 侧通过轮询 /healthz 判断启动完成
	console.log(`[unm-server] listening on 127.0.0.1:${PORT}`);
});
