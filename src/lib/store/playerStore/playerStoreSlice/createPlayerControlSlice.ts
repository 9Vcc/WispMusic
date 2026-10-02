import { invoke } from "@tauri-apps/api/core";
import type { StateCreator } from "zustand";
import { REPEAT_MODE_BY_TYPE } from "@/lib/constants/player";
import type { QualityKey } from "@/lib/constants/song";
import { corePlayer } from "@/lib/player/corePlayer";
import { prefetchSong } from "@/lib/player/prefetch";
import {
	getKnownMaxQualityKey,
	invalidateUnblockCache,
	resolveSongUrlWithQuality,
	resolveUnblockedSource,
} from "@/lib/player/song-source";
import { getAlbum } from "@/lib/services/album";
import { getArtistAllSongs } from "@/lib/services/artist";
import { getPlaylistAllTrack } from "@/lib/services/playlist";
import { fmTrash, getPersonalFm } from "@/lib/services/recommend";
import { getSongLyric, getSongMusicDetail } from "@/lib/services/song";
import type { Song } from "@/lib/types";
import type { PlayerControlSlice, SharedPlayerState } from "@/lib/types/player";
import { useSettingStore } from "../../settingStore/settingStore";
import { useProgressStore } from "../progressStore";

let currentPlayAbortController: AbortController | null = null;
/** 当前曲剩余多少秒时开始预取下一首。悬停预取（prefetch.ts）覆盖"用户主动点"的
 *  那条路径，这里是"自然放完接下一首"的兜底提前量。 */
const NEXT_SONG_PRELOAD_LEAD_SECONDS = 10;
// 连续播放失败的上限：超过后停止自动跳歌，避免全列表不可播时无限循环
const MAX_CONSECUTIVE_PLAY_ERRORS = 5;
let consecutivePlayErrors = 0;

function resetPlayErrorCount() {
	consecutivePlayErrors = 0;
}

/** 播放失败时调用；返回 true 表示可以继续自动跳下一首 */
function shouldSkipToNextOnError() {
	consecutivePlayErrors += 1;
	return consecutivePlayErrors < MAX_CONSECUTIVE_PLAY_ERRORS;
}

function getSongDurationSeconds(song: Song) {
	const localDuration = song.localOriginalMetadata?.durationSecs ?? 0;
	if (localDuration > 0) return localDuration;

	if (song.dt > 0) return song.dt / 1000;

	const duration = song.duration ?? 0;
	if (duration <= 0) return 0;

	return duration > 10000 ? duration / 1000 : duration;
}

function getPredictableNextSong(state: SharedPlayerState) {
	if (state.isFmMode)
		return state.fmRepeatMode ? undefined : state.fmPlaylist[1];
	if (state.repeatMode === "single" || state.isShuffle === "on")
		return undefined;

	const nextIndex = state.currentIndexInPlaylist + 1;
	if (nextIndex < state.playlist.length) return state.playlist[nextIndex];
	if (state.repeatMode === "repeat" && state.playlist.length > 0) {
		return state.playlist[0];
	}
	return undefined;
}

function preloadNextSong(get: () => SharedPlayerState) {
	const nextSong = getPredictableNextSong(get());
	if (!nextSong) return false;

	// urgent：这是"自然放完接下一首"的关键提前量，不该被悬停预取的节流挡住；
	// 若这首歌已经因悬停预热过，prefetch.ts 内部的去重会直接跳过，不会重复下载
	prefetchSong(nextSong, true);
	return true;
}

function canStartNaturalCrossfade(state: SharedPlayerState) {
	if (state.isFmMode) return !state.fmRepeatMode && state.fmPlaylist.length > 1;
	if (state.repeatMode === "single") return false;
	if (state.isShuffle === "on") return state.playlist.length > 1;
	if (state.currentIndexInPlaylist + 1 < state.playlist.length) return true;
	return state.repeatMode === "repeat" && state.playlist.length > 0;
}

function shouldStartNaturalCrossfade(
	state: SharedPlayerState,
	currentTime: number,
	duration: number,
) {
	const crossfadeDuration = corePlayer.getCrossfadeDuration();
	if (crossfadeDuration <= 0 || duration <= 0) return false;
	if (!canStartNaturalCrossfade(state)) return false;

	const remaining = duration - currentTime;
	return remaining > 0 && remaining <= Math.min(crossfadeDuration, duration);
}

