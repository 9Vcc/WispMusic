import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { corePlayer } from "@/lib/player/corePlayer";
import { usePlayerStore } from "./playerStore";
import { useProgressStore } from "./progressStore";
import { shallow } from "zustand/shallow";

interface NativeAudioSnapshot {
	isReady: boolean;
	isPlaying: boolean;
	currentTime: number;
	duration: number;
}

// 静音前的最后音量（托盘“静音/取消静音”恢复用）
let lastAudibleVolume = 0.7;

/** 托盘菜单展示用的播放模式（repeatMode + isShuffle 组合派生） */
export type TrayPlayMode = "sequential" | "list" | "random" | "single";

function deriveTrayPlayMode(
	repeatMode: string,
	isShuffle: string,
): TrayPlayMode {
	if (isShuffle === "on") return "random";
	if (repeatMode === "repeat") return "list";
	if (repeatMode === "single") return "single";
	return "sequential";
}

function syncNativeSnapshot(snapshot: NativeAudioSnapshot) {
	const progress =
		snapshot.duration > 0
			? (snapshot.currentTime / snapshot.duration) * 100
			: 0;

	usePlayerStore.setState({
		isPlaying: snapshot.isPlaying,
		duration: snapshot.duration,
	});
	useProgressStore.getState().setProgress(snapshot.currentTime, progress);
}

async function playNativeOrCurrentSong() {
	const store = usePlayerStore.getState();
	if (!store.currentSong) return;

	const snapshot = await invoke<NativeAudioSnapshot>("player_get_state");
	if (!snapshot.isReady) {
		// 与 togglePlay 同一防抖语义：加载正在路上时静默等待，
		// 别把它 abort 掉从 URL 解析重新来一遍
		//
		// 同样以原生侧的真实加载态为准：只看 isLoadingMusic 时，只要它因某次
		// 载入「没有结论」而滞留，媒体键/SMTC 就和播放按钮一起失灵（点多少下
		// 都没反应），而 corePlayer 早已不在加载途中
		if (usePlayerStore.getState().isLoadingMusic && corePlayer.isLoading())
			return;
		store.playSong(store.currentSong, store.isFmMode);
		return;
	}

	syncNativeSnapshot(await invoke<NativeAudioSnapshot>("player_play"));
}

async function pauseNative() {
	syncNativeSnapshot(await invoke<NativeAudioSnapshot>("player_pause"));
}

