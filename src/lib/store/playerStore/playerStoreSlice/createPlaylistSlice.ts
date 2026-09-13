import { StateCreator } from "zustand";
import { PlaylistSlice, SharedPlayerState } from "@/lib/types/player";
import { Song } from "@/lib/types";
import { corePlayer } from "@/lib/player/corePlayer";
import { useProgressStore } from "../progressStore";

export const createPlaylistSlice: StateCreator<
	SharedPlayerState,
	[],
	[],
	PlaylistSlice
> = (set, get) => ({
	playlist: [],
	currentIndexInPlaylist: -1,

	addToPlaylist: (song) => {
		const { playlist } = get();
		if (playlist.some((s) => Number(s.id) === Number(song.id))) return;
		set({ playlist: [...playlist, song] });
	},

	clearPlaylist: () => {
		corePlayer.pause();
		useProgressStore.getState().resetProgress();
		set({
			currentSong: null,
			currentIndexInPlaylist: -1,
			currentSongMusicDetail: [],
			currentSongLyrics: null,
			localLyric: null,
			localLyricError: null,
			playlist: [],
			isPlaying: false,
			isLoadingMusic: false,
			duration: 0,
		});
	},

	removeFromPlaylist: async (song: Song) => {
		const { playlist, currentSong, next } = get();

		const newPlaylist = playlist.filter((s) => s.id !== song.id);

		if (currentSong?.id === song.id) {
			if (newPlaylist.length === 0) {
				corePlayer.pause();
				set({
					playlist: [],
					currentSong: null,
					currentIndexInPlaylist: -1,
					isPlaying: false,
				});
				return;
			}

			set({ playlist: newPlaylist });
			next();
		} else {
			const newIdx = newPlaylist.findIndex((s) => s.id === currentSong?.id);
			set({
				playlist: newPlaylist,
				currentIndexInPlaylist: newIdx,
			});
		}
	},

	isInPlaylist: (song: Song) => {
		const { playlist } = get();

		return playlist.some((s) => s.id === song.id);
	},

	fmPlaylist: [],
});
