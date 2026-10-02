import { corePlayer } from "@/lib/player/corePlayer";
import { usePlayerStore } from "./playerStore";

/**
 * 加载态看门狗：`isLoadingMusic` 置 true 后迟迟不落幕时兜底。
 *
 * 转圈只有两条落幕路径——起播成功（onPlay）或本次载入报错（onError）。任何一条
 * 路上「没有结论」都会让转圈永久停在那里，而应用里没有任何机制再把它清掉：
 * 地址解析（getSongUrl / 解灰）这一段没有超时，请求僵住时 await 永不返回；
 * 原生侧回包超时是 20s，超时之后走到的是另一条命令而不是本次载入的结论。
 * 与其给每条路径单独补超时，不如在唯一的可见状态上设一道总闸。
 *
 * 25s 的取法：必须盖过原生的 20s 回包上限，否则看门狗会抢在正常失败之前把
 * 状态清掉，前端随后又被真实结论覆写，转圈闪一下继续转——那还不如没有。
 */
const LOADING_STUCK_TIMEOUT_MS = 25_000;

export function initLoadingWatchdog(): () => void {
	let timer: ReturnType<typeof setTimeout> | null = null;

	const cancel = () => {
		if (timer === null) return;
		clearTimeout(timer);
		timer = null;
	};

	const unsubscribe = usePlayerStore.subscribe(
		(state) => state.isLoadingMusic,
		(isLoading) => {
			// 每一次加载开始都重新计时：中途用户又切了歌，以最新那次为准
			cancel();
			if (!isLoading) return;

			timer = setTimeout(() => {
				timer = null;
				if (!usePlayerStore.getState().isLoadingMusic) return;
				console.warn(
					`[PLAYER] 加载态超过 ${LOADING_STUCK_TIMEOUT_MS / 1000}s 未落幕，按无结论清除`,
				);
				// 同时丢弃 corePlayer 的在途标记：否则 togglePlay 会认为「加载正在
				// 路上」而一直早退，用户点了播放按钮也没任何反应
				corePlayer.abandonPendingLoad();
				usePlayerStore.setState({ isLoadingMusic: false, isPlaying: false });
			}, LOADING_STUCK_TIMEOUT_MS);
		},
	);

	return () => {
		cancel();
		unsubscribe();
	};
}