export function initMediaSession() {
	const unsubscribes: Array<() => void> = [];

	// mini 窗口是否可见（mini-entered / mini-hidden 记账）。不可见时一律不推：
	// 从未开过 mini、或已关闭/退出 mini 时，进度订阅每 500ms 的推送都无人接收，
	// 白白付出 invoke + serde + emit_to 的开销（窗口销毁后还落一次报错日志）
	let miniActive = false;

	// 托盘菜单状态推送：经 Rust 定向转发到 tray-menu 窗口。
	// 不直接用 JS emit 广播——webview 间 JS 事件在部分环境下不可达，
	// 而 invoke 命令链路已被验证可靠（退出按钮等即走该链路）。
	function pushTrayState() {
		const s = usePlayerStore.getState();
		invoke("tray_push_state", {
			state: {
				title: s.currentSong?.name || "WispMusic",
				artist:
					s.currentSong?.ar?.map((a) => a.name).join("、") || "未播放",
				coverUrl: s.currentSong?.al?.picUrl
					? `${s.currentSong.al.picUrl}?param=128y128`
					: "",
				isPlaying: s.isPlaying,
				playMode: deriveTrayPlayMode(s.repeatMode, s.isShuffle),
				volume: s.volume,
			},
		}).catch(console.error);
	}

	// mini 播放器状态推送：同走 Rust 定向转发链路。
	// full = true 时附带播放列表（歌曲切换/列表变化/mini 窗口刚显示时），
	// 进度走增量推送。mini 窗口不可见时不推（见 miniActive）
	function pushMiniState(full: boolean) {
		if (!miniActive) return;
		const s = usePlayerStore.getState();
		const p = useProgressStore.getState();
		const state: Record<string, unknown> = {
			title: s.currentSong?.name || "WispMusic",
			artist:
				s.currentSong?.ar?.map((a) => a.name).join("、") || "未播放",
			coverUrl: s.currentSong?.al?.picUrl
				? `${s.currentSong.al.picUrl}?param=256y256`
				: "",
			isPlaying: s.isPlaying,
			playMode: deriveTrayPlayMode(s.repeatMode, s.isShuffle),
			volume: s.volume,
			currentTime: p.currentTime,
			duration: s.duration,
		};
		if (full) {
			// 播放列表压缩为展示所需的最小字段，降低 IPC 负担
			state.playlist = s.playlist.map((song) => ({
				id: song.id,
				name: song.name || "",
				artist: song.ar?.map((a) => a.name).join("、") || "",
				dt: song.dt,
			}));
			state.currentIndex = s.currentIndexInPlaylist;
		}
		invoke("mini_push_state", { state }).catch(console.error);
	}

	// 歌曲信息订阅
	const unsubMetadata = usePlayerStore.subscribe(
		(state) => ({ currentSong: state.currentSong, isPlaying: state.isPlaying }),
		({ currentSong }) => {
			if (!currentSong) return;

			// 设置元数据
			invoke("smtc_update_metadata", {
				title: currentSong.name || "",
				artist: currentSong.ar?.map((a) => a.name).join("、") || "",
				album: currentSong.al?.name || "",
				coverUrl: currentSong.al?.picUrl
					? `${currentSong.al.picUrl}?param=512y512`
					: "",
				durationSecs: 0,
			}).catch((e) => console.error("Update SMTC Info Failed:", e));

			pushTrayState();
			// 歌曲切换：歌词随完整快照推送到 mini 窗口
			pushMiniState(true);
		},
		{ equalityFn: shallow },
	);
	unsubscribes.push(unsubMetadata);

	// 播放状态订阅
	const unsubPlayback = usePlayerStore.subscribe(
		(state) => ({
			isPlaying: state.isPlaying,
			duration: state.duration,
		}),
		({ isPlaying, duration }) => {
			const time = useProgressStore.getState().currentTime;

			invoke("smtc_update_playback", {
				isPlaying,
				positionSecs: time,
				durationSecs: duration,
			}).catch(console.error);
		},
		{ equalityFn: shallow },
	);
	unsubscribes.push(unsubPlayback);

	// 播放模式订阅
	const unsubPlaymode = usePlayerStore.subscribe(
		(state) => ({
			isShuffle: state.isShuffle,
			repeatMode: state.repeatMode,
		}),
		() => {
			pushTrayState();
			pushMiniState(false);
		},
		{ equalityFn: shallow },
	);
	unsubscribes.push(unsubPlaymode);

	// 音量订阅：托盘菜单需要展示/调节音量
	const unsubVolume = usePlayerStore.subscribe(
		(state) => state.volume,
		() => {
			pushTrayState();
			pushMiniState(false);
		},
	);
	unsubscribes.push(unsubVolume);

	// 进度订阅：mini 播放器进度条增量同步（200ms 高频，节流到 500ms）
	let lastMiniProgressPush = 0;
	const unsubProgress = useProgressStore.subscribe(() => {
		const now = Date.now();
		if (now - lastMiniProgressPush < 500) return;
		lastMiniProgressPush = now;
		pushMiniState(false);
	});
	unsubscribes.push(unsubProgress);

	// mini 窗口显示：立即推送一次完整快照（含播放列表），避免显示旧状态
	const unlistenMiniEntered = listen("mini-entered", () => {
		miniActive = true;
		pushMiniState(true);
	});
	unsubscribes.push(() => {
		unlistenMiniEntered.then((f) => f());
	});

	// mini 窗口隐藏（关闭按钮 / Alt+F4 / 托盘显示主窗口）或退出 mini 模式销毁：
	// 之后的推送无人接收，直接停掉
	const unlistenMiniHidden = listen("mini-hidden", () => {
		miniActive = false;
	});
	unsubscribes.push(() => {
		unlistenMiniHidden.then((f) => f());
	});

	// 播放列表订阅：列表内容/当前曲目的变化同步到 mini 窗口
	const unsubPlaylist = usePlayerStore.subscribe(
		(state) => ({
			playlist: state.playlist,
			currentIndexInPlaylist: state.currentIndexInPlaylist,
		}),
		() => {
			pushMiniState(true);
		},
		{ equalityFn: shallow },
	);
	unsubscribes.push(unsubPlaylist);

	// 托盘菜单弹出：立即推送一次完整状态（托盘窗口隐藏期间收不到增量推送）
	const unlistenTrayShown = listen("tray-menu-shown", () => {
		pushTrayState();
	});
	unsubscribes.push(() => {
		unlistenTrayShown.then((f) => f());
	});

	// Tauri 事件监听
	const listenPromise = listen<{ event: string; position?: number }>(
		"smtc-event",
		(e) => {
			const { event, position } = e.payload;
			const store = usePlayerStore.getState();

			switch (event) {
				case "play":
					if (!store.isPlaying) {
						playNativeOrCurrentSong().catch(console.error);
					}
					break;
				case "pause":
					if (store.isPlaying) {
						pauseNative().catch(console.error);
					}
					break;
				case "toggle":
					if (store.isPlaying) {
						pauseNative().catch(console.error);
					} else {
						playNativeOrCurrentSong().catch(console.error);
					}
					break;
				case "next":
					store.next();
					break;
				case "previous":
					store.prev();
					break;
				case "set_position":
				if (position !== undefined && store.duration > 0) {
					// 必须走 corePlayer.seek：它会同步更新本地插值锚点
					//（anchorTime）再 invoke player_seek。若直接 invoke 原生命令，
					// corePlayer 的 RAF 进度循环仍按旧锚点插值，每 200ms 把
					// seek 前的旧位置写回 progressStore，进度条会跳回且与实际
					// 播放脱节（mini 播放器拖动后不再跟随播放的根因）
					corePlayer.seek(position / store.duration);
				}
				break;
				case "shuffle":
				store.toggleShuffleMode();
				break;
			case "repeat":
				store.toggleRepeatMode();
				break;
			// —— 托盘菜单扩展事件 ——
			case "volumeUp":
				store.updateVolume(Math.min(1, store.volume + 0.05));
				break;
			case "volumeDown":
				store.updateVolume(Math.max(0, store.volume - 0.05));
				break;
			case "toggleMute": {
				// 静音切换：0 视为静音态，恢复到 0.7 默认值
				if (store.volume > 0) {
					lastAudibleVolume = store.volume;
					store.updateVolume(0);
				} else {
					store.updateVolume(lastAudibleVolume || 0.7);
				}
				break;
			}
			case "setVolume": {
				// mini 播放器音量滑杆：直接设置目标值
				const volume = (e.payload as { volume?: number }).volume;
				if (typeof volume === "number") {
					store.updateVolume(Math.min(1, Math.max(0, volume)));
				}
				break;
			}
			case "playIndex": {
				// mini 播放列表：点播指定曲目（playSong 按 id 复用列表内索引）
				const index = (e.payload as { index?: number }).index;
				if (
					typeof index === "number" &&
					index >= 0 &&
					index < store.playlist.length
				) {
					store.playSong(store.playlist[index], store.isFmMode);
				}
				break;
			}
			case "setPlayMode": {
				const mode = (e.payload as { mode?: string }).mode;
				if (mode === "sequential") {
					usePlayerStore.setState({ isShuffle: "off", repeatMode: "order" });
				} else if (mode === "list") {
					usePlayerStore.setState({ isShuffle: "off", repeatMode: "repeat" });
				} else if (mode === "random") {
					usePlayerStore.setState({ isShuffle: "on", repeatMode: "order" });
				} else if (mode === "single") {
					usePlayerStore.setState({ isShuffle: "off", repeatMode: "single" });
				}
				break;
			}
			case "showWindow": {
				invoke("tray_show_main_window").catch(console.error);
				break;
			}
			}
		},
	);

	return () => {
		unsubscribes.forEach((unsub) => unsub());
		listenPromise.then((unlisten) => unlisten());
	};
}
