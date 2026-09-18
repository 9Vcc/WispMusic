import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { LocalTrack, LocalTrackMatchedSong } from "../types/localMusic";
import type { Album, Artist, Song } from "../types";
import type { DownloadedSong } from "../types/download";

export async function scanLocalMusic(dir: string): Promise<LocalTrack[]> {
	return invoke<LocalTrack[]>("scan_local_music", { dir });
}

/**
 * 把完整 Song 压成 `neteaseMatch` 需要的投影（见 LocalTrackMatchedSong 注释）。
 * 搜索/云盘等接口可能只给 `artists/album/duration` 别名，这里统一回退。
 */
export function toLocalMatchedSong(song: Song): LocalTrackMatchedSong {
	const raw = song as Song & {
		artists?: Artist[];
		album?: Album;
		duration?: number;
	};

	return {
		id: raw.id,
		name: raw.name,
		ar: raw.ar ?? raw.artists ?? [],
		al: raw.al ?? raw.album ?? { id: 0, name: "" },
		dt: raw.dt ?? raw.duration ?? 0,
	};
}

export function DownloadedSongToLocalTrack(
	downloadedSong: DownloadedSong,
): LocalTrack {
	const { song } = downloadedSong;

	return {
		filePath: downloadedSong.savePath,
		title: song.name,
		artist: song.ar?.map((artist) => artist.name).join(" / ") || "",
		album: song.al?.name || "",
		durationSecs: (song.dt || 0) / 1000,
		coverArtPath: null,
		source: "download",
		downloadInfo: {
			level: downloadedSong.level,
			fileType: downloadedSong.fileType,
			fileSize: downloadedSong.fileSize,
			downloadedAt: downloadedSong.downloadedAt,
		},
		neteaseMatch: {
			songId: song.id,
			song: toLocalMatchedSong(song),
			matchedAt: downloadedSong.downloadedAt,
		},
	};
}

export function LocalTrackToSong(track: LocalTrack): Song {
	const matchedSong = track.neteaseMatch?.song;

	return {
		id: -Math.abs(hashString(track.filePath)),
		name: matchedSong?.name || track.title,
		ar: matchedSong?.ar?.length
			? matchedSong.ar
			: [{ id: 0, name: track.artist }],
		al: {
			id: matchedSong?.al?.id || 0,
			name: matchedSong?.al?.name || track.album,
			picUrl:
				matchedSong?.al?.picUrl ||
				(track.coverArtPath ? convertFileSrc(track.coverArtPath) : undefined),
		},
		dt: track.durationSecs * 1000,
		localFilePath: track.filePath,
		localOriginalMetadata: {
			title: track.title,
			artist: track.artist,
			album: track.album,
			durationSecs: track.durationSecs,
		},
		localNeteaseMatch: track.neteaseMatch,
		localMatchedSongId: track.neteaseMatch?.songId,
	};
}

function hashString(str: string): number {
	let hash = 0;
	for (let i = 0; i < str.length; ++i) {
		const char = str.charCodeAt(i);
		hash = (hash << 5) - hash + char;
		hash |= 0;
	}
	return hash;
}

export async function getDefaultMusicDir(): Promise<string> {
	return invoke<string>("get_default_music_dir");
}
