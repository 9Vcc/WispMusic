import { useEffect } from "react";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";

/**
 * 切歌时自动尝试加载音频同目录下的同名歌词文件。
 *
 * 挂在 RootLayout（只有主窗口会渲染）：
 * - 正常切歌（手动 / 自动下一首 / 交叉淡入）都会改变 currentSong，订阅即可覆盖；
 * - 重启后恢复上次播放的歌曲也能补一次加载（等设置水合完成后再订阅）；
 * - 重新打开「自动匹配同名歌词」开关时会按新设置再评估一次。
 *
 * 即使开关关闭也保持订阅：切歌时同样要清掉上一首残留的本地歌词与错误提示。
 * 加载失败由 store 统一 toast（文件是用户自己放进目录的，静默失败会让人
 * 以为播放器没做这件事）。
 */
export function useLocalLyricAutoLoad() {
	const hydrated = useSettingStore((s) => s.hydrated);
	const autoLoadLocalLyric = useSettingStore((s) => s.lyric.autoLoadLocalLyric);

	useEffect(() => {
		if (!hydrated) return;

		const loadForCurrentSong = () => {
			const { currentSong, loadSiblingLyric } = usePlayerStore.getState();
			void loadSiblingLyric(currentSong, { silent: true });
		};

		const unsubscribe = usePlayerStore.subscribe(
			(state) => state.currentSong,
			() => loadForCurrentSong(),
		);

		// 开关刚打开时补一次匹配；关闭时保持现状，等下次切歌再清理
		if (autoLoadLocalLyric) loadForCurrentSong();

		return unsubscribe;
	}, [hydrated, autoLoadLocalLyric]);
}
