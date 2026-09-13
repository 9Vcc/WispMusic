import { api } from "../api";
import type { Album, Artist, Playlist, Song } from "../types";

// 歌手分类选项（与网易云 API 的 area/type 编码一致）
export const ARTIST_AREAS = [
	{ name: "全部", value: -1 },
	{ name: "华语", value: 7 },
	{ name: "欧美", value: 96 },
	{ name: "日本", value: 8 },
	{ name: "韩国", value: 16 },
	{ name: "其他", value: 0 },
] as const;

export const ARTIST_TYPES = [
	{ name: "男歌手", value: 1 },
	{ name: "女歌手", value: 2 },
	{ name: "乐队", value: 3 },
] as const;

// 新歌速递地区编码
export const NEW_SONG_AREAS = [
	{ name: "全部", value: 0 },
	{ name: "华语", value: 7 },
	{ name: "欧美", value: 96 },
	{ name: "日本", value: 8 },
	{ name: "韩国", value: 16 },
] as const;

// 新碟地区编码
export const NEW_ALBUM_AREAS = [
	{ name: "全部", value: "ALL" },
	{ name: "华语", value: "ZH" },
	{ name: "欧美", value: "EA" },
	{ name: "日本", value: "JP" },
	{ name: "韩国", value: "KR" },
] as const;

// /toplist/detail 返回的 tracks 通常是摘要格式（first=歌名 second=歌手 third=专辑），
// 不是完整的 Song 对象；保留完整字段以防接口变化
export interface ToplistSummaryTrack {
	first?: string;
	second?: string;
	third?: string;
	name?: string;
	ar?: { name?: string }[];
}

export interface ToplistDetailItem extends Omit<Playlist, "tracks"> {
	tracks?: ToplistSummaryTrack[];
	updateFrequency: string;
}

interface ToplistDetailResponse {
	code: number;
	list: ToplistDetailItem[];
}

// 所有榜单内容摘要（含每个榜单的前几首歌）
export async function getToplistDetail() {
	const res = await api.get<ToplistDetailResponse>("/toplist/detail");
	return res.list;
}

interface TopPlaylistResponse {
	code: number;
	playlists: Playlist[];
	total: number;
	more: boolean;
}

// 分类歌单
export async function getTopPlaylist(
	cat: string,
	limit: number = 50,
	offset: number = 0,
) {
	const res = await api.get<TopPlaylistResponse>("/top/playlist", {
		cat,
		limit: limit.toString(),
		offset: offset.toString(),
	});
	return { playlists: res.playlists, hasMore: res.more, total: res.total };
}

// 精品歌单
export async function getHighQualityPlaylist(limit: number = 50) {
	const res = await api.get<TopPlaylistResponse>(
		"/top/playlist/highquality",
		{ limit: limit.toString() },
	);
	return res.playlists;
}

interface PlaylistCatlistResponse {
	code: number;
	sub: { name: string; category: number; hot: boolean }[];
}

// 全部歌单分类
export async function getPlaylistCatlist() {
	const res = await api.get<PlaylistCatlistResponse>("/playlist/catlist");
	return res.sub;
}

interface TopSongResponse {
	code: number;
	// /api/v1/discovery/new/songs 的条目使用 artists/album/duration/picUrl 字段，
	// 与标准 Song（ar/al/dt）不同，需要规范化
	data: (Song & {
		artists?: Artist[];
		album?: Album;
		duration?: number;
		picUrl?: string;
	})[];
}

// 新歌速递
export async function getTopSong(type: number) {
	const res = await api.get<TopSongResponse>("/top/song", {
		type: type.toString(),
	});
	return res.data.map((song) => ({
		...song,
		ar: song.ar ?? song.artists ?? [],
		al: song.al ?? song.album ?? { id: 0, name: "", picUrl: song.picUrl },
		dt: song.dt ?? song.duration ?? 0,
	}));
}

// top/album、album/new 返回的专辑使用 artist（单数对象），统一补齐 artists 数组
function normalizeAlbum(album: Album & { artist?: Artist }): Album {
	return {
		...album,
		artists: album.artists ?? (album.artist ? [album.artist] : []),
	};
}

interface TopAlbumResponse {
	code: number;
	weekData: (Album & { artist?: Artist })[];
	monthData: (Album & { artist?: Artist })[];
}

// 新碟上架（热门：本周/本月）
export async function getTopAlbum(area: string) {
	const res = await api.get<TopAlbumResponse>("/top/album", { area });
	return {
		weekData: (res.weekData ?? []).map(normalizeAlbum),
		monthData: (res.monthData ?? []).map(normalizeAlbum),
	};
}

interface NewAlbumResponse {
	code: number;
	albums: (Album & { artist?: Artist })[];
	hasMore: boolean;
}

// 全部新碟
export async function getNewAlbums(
	area: string,
	limit: number = 50,
	offset: number = 0,
) {
	const res = await api.get<NewAlbumResponse>("/album/new", {
		area,
		limit: limit.toString(),
		offset: offset.toString(),
	});
	return {
		albums: (res.albums ?? []).map(normalizeAlbum),
		hasMore: res.hasMore,
	};
}

interface ArtistListResponse {
	code: number;
	artists: Artist[];
	more: boolean;
}

// 歌手分类列表
export async function getArtistList(
	type: number,
	area: number,
	initial: string = "",
	limit: number = 50,
	offset: number = 0,
) {
	const res = await api.get<ArtistListResponse>("/artist/list", {
		type: type.toString(),
		area: area.toString(),
		...(initial && { initial }),
		limit: limit.toString(),
		offset: offset.toString(),
	});
	return { artists: res.artists, hasMore: res.more };
}