function shouldPreloadNextSong(
	state: SharedPlayerState,
	currentTime: number,
	duration: number,
) {
	if (duration <= 0 || !getPredictableNextSong(state)) return false;

	const remaining = duration - currentTime;
	return (
		remaining > 0 &&
		remaining <= Math.min(NEXT_SONG_PRELOAD_LEAD_SECONDS, duration)
	);
}

export const createPlayerControlSlice: StateCreator<
	SharedPlayerState,
	[],
	[],
	PlayerControlSlice
> = (set, get) => ({
	isPlaying: false,
	isLoadingMusic: false,
	repeatMode: "order",
	isShuffle: "off",
	volume: 0.7,
	playbackRate: 1,
	duration: 0,

	playSong: async (song, isFm = false) => {
		if (!isFm) {
			set({ isFmMode: false });
		}

		if (currentPlayAbortController) currentPlayAbortController.abort();
		currentPlayAbortController = new AbortController();

		const { signal } = currentPlayAbortController;
		let naturalCrossfadeStarted = false;
		let nextSongPreloadStarted = false;
		const updatePlaybackProgress = (currentTime: number) => {
			if (signal.aborted) return;

			const state = get();
			const { duration } = state;
			const progress = duration > 0 ? (currentTime / duration) * 100 : 0;
			useProgressStore.getState().setProgress(currentTime, progress);

			if (
				!nextSongPreloadStarted &&
				shouldPreloadNextSong(state, currentTime, duration)
			) {
				nextSongPreloadStarted = preloadNextSong(get);
			}

			if (
				!naturalCrossfadeStarted &&
				shouldStartNaturalCrossfade(state, currentTime, duration)
			) {
				naturalCrossfadeStarted = true;
				get().next();
			}
		};

		try {
			const { playlist } = get();
			const preferMusicLevel = useSettingStore.getState().audio.preferQuality;
			const fallbackDuration = getSongDurationSeconds(song);

			const existingIndex = playlist.findIndex((s) => s.id === song.id);
			let targetIndex: number;

			if (existingIndex !== -1) targetIndex = existingIndex;
			else {
				targetIndex = playlist.length;
				set({ playlist: [...playlist, song] });
			}

			set({ currentSong: song, currentIndexInPlaylist: targetIndex });

			set({
				isLoadingMusic: true,
				isPlaying: false,
			});
			useProgressStore.getState().resetProgress();

			// 本地歌曲
			if (song.localFilePath) {
				const url = song.localFilePath;
				const matchedSongId = song.localMatchedSongId;

				corePlayer.play(
					url,
					() => get().next(),
					(duration) => {
						resetPlayErrorCount();
						set({ isPlaying: true, isLoadingMusic: false, duration });
					},
					updatePlaybackProgress,
					() => {
						console.error("本地歌曲播放错误");
						set({ isPlaying: false, isLoadingMusic: false });
						if (shouldSkipToNextOnError()) get().next();
					},
					fallbackDuration,
					// 音量与倍速随载入下发，起播首帧就是正确值
					{ volume: get().volume, speed: get().playbackRate },
				);

				set({
					currentSongMusicDetail: undefined,
					currentSongLyrics: null,
					currentMusicLevelKey: "local",
				});

				if (matchedSongId) {
					getSongLyric(matchedSongId, signal)
						.then((musicLyric) => {
							if (!signal.aborted) set({ currentSongLyrics: musicLyric });
						})
						.catch((err) => {
							if (!signal.aborted) {
								console.error("获取本地匹配歌词失败:", err);
							}
						});
				}

				return;
			}

			// 先检查歌曲是否可用
			let url: string | undefined;
			let levelKey: QualityKey | undefined;
			const canPlay = song.privilege
				? song.privilege.st >= 0 && song.privilege.pl > 0
				: true; // 默认尝试当做可用处理

			if (canPlay) {
				const resolved = await resolveSongUrlWithQuality(
					song.id,
					preferMusicLevel,
					getKnownMaxQualityKey(song),
					signal,
				);
				url = resolved?.url;
				levelKey = resolved?.levelKey;

				if (url && resolved) {
					set({ currentMusicLevelKey: resolved.levelKey });
					// 这里不再查 check_audio_cache 替换本地路径：原生 player_load 的
					// resolve_audio_source 用同样的 songId+level 键查同一个缓存文件，
					// 命中即直接 File 播放。多出的这跳 IPC 串行在出声路径上，
					// 只会让每次点歌白等一次往返。
				}
			}

			if (!url) {
				// 尝试解锁灰色歌曲，或弥补普通请求失败的歌曲。
				// 解灰结果带缓存：同一首灰歌的二次播放不必再付一次 /match 往返
				try {
					const unblocked = await resolveUnblockedSource(song.id, signal);
					if (unblocked) {
						url = unblocked.url;
						// 解灰音源用固定档位名，让它的缓存也落在稳定 key 上
						levelKey = unblocked.levelKey;
						set({ currentMusicLevelKey: unblocked.levelKey });
					}
				} catch (e) {
					console.error("尝试解锁歌曲出错", e);
				}
			}

			if (!url) {
				// 全部解析途径都拿不到地址且未抛异常：必须复位加载态，
				// 否则 isLoadingMusic 停在 true、播放按钮永远转圈
				set({ isLoadingMusic: false, isPlaying: false });
				return;
			}

			{
				corePlayer.play(
					url,
					() => {
						get().next();
					},
					(duration) => {
						resetPlayErrorCount();
						// 加载态在真正起播时才清除：提前清会出现「按钮已是
						// 播放态、实际还在等网络」的假状态；就绪前硬切保证这
						// 段等待里旧歌继续响，用户不会被卡在静音里
						set({ isPlaying: true, isLoadingMusic: false, duration });
					},
					updatePlaybackProgress,
					async () => {
						console.error("远程歌曲播放错误，尝试恢复");
						// 解灰地址失效时立刻剔除缓存，避免下次又拿着同一个坏地址起播
						if (levelKey === "unlock") invalidateUnblockCache(song.id);
						const currentTime = useProgressStore.getState().currentTime;
						const { currentSong, currentMusicLevelKey } = get();

						if (currentSong && currentMusicLevelKey) {
							// 如果是 URL 过期，且这时候后台已经缓存好了，我们可以无缝切换到缓存
							try {
								const cachedPath = await invoke<string | null>(
									"check_audio_cache",
									{
										songId: currentSong.id,
										level: currentMusicLevelKey,
										url: url || "", // 这时候真实的 url 可能已经不可用，但可以用来提取后缀
									},
								);

								if (cachedPath) {
									console.log(
										"[CACHE] 发现已缓存文件，从缓存恢复播放:",
										cachedPath,
									);
									corePlayer.play(
										cachedPath,
										() => get().next(),
										(duration) => {
											if (duration > 0) {
												corePlayer.seek(currentTime / duration);
											}
											resetPlayErrorCount();
											set({
												isPlaying: true,
												isLoadingMusic: false,
												duration,
											});
										},
										updatePlaybackProgress,
										() => {
											console.error("缓存歌曲恢复播放仍然失败，跳过");
											set({
												isPlaying: false,
												isLoadingMusic: false,
											});
											if (shouldSkipToNextOnError()) get().next();
										},
										fallbackDuration,
										{
											volume: get().volume,
											speed: get().playbackRate,
										},
									);
									return; // 恢复成功，不再执行下面逻辑
								}
							} catch (e) {
								console.error("尝试从缓存恢复播放失败", e);
							}
						}

						set({ isPlaying: false, isLoadingMusic: false });
						if (shouldSkipToNextOnError()) get().next();
					},
					fallbackDuration,
					{
						// 缓存键（歌曲 + 音质）交付给原生侧，让它把整首文件写到
						// 与 check_audio_cache 一致的位置；音量与倍速一起下发
						songId: song.id,
						level: levelKey,
						volume: get().volume,
						speed: get().playbackRate,
					},
				);

				set({ isLoadingMusic: false });

				// 详			);

			// isLoadingMusic 此处不再清除：它要到 onPlay（loaded 事件）才落幕，
			// 期间旧歌继续响（就绪后硬切），转圈是对「等新歌就绪」的诚实反馈。

			// 详情与歌词在音频启动后并行获取，不再阻塞出声
			getSongMusicDetail(song.id, signal)
					.then((musicDetail) => {
						if (!signal.aborted && musicDetail) {
							set({ currentSongMusicDetail: musicDetail });
						}
					})
					.catch((err) => {
						if (!signal.aborted) console.error("获取歌曲详情失败:", err);
					});

				getSongLyric(song.id, signal)
					.then((musicLyric) => {
						if (!signal.aborted) set({ currentSongLyrics: musicLyric });
					})
					.catch((err) => {
						if (!signal.aborted) console.error("获取歌词失败:", err);
					});
			}
		} catch (err: unknown) {
			if (err instanceof Error && err.name === "AbortError") {
				console.log("播放被中断");
				return;
			}
			console.error("播放歌曲失败:", err);
			set({ isLoadingMusic: false });
		}
	},

	playQueue: async (songs: Song[]) => {
		set({
			isLoadingMusic: true,
			playlist: songs,
			currentIndexInPlaylist: 0,
			isFmMode: false,
		});
		await get().playSong(songs[0]);
	},

	playList: async (listId, listType) => {
		set({ isLoadingMusic: true, isFmMode: false });

		try {
			let songs: Song[] | undefined;

			switch (listType) {
				case "list": {
					const res = await getPlaylistAllTrack(listId);
					songs = res;
					break;
				}
				case "album": {
					const res = await getAlbum(listId);
					songs = res?.songs;
					break;
				}
			}

			if (!songs || !songs.length) {
				set({ isLoadingMusic: false });
				return;
			}

			set({ playlist: songs });

			await get().playSong(songs[0]);
		} catch (error) {
			console.error("播放歌单失败", error);
			set({ isPlaying: false, isLoadingMusic: false });
		}
	},

	playArtist: async (artistId) => {
		set({ isLoadingMusic: true });
		try {
			const music = await getArtistAllSongs({ id: artistId });
			const songs = music.songDetails;

			if (!songs || !songs.length) {
				set({ isLoadingMusic: false });
				return;
			}

			set({ playlist: songs });
			await get().playSong(songs[0]);
		} catch (err) {
			console.error("从歌手详情播放失败", err);
			set({ isPlaying: false, isLoadingMusic: false });
		}
	},

	togglePlay: () => {
		const { isPlaying, currentSong, playSong, isFmMode } = get();
		if (!currentSong) return;

		if (!corePlayer.isReady()) {
			// 一次加载正在路上（URL 解析或原生拉流中）：静默等待即可。
			// 这里若照旧走 playSong，会先把在途请求 abort 掉再从 URL 解析
			// 重新来一遍——旧加载的 Rust 侧工作全部作废，转圈时间翻倍，
			// 这正是「暂停后再次播放要转圈等待」的一类来源。
			if (get().isLoadingMusic) return;
			playSong(currentSong, isFmMode);
			return;
		}

		if (isPlaying) corePlayer.pause();
		else corePlayer.resume();

		set({ isPlaying: !isPlaying });
	},

	next: (isManual = false) => {
		if (get().isFmMode) {
			get().nextFmSong();
			return;
		}

		const {
			togglePlay,
			currentIndexInPlaylist,
			playlist,
			currentSong,
			repeatMode,
			isShuffle,
		} = get();
		if (!currentSong || playlist.length === 0) return;

		console.log(
			`[PLAYER] 当前播放歌曲在列表中的顺序: ${currentIndexInPlaylist}`,
		);

		// 单曲循环
		if (repeatMode === "single" && !isManual) {
			corePlayer.seek(0);
			corePlayer.resume();
			set({ isPlaying: true });
			return;
		}

		let nextIdx: number;
		// 随机播放
		if (isShuffle === "on") {
			nextIdx = Math.floor(Math.random() * playlist.length);
			if (playlist.length > 1 && nextIdx === currentIndexInPlaylist) {
				nextIdx = (nextIdx + 1) % playlist.length;
			}
		}
		// 顺序或循环
		else {
			console.log(
				`[PLAYER] 当前播放歌曲在列表中的顺序: ${currentIndexInPlaylist}`,
			);
			nextIdx = currentIndexInPlaylist + 1;
			console.log(
				`[PLAYER] 当前播放歌曲在列表中的顺序: ${currentIndexInPlaylist}`,
			);
			// 顺序播放时，最后一首播完就暂停
			if (nextIdx >= playlist.length) {
				nextIdx = 0;
				if (repeatMode === "order") {
					set({ currentIndexInPlaylist: 0 });
					togglePlay();
					return;
				}
			}
		}

		set({ currentIndexInPlaylist: nextIdx });
		get().playSong(playlist[nextIdx]);
	},

	prev: (isManual = false) => {
		if (get().isFmMode) {
			get().seek(0);
			return;
		}

		const {
			currentIndexInPlaylist,
			playlist,
			currentSong,
			repeatMode,
			isShuffle,
		} = get();
		if (!currentSong || playlist.length === 0) return;

		// 单曲循环
		if (repeatMode === "single" && !isManual) {
			corePlayer.seek(0);
			corePlayer.resume();
			set({ isPlaying: true });
			return;
		}

		let prevIdx: number;
		// 随机播放
		if (isShuffle === "on") {
			prevIdx = Math.floor(Math.random() * playlist.length);
			if (playlist.length > 1 && prevIdx === currentIndexInPlaylist) {
				prevIdx = (prevIdx - 1 + playlist.length) % playlist.length;
			}
		}
		// 顺序或循环
		else {
			prevIdx = currentIndexInPlaylist - 1;
			if (prevIdx < 0) {
				prevIdx = playlist.length - 1;
				if (repeatMode === "order") {
					prevIdx = 0;
				}
			}
		}

		set({ currentIndexInPlaylist: prevIdx });
		get().playSong(playlist[prevIdx]);
	},

	updateProgress: () => {
		const currentTime = corePlayer.getPosition();
		const { duration } = get();
		const progress = duration > 0 ? (currentTime / duration) * 100 : 0;
		useProgressStore.getState().setProgress(currentTime, progress);
	},

	updateVolume: (volume: number) => {
		corePlayer.setVolume(volume);
		set({ volume });
	},

	setPlaybackRate: (rate: number) => {
		corePlayer.setSpeed(rate);
		set({ playbackRate: rate });
	},

	seek: (percentage: number) => {
		if (get().duration <= 0) return;
		corePlayer.seek(percentage / 100);
		get().updateProgress();
	},

	toggleRepeatMode: () => {
		const { repeatMode, isShuffle } = get();

		const repeatModeConfig = REPEAT_MODE_BY_TYPE[repeatMode];
		const nextRepetMode = repeatModeConfig.next;
		if (!REPEAT_MODE_BY_TYPE[nextRepetMode].canShuffle && isShuffle === "on") {
			set({ isShuffle: "off" });
		}
		set({ repeatMode: nextRepetMode });
	},

	toggleShuffleMode: () => {
		const { isShuffle } = get();
		set({ isShuffle: isShuffle === "on" ? "off" : "on" });
	},

	// 顺序播放 → 列表循环 → 随机播放 → 单曲循环 → 顺序播放
	cyclePlayMode: () => {
		const { repeatMode, isShuffle } = get();
		if (isShuffle === "on") {
			set({ isShuffle: "off", repeatMode: "single" });
		} else if (repeatMode === "order") {
			set({ repeatMode: "repeat" });
		} else if (repeatMode === "repeat") {
			set({ isShuffle: "on" });
		} else {
			set({ repeatMode: "order" });
		}
	},

	isFmMode: false,
	fmRepeatMode: false,

	toggleFmRepeatMode: async () => {
		const { fmRepeatMode } = get();
		set({ fmRepeatMode: !fmRepeatMode });
	},

	fetchFmSongs: async () => {
		const res = await getPersonalFm();
		if (res && res.length > 0) {
			set({ fmPlaylist: [...get().fmPlaylist, ...res] });
		}
	},

	playFm: async () => {
		const { fmPlaylist, fetchFmSongs, playSong } = get();

		if (fmPlaylist.length === 0) await fetchFmSongs();

		const currentFmSongs = get().fmPlaylist;
		if (currentFmSongs.length > 0) {
			set({ isFmMode: true });
			playSong(currentFmSongs[0], true);
		}
	},

	nextFmSong: () => {
		set({ isFmMode: true });
		const { fmPlaylist, fmRepeatMode, playSong, fetchFmSongs } = get();

		if (fmRepeatMode) {
			corePlayer.seek(0);
			corePlayer.resume();
			set({ isPlaying: true });
			return;
		}

		const newFmPlaylist = fmPlaylist.slice(1);
		set({ fmPlaylist: newFmPlaylist });

		if (newFmPlaylist.length > 0) {
			playSong(newFmPlaylist[0], true);
		}

		if (newFmPlaylist.length <= 1) {
			fetchFmSongs();
		}
	},

	trashFmSong: async () => {
		const { currentSong, nextFmSong } = get();
		if (!currentSong) return;

		try {
			await fmTrash(currentSong.id);
			nextFmSong();
		} catch (error) {
			console.error("垃圾桶操作失败", error);
		}
	},
});
