import { corePlayer } from "@/lib/player/corePlayer";
import { usePlayerStore } from "./playerStore";

/**
 * 加载态看门狗：`isLoadingMusic` 置 true 后迟迟不落幕时兜底。
 *
 * 转圈只有两条落幕路径——起播成功（onPlay）或本次载入报错（onError）。任何一条
 * 路上「没有结论」都会让转圈永久停在那里，而应用里没有任何机制再把它清掉。
 *
 * 判据不是「等了多久」而是「有没有真的在做事」：
 * · `corePlayer.isLoading()` 为真 → `player_load` 已发出、正在等原生回包。这是
 *   「慢但正常」——解灰音源要把整首下完才出声，几十秒很常见，这里重新计时好了。
 *   若误清：转圈先消失，用户再点一次又把整段下载从头来过，越点越慢。
 * · 为假却仍显示加载中 → 这次加载卡在了**没有任何超时保护**的等待上（典型就是
 *   音源解析挂在一条死掉的第三方链路上），它不会再有结论，清掉才是对的。
 *
 * 25s 的取法：明显长于地址解析的正常耗时（内置 API 1~3s、解灰通常 1~5s），
 * 又不至于让用户干等到怀疑人生。
 */
const LOADING_STUCK_TIMEOUT_MS = 25_000;

export function initLoadingWatchdog(): () => void {
	let timer: ReturnType<typeof setTimeout> | null = null;

	const cancel = () => {
		if (timer === null) return;
		clearTimeout(timer);
		timer = null;
	};

	const arm = () => {
		timer = setTimeout(() => {
			timer = null;
			if (!usePlayerStore.getState().isLoadingMusic) return;
			if (corePlayer.isLoading()) {
				console.debug("[PLAYER] 加载仍在原生侧进行中，看门狗重新计时");
				arm();
				return;
			}
			console.warn(
				`[PLAYER] 加载态滞留超过 ${LOADING_STUCK_TIMEOUT_MS / 1000}s 且无在途载入，按无结论清除`,
			);
			usePlayerStore.setState({ isLoadingMusic: false, isPlaying: false });
		}, LOADING_STUCK_TIMEOUT_MS);
	};

	const unsubscribe = usePlayerStore.subscribe(
		(state) => state.isLoadingMusic,
		(isLoading) => {
			// 每一次加载开始都重新计时：中途用户又切了歌，以最新那次为准
			cancel();
			if (isLoading) arm();
		},
	);

	return () => {
		cancel();
		unsubscribe();
	};
}
