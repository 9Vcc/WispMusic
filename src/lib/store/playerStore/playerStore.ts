import { create } from "zustand";
import { SharedPlayerState } from "../../types/player";
import { createJSONStorage, persist, subscribeWithSelector } from "zustand/middleware";
import { idbStorage, localStorageWithDirtyCheck } from "../idbStorage";
import { createLyricOffsetSlice } from "./playerStoreSlice/createLyricOffsetSlice";
import { createLyricVersionSlice } from "./playerStoreSlice/createLyricVersionSlice";
import { createPlaylistSlice } from "./playerStoreSlice/createPlaylistSlice";
import { createPlayerControlSlice } from "./playerStoreSlice/createPlayerControlSlice";
import { createSongInfoSlice } from "./playerStoreSlice/createSongInfoSlice";
import { createLocalLyricSlice } from "./playerStoreSlice/createLocalLyricSlice";

// 注意嵌套顺序：IndexedDB 层必须在内层，localStorage 层必须在外层。
// zustand persist 水合时使用的是传入该层的原始 set；若 localStorage 在内层，
// 其同步水合会经由外层 IDB 的包装 set 触发 setItem，在 IDB 读取完成前把
// 默认空 playlist 写入 IndexedDB，导致重启后播放列表被清空。
export const usePlayerStore = create<SharedPlayerState>()(
	persist(
		persist(
			subscribeWithSelector((...a) => ({
				...createPlaylistSlice(...a),
				...createPlayerControlSlice(...a),
				...createSongInfoSlice(...a),
				...createLocalLyricSlice(...a),
				...createLyricOffsetSlice(...a),
				...createLyricVersionSlice(...a),
			})),
			{
				name: "player-store-idb",
				storage: createJSONStorage(() => idbStorage),
				partialize: (state) => ({
					playlist: state.playlist,
				}),
			},
		),
		{
			name: "player-store-ls",
			storage: createJSONStorage(() => localStorageWithDirtyCheck),
			partialize: (state) => ({
				currentSong: state.currentSong,
				currentIndexInPlaylist: state.currentIndexInPlaylist,
				volume: state.volume,
				playbackRate: state.playbackRate,
				currentMusicLevelKey: state.currentMusicLevelKey,
				repeatMode: state.repeatMode,
				isShuffle: state.isShuffle,
				// 歌词对齐是逐首歌的校准结果：按歌曲 id 持久化（EchoMusic timeOffsetMap 模式）
				lyricTimeOffsetMap: state.lyricTimeOffsetMap,
				lyricVersionMap: state.lyricVersionMap,
			}),
		},
	),
);
