import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import React from "react";
import ReactDOM from "react-dom/client";
import { SWRConfig } from "swr";
import App from "./App";
import "./index.css";
import { initGlobalShortcuts } from "./lib/shortcuts/global-shortcut-manager";
import { initLocalMusic } from "./lib/store/localMusicStore/localMusicStore";
import { initSettings } from "./lib/store/settingStore/settingStore";
import { createSwrLruCache } from "./lib/utils/swr-cache";

const settingsReady = initSettings();
settingsReady.then(() => initGlobalShortcuts());

/**
 * 解灰预热：把本地解灰 sidecar（Node + unm 引擎）的冷启动提前到首帧空闲时。
 *
 * 首次播放灰色歌曲时 unm_match 要同步等一整条链路：拉起 Node 进程 → require
 * 解灰引擎（十几个 provider）→ release 还要解压内嵌的运行时。这部分与「听哪首歌」
 * 无关，可以提前付掉；真实匹配的网络往返不在预热范围内（那只能等）。
 *
 * 只在解灰开关打开时预热——关掉解灰的用户白开一个 Node 进程不划算。
 * 失败静默：真正播放时 unm_match 会再走一次完整的按需拉起流程。
 */
let warmupStarted = false;

async function warmupUnblock() {
	if (warmupStarted) return;
	// 等设置水合：解灰开关是用户设置，水合前读到的是默认值
	await settingsReady.catch(() => undefined);
	const { useSettingStore } = await import(
		"./lib/store/settingStore/settingStore"
	);
	if (!useSettingStore.getState().network.unblockEnabled) return;
	// 两个入口（空闲 + 首次交互）可能并发进入，这里再确认一次
	if (warmupStarted) return;
	warmupStarted = true;
	await invoke("unm_warmup");
}

function runWarmupQuietly() {
	void warmupUnblock().catch((error) => {
		console.warn("[unm] 解灰引擎预热失败（首次播放时会按需重试）:", error);
	});
}

/**
 * 空闲回调可能被启动期的重活推后（同一个回调里还有本地曲库水合），
 * 于是补一个入口：用户第一次交互就把 sidecar 拉起来，不用等真点了灰歌才现开进程。
 */
function registerWarmupOnFirstInteraction() {
	const trigger = () => {
		window.removeEventListener("pointerdown", trigger);
		window.removeEventListener("keydown", trigger);
		runWarmupQuietly();
	};
	window.addEventListener("pointerdown", trigger, { passive: true });
	window.addEventListener("keydown", trigger);
}

if (isTauri() && getCurrentWindow().label === "main") {
	// 本地音乐库可能有数 MB JSON 要过 IPC，推迟到首帧空闲再水合，
	// 且仅主窗口执行，避免 tray-menu 窗口重复加载一遍
	const idle = (cb: () => void) =>
		"requestIdleCallback" in window
			? window.requestIdleCallback(() => cb())
			: setTimeout(cb, 200);
	idle(() => {
		// 先发起预热再水合本地曲库：水合是同步重活，反过来会把 sidecar 的
		// 拉起动作堵在它后面（预热本身是异步的，先发起即可后台并行）
		runWarmupQuietly();
		registerWarmupOnFirstInteraction();
		initLocalMusic();
	});

	// 主窗口以 visible: false 启动，待设置（主题/材质）随首帧应用后再显示；
	// 双 rAF 确保首帧已提交，超时兜底保证前端异常时窗口也能出现
	let shown = false;
	const show = () => {
		if (shown) return;
		shown = true;
		const win = getCurrentWindow();
		win
			.show()
			// 上报「主窗口已显示」：Rust 侧的启动兜底线程据此收手。兜底若只看
			// 可见性，用户稍后关闭窗口（关闭 = 藏进托盘）时会被它重新 show 出来，
			// 而此时内容已被清空，桌面上就留下一块空白窗口
			.then(() => invoke("main_mark_shown").catch(() => {}))
			.then(() => win.setFocus())
			.catch(() => {});
	};
	const fallbackTimer = setTimeout(show, 3000);
	settingsReady.finally(() => {
		requestAnimationFrame(() =>
			requestAnimationFrame(() => {
				clearTimeout(fallbackTimer);
				show();
			}),
		);
	});
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
	<React.StrictMode>
		{/* SWR 默认缓存是无上限的 Map，而 playlist-tracks / user-cloud / 探索分页这些
		    key 会装下整张歌单或整个云盘的数据；这里换成带上限的 LRU（见 utils/swr-cache.ts） */}
		<SWRConfig value={{ provider: () => createSwrLruCache() }}>
			<App />
		</SWRConfig>
	</React.StrictMode>,
);
