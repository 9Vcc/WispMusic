import { toast } from "sonner";
import type { StateCreator } from "zustand";
import {
	findSiblingLyricFile,
	getBaseName,
	pickLyricFile,
	readLyricFile,
} from "@/lib/services/localLyric";
import type {
	LocalLyricSlice,
	LocalLyricSource,
	LocalLyricState,
	SharedPlayerState,
} from "@/lib/types/player";
import {
	LocalLyricParseError,
	ParseLocalLyric,
} from "@/lib/utils/lyric-parser";
import { createLruCache } from "@/lib/utils/lru";
import { useSettingStore } from "../../settingStore/settingStore";

/**
 * 并发熔断：切歌 / 连续点击时会并发多次加载，
 * 只有最后一次发起的请求允许写入状态（与其「校验歌曲 id」配合使用）。
 */
let localLyricRequestId = 0;

export const createLocalLyricSlice: StateCreator<
	SharedPlayerState,
	[],
	[],
	LocalLyricSlice
> = (set, get) => {
	/**
	 * 本次运行内记住用户手动指定过的歌词，切歌往返时不必重选。
	 * 只存内存：歌词文件位置属于用户当下的临时意图，不值得落盘。
	 *
	 * 每条记录持有整份解析后的歌词行（可达数百 KB），无上限时会随手动指定的
	 * 歌曲数单调增长；用 LRU 只保留最近若干首（用户实际来回重听的总是那几首）。
	 * 上限 4：够覆盖「上一首/下一首」往返，常驻量压在 ~2MB 内。
	 */
	const manualLyricCache = createLruCache<number, LocalLyricState>(4);

	const resetLocalLyric = () => {
		localLyricRequestId += 1;
		set({
			localLyric: null,
			localLyricError: null,
			isLoadingLocalLyric: false,
		});
	};

	/** 读取 + 解析 + 校验歌曲归属，成功才写入状态 */
	const applyLyricFile = async (
		filePath: string,
		source: LocalLyricSource,
		songId: number,
		notifySuccess: boolean,
	): Promise<boolean> => {
		const requestId = ++localLyricRequestId;
		const fileName = getBaseName(filePath);

		set({ isLoadingLocalLyric: true, localLyricError: null });

		try {
			const text = await readLyricFile(filePath);
			if (requestId !== localLyricRequestId) return false;

			// 解析即校验：不可解析 / 不支持的格式在这里抛出可读原因
			const parsed = ParseLocalLyric(text);

			// 读盘期间歌曲可能已经切走，此时结果已无意义
			if (get().currentSong?.id !== songId) return false;

			const next: LocalLyricState = {
				filePath,
				fileName,
				lines: parsed.lines,
				isVerbatim: parsed.isVerbatim,
				source,
				songId,
			};

			if (source === "manual") manualLyricCache.set(songId, next);

			set({
				localLyric: next,
				isLoadingLocalLyric: false,
				localLyricError: null,
			});

			if (notifySuccess) toast.success(`已加载本地歌词：${fileName}`);
			parsed.warnings.forEach((warning) => {
				toast.info(warning, { description: fileName });
			});

			return true;
		} catch (error) {
			if (requestId !== localLyricRequestId) return false;

			const reason =
				error instanceof LocalLyricParseError
					? error.message
					: `读取歌词文件失败：${
							error instanceof Error ? error.message : String(error)
						}`;
			const message = `${fileName}：${reason}`;

			set({ isLoadingLocalLyric: false, localLyricError: message });
			// 自动匹配失败同样要提示：文件是用户自己放进目录的，
			// 静默跳过会让人以为是播放器没做这件事
			toast.error("本地歌词无法加载", { description: message });

			return false;
		}
	};

	return {
		localLyric: null,
		isLoadingLocalLyric: false,
		localLyricError: null,

		clearLocalLyric: () => {
			const current = get().localLyric;
			if (current) manualLyricCache.delete(current.songId);
			resetLocalLyric();
		},

		pickLocalLyric: async () => {
			// 选择期间放弃正在进行的自动加载，避免两边互相覆盖
			localLyricRequestId += 1;
			set({ isLoadingLocalLyric: false });

			const song = get().currentSong;
			if (!song) {
				toast.error("请先播放一首歌曲，再选择歌词文件");
				return false;
			}

			const filePath = await pickLyricFile();
			// 用户取消：保持当前歌词不变
			if (!filePath) return false;

			return applyLyricFile(filePath, "manual", song.id, true);
		},

		loadSiblingLyric: async (song, options) => {
			const notifySuccess = options?.silent === false;
			// force：用户显式要求「重新匹配同目录歌词」，
			// 此时绕开自动匹配开关与手动歌词记忆，按目录里的文件重新来一遍
			const force = options?.force === true;
			const songId = song?.id;

			// 在线歌曲没有本地目录可查：清掉上一首残留的本地歌词
			if (songId == null || !song?.localFilePath) {
				resetLocalLyric();
				return false;
			}

			if (force) {
				manualLyricCache.delete(songId);
			} else {
				// 用户手动指定过的歌词优先，切歌往返不必重选
				const remembered = manualLyricCache.get(songId);
				if (remembered) {
					set({
						localLyric: remembered,
						localLyricError: null,
						isLoadingLocalLyric: false,
					});
					return true;
				}

				if (!useSettingStore.getState().lyric.autoLoadLocalLyric) {
					resetLocalLyric();
					return false;
				}

				if (get().localLyric?.songId === songId) return true;
			}

			resetLocalLyric();

			let siblingPath: string | null = null;
			try {
				siblingPath = await findSiblingLyricFile(song.localFilePath);
			} catch (error) {
				console.error("[LocalLyric] 查找同目录歌词失败:", error);
				return false;
			}

			// 目录里没有同名歌词是常态，不算错误，静默回退到在线歌词
			if (!siblingPath) return false;
			if (get().currentSong?.id !== songId) return false;

			return applyLyricFile(siblingPath, "auto", songId, notifySuccess);
		},
	};
};

/**
 * 仅返回「确实属于当前播放歌曲」的本地歌词。
 * 返回值取自 state 中的同一个对象引用（或 null），可直接作为 zustand 选择器使用。
 */
export const selectActiveLocalLyric = (state: SharedPlayerState) =>
	state.localLyric && state.currentSong?.id === state.localLyric.songId
		? state.localLyric
		: null;
