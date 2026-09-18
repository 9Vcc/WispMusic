import type { Song } from "./song";

/**
 * 匹配到的网易云歌曲在界面侧**真正会被读到**的字段投影。
 *
 * `neteaseMatch` 原先内嵌一整份 `Song`（单条 2~3KB）：其中 privilege、各档位
 * url/size（l/m/h/sq/hr）、`originSongSimpleData`（翻唱原曲又是一整份 Song）、
 * 多套别名数组等从不被消费——它们只用于展示（标题/歌手/专辑封面/时长）和判重。
 * 本地曲库上千首时这是数 MB 纯冗余，而且会被整份写进 local-music.json。
 * 消费点：`LocalTrackToSong` 取 name/al/ar、store 判重用 songId、列表标记只用真值。
 */
export type LocalTrackMatchedSong = Pick<
	Song,
	"id" | "name" | "ar" | "al" | "dt"
>;

export interface LocalTrackNeteaseMatch {
	songId: number;
	song: LocalTrackMatchedSong;
	matchedAt: number;
}

export interface LocalTrackDownloadInfo {
	level: string;
	fileType: string;
	fileSize: number;
	downloadedAt: number;
}

export interface LocalTrack {
	filePath: string;
	title: string;
	artist: string;
	album: string;
	durationSecs: number;
	/** 封面图片在封面缓存目录中的文件路径 */
	coverArtPath?: string | null;
	/** @deprecated 旧版内嵌 base64 封面，仅在读取旧缓存迁移时出现 */
	coverArtBase64?: string | null;
	source?: "scan" | "download";
	downloadInfo?: LocalTrackDownloadInfo;
	neteaseMatch?: LocalTrackNeteaseMatch;
}
