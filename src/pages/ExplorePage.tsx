import {
	ChevronDown24Regular,
	CompassNorthwestRegular,
	Play24Filled,
} from "@fluentui/react-icons";
import { useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import useSWR from "swr";
import useSWRInfinite from "swr/infinite";
import { AlbumList } from "@/components/album/album-list";
import { ArtistList } from "@/components/artist/artist-list";
import { Entrance } from "@/components/entrance";
import { PinnedBar, usePinned } from "@/components/pinned-bar";
import {
	getArtistList,
	getHighQualityPlaylist,
	getNewAlbums,
	getPlaylistCatlist,
	getTopAlbum,
	getToplistDetail,
	getTopPlaylist,
	getTopSong,
	NEW_ALBUM_AREAS,
	NEW_SONG_AREAS,
	ARTIST_AREAS,
	ARTIST_TYPES,
	type ToplistDetailItem,
} from "@/lib/services/explore";
import { SongList } from "@/components/song/song-list";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { WispButton } from "@/components/wisp-button";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { useContextMenuStore } from "@/lib/store/contextMenuStore/contextMenuStore";
import { GetThumbnail, cn } from "@/lib/utils";
import type { Album, Artist, Playlist } from "@/lib/types";

const PAGE_SIZE = 50;
// 新碟一页数量较小：封面网格一次渲染过多 DOM 会造成卡顿
const ALBUM_PAGE_SIZE = 24;

// 热门歌单分类兜底（接口失败时仍可用）
const DEFAULT_HOT_CATS = [
	"华语",
	"欧美",
	"日语",
	"韩语",
	"流行",
	"摇滚",
	"民谣",
	"电子",
	"说唱",
	"古风",
	"ACG",
	"轻音乐",
];

const FEATURED_CHARTS = ["飙升榜", "新歌榜", "原创榜", "热歌榜"];

export default function ExplorePage() {
	const [searchParams, setSearchParams] = useSearchParams();
	const tab = searchParams.get("tab") ?? "playlist";

	const headerRef = useRef<HTMLDivElement>(null);
	const isPinned = usePinned(headerRef);

	return (
		<div className="flex min-h-full w-full flex-col py-8">
			<div ref={headerRef} className="mb-6 flex flex-col gap-4 px-8">
				<Entrance>
					<h1 className="flex items-center gap-3 text-2xl font-bold tracking-tight">
						<CompassNorthwestRegular className="size-7 text-primary" />
						探索
					</h1>
				</Entrance>
				<Entrance delay={0.06}>
					<p className="text-sm text-foreground/50">
						发现歌单、榜单、新歌、新碟与歌手
					</p>
				</Entrance>
			</div>

			<Tabs
				value={tab}
				onValueChange={(v) =>
					setSearchParams(v === "playlist" ? {} : { tab: v })
				}
			>
				<PinnedBar isPinned={isPinned} title="探索">
					<div className="pl-8">
						<TabsList>
							<TabsTrigger value="playlist">歌单</TabsTrigger>
							<TabsTrigger value="chart">排行榜</TabsTrigger>
							<TabsTrigger value="newSong">新歌</TabsTrigger>
							<TabsTrigger value="newAlbum">新碟</TabsTrigger>
							<TabsTrigger value="artist">歌手</TabsTrigger>
						</TabsList>
					</div>
				</PinnedBar>

				<div className="px-8">
					<TabsContent value="playlist" className="mt-6">
						<PlaylistTab />
					</TabsContent>
					<TabsContent value="chart" className="mt-6">
						<ChartTab />
					</TabsContent>
					<TabsContent value="newSong" className="mt-6">
						<NewSongTab />
					</TabsContent>
					<TabsContent value="newAlbum" className="mt-6">
						<NewAlbumTab />
					</TabsContent>
					<TabsContent value="artist" className="mt-6">
						<ArtistTab />
					</TabsContent>
				</div>
			</Tabs>
		</div>
	);
}

/** 分类筛选按钮行 */
function CategoryPills<T extends string | number>({
	options,
	value,
	onChange,
}: {
	options: readonly { name: string; value: T }[];
	value: T;
	onChange: (v: T) => void;
}) {
	return (
		<div className="flex flex-wrap items-center gap-2">
			{options.map((opt) => (
				<button
					type="button"
					key={opt.value}
					onClick={() => onChange(opt.value)}
					className={cn(
						"rounded-full px-3.5 py-1.5 text-sm transition-all duration-200 select-none",
						value === opt.value
							? "bg-primary text-primary-foreground shadow-sm"
							: "bg-fill-subtle text-foreground/70 hover:bg-fill-hover hover:text-foreground",
					)}
				>
					{opt.name}
				</button>
			))}
		</div>
	);
}

/** 加载更多按钮 */
function LoadMore({
	visible,
	onClick,
	isLoading,
}: {
	visible: boolean;
	onClick: () => void;
	isLoading?: boolean;
}) {
	if (!visible) return null;
	return (
		<div className="flex justify-center py-8">
			<WispButton
				variant="outline"
				size="lg"
				icon={
					<ChevronDown24Regular
						className={cn("size-4 transition-transform", isLoading && "animate-bounce")}
					/>
				}
				onClick={onClick}
			>
				加载更多
			</WispButton>
		</div>
	);
}

// ==================== 歌单 Tab ====================

function PlaylistTab() {
	// 支持从 URL 读取初始分类（如首页“查看更多”直达精品歌单）
	const [searchParams] = useSearchParams();
	const initialCat = searchParams.get("cat") ?? "全部";
	const [cat, setCat] = useState(initialCat);
	const isHighQuality = cat === "精品歌单";

	const { data: catlist } = useSWR("explore/playlistCatlist", getPlaylistCatlist, {
		revalidateOnFocus: false,
	});

	const hotCats =
		catlist?.filter((c) => c.hot).map((c) => c.name).slice(0, 14) ??
		DEFAULT_HOT_CATS;
	const cats = ["全部", "精品歌单", ...hotCats];

	const { data, size, setSize, isLoading } = useSWRInfinite(
		(index: number) => ["explore/playlist", cat, index],
		([, , index]: [string, string, number]) => {
			if (isHighQuality) {
				return getHighQualityPlaylist(50).then((playlists) => ({
					playlists,
					hasMore: false,
				}));
			}
			return getTopPlaylist(cat, PAGE_SIZE, index * PAGE_SIZE);
		},
		{ revalidateOnFocus: false },
	);

	const playlists = data?.flatMap((d: { playlists: Playlist[] }) => d.playlists) ?? [];
	const hasMore = data?.[data.length - 1]?.hasMore ?? false;

	return (
		<div className="flex flex-col gap-6">
			<CategoryPills
				options={cats.map((name) => ({ name, value: name }))}
				value={cat}
				onChange={setCat}
			/>
			<PlaylistCoverGrid playlists={playlists} isLoading={isLoading && size === 1} />
			<LoadMore
				visible={hasMore && !isHighQuality}
				onClick={() => setSize(size + 1)}
				isLoading={isLoading}
			/>
		</div>
	);
}

/** 歌单封面网格（与首页推荐歌单同款视觉）。
 *  auto-fill + minmax 让列数随窗口宽度平滑增减，封面不会在固定断点间忽大忽小 */
function PlaylistCoverGrid({
	playlists,
	isLoading,
}: {
	playlists: Playlist[];
	isLoading: boolean;
}) {
	return (
		<div className="grid grid-cols-[repeat(auto-fill,minmax(8rem,1fr))] gap-x-5 gap-y-8">
			{isLoading
				? Array.from({ length: 10 }, (_, i) => (
						<div key={i} className="flex w-full flex-col gap-3">
							<Skeleton className="aspect-square w-full rounded-lg" />
							<Skeleton className="h-4 w-4/5" />
						</div>
					))
				: playlists.map((playlist) => (
						<ExplorePlaylistCard key={playlist.id} playlist={playlist} />
					))}
		</div>
	);
}

function formatPlayCount(count: number) {
	if (count >= 100_000_000)
		return `${(count / 100_000_000).toFixed(1)}亿`;
	if (count >= 10_000) return `${(count / 10_000).toFixed(1)}万`;
	return count.toString();
}

function ExplorePlaylistCard({ playlist }: { playlist: Playlist }) {
	const playList = usePlayerStore((s) => s.playList);
	const openMenu = useContextMenuStore((s) => s.openMenu);

	return (
		<div
			className="flex w-full flex-col gap-3"
			onContextMenu={(e) => {
				e.preventDefault();
				openMenu(e.clientX, e.clientY, "playlist", playlist);
			}}
		>
			<div className="group relative aspect-square w-full overflow-hidden rounded-lg border drop-shadow-md">
				<Link to={`/detail/playlist?id=${playlist.id}`}>
					<img
						className="h-full w-full object-cover transition duration-300 group-hover:scale-105"
						src={GetThumbnail(playlist.coverImgUrl)}
						alt={`${playlist.name} 歌单封面`}
						loading="lazy"
					/>
				</Link>
				{playlist.playCount > 0 && (
					<span className="pointer-events-none absolute top-2 right-2 rounded-full bg-black/45 px-2 py-0.5 text-xs text-white/90 opacity-100 backdrop-blur-sm transition-opacity duration-300 group-hover:opacity-0">
						{formatPlayCount(playlist.playCount)}
					</span>
				)}
				<div className="pointer-events-none absolute inset-0 flex items-center justify-center bg-black/50 text-white opacity-0 transition duration-300 group-hover:opacity-100">
					<Play24Filled
						className="pointer-events-auto size-8 cursor-pointer transition-all duration-300 hover:scale-110"
						onClick={(e) => {
							e.preventDefault();
							e.stopPropagation();
							playList(playlist.id, "list");
						}}
					/>
				</div>
			</div>
			<p className="line-clamp-2 w-full text-sm font-medium select-text">
				{playlist.name}
			</p>
		</div>
	);
}

// ==================== 排行榜 Tab ====================

function ChartTab() {
	const { data, isLoading } = useSWR("explore/toplistDetail", getToplistDetail, {
		revalidateOnFocus: false,
	});

	if (isLoading) {
		return (
			<div className="grid grid-cols-1 gap-6 xl:grid-cols-2">
				{Array.from({ length: 4 }, (_, i) => (
					<Skeleton key={i} className="h-44 rounded-xl" />
				))}
			</div>
		);
	}

	if (!data?.length) {
		return (
			<p className="py-16 text-center text-sm text-foreground/50">
				暂时无法获取榜单
			</p>
		);
	}

	const featured = FEATURED_CHARTS.map((name) =>
		data.find((l) => l.name === name),
	).filter(Boolean) as ToplistDetailItem[];
	const rest = data.filter(
		(l) => !FEATURED_CHARTS.includes(l.name),
	);

	return (
		<div className="flex flex-col gap-10">
			<div className="grid grid-cols-1 gap-6 xl:grid-cols-2">
				{featured.map((chart, i) => (
					<Entrance key={chart.id} delay={i * 0.05}>
						<ChartCard chart={chart} />
					</Entrance>
				))}
			</div>

			{rest.length > 0 && (
				<div className="flex flex-col gap-4">
					<h2 className="text-xl font-bold">更多榜单</h2>
					<PlaylistCoverGrid
						playlists={rest.map(({ tracks: _summary, ...playlist }) => playlist)}
						isLoading={false}
					/>
				</div>
			)}
		</div>
	);
}

function ChartCard({ chart }: { chart: ToplistDetailItem }) {
	// /toplist/detail 的 tracks 是摘要格式：first=歌名 second=歌手 third=专辑；
	// 同时兼容完整 Song 格式（name/ar）
	const tracks = chart.tracks?.slice(0, 5) ?? [];

	return (
		<Link
			to={`/detail/playlist?id=${chart.id}`}
			className="group flex gap-5 rounded-xl p-3 transition-colors duration-300 hover:bg-fill-hover"
		>
			{/* 窄窗口（<lg）下只显示封面，歌曲列表隐藏，避免卡片内容换行 */}
			<div className="relative aspect-square w-28 shrink-0 self-center overflow-hidden rounded-lg drop-shadow-md sm:w-32 lg:w-36">
				<img
					className="h-full w-full object-cover transition duration-300 group-hover:brightness-60"
					src={GetThumbnail(chart.coverImgUrl)}
					alt={`${chart.name} 榜单封面`}
					loading="lazy"
				/>
				<span className="absolute right-0 bottom-0 rounded-tl-md bg-black/50 px-1.5 py-0.5 text-xs text-white/90 backdrop-blur-sm">
					{chart.updateFrequency}
				</span>
			</div>
			<div className="hidden min-w-0 flex-1 flex-col justify-center gap-1.5 xl:flex">
				<span className="mb-1 text-base font-bold">{chart.name}</span>
				{tracks.map((track, i) => (
					<span
						key={`${track.first ?? track.name}-${i}`}
						className="truncate text-sm text-foreground/60"
					>
						<span className="mr-2 text-foreground/40 tabular-nums">{i + 1}</span>
						{track.first ?? track.name}
						{(track.second ?? track.ar?.map((ar) => ar.name).join("/")) && (
							<span className="ml-2 text-foreground/35">
								{track.second ?? track.ar?.map((ar) => ar.name).join("/")}
							</span>
						)}
					</span>
				))}
			</div>
		</Link>
	);
}

// ==================== 新歌 Tab ====================

function NewSongTab() {
	const [area, setArea] = useState(0);

	const { data, isLoading } = useSWR(
		["explore/topSong", area],
		([, a]) => getTopSong(a),
		{ revalidateOnFocus: false },
	);

	return (
		<div className="flex flex-col gap-6">
			<CategoryPills options={NEW_SONG_AREAS} value={area} onChange={setArea} />
			{isLoading ? (
				<div className="flex flex-col gap-4">
					{Array.from({ length: 8 }, (_, i) => (
						<Skeleton key={i} className="h-12 w-full" />
					))}
				</div>
			) : data?.length ? (
				<SongList songList={data} showAlbum />
			) : (
				<p className="py-16 text-center text-sm text-foreground/50">
					暂时无法获取新歌速递
				</p>
			)}
		</div>
	);
}

// ==================== 新碟 Tab ====================

const ALBUM_TYPE_OPTIONS = [
	{ name: "热门", value: "hot" as const },
	{ name: "最新", value: "new" as const },
];

function NewAlbumTab() {
	const [area, setArea] = useState("ALL");
	const [type, setType] = useState<"hot" | "new">("hot");

	const { data: topAlbum, isLoading: isTopLoading } = useSWR(
		["explore/topAlbum", area],
		([, a]) => getTopAlbum(a),
		{ revalidateOnFocus: false },
	);

	const { data, size, setSize, isLoading } = useSWRInfinite(
		(index: number) => ["explore/newAlbums", area, index],
		([, , index]: [string, string, number]) =>
			getNewAlbums(area, ALBUM_PAGE_SIZE, index * ALBUM_PAGE_SIZE),
		{ revalidateOnFocus: false },
	);

	const newAlbums =
		data?.flatMap((d: { albums: Album[] }) => d.albums) ?? [];
	const hasMore = data?.[data.length - 1]?.hasMore ?? false;

	return (
		<div className="flex flex-col gap-6">
			<div className="flex flex-wrap items-center gap-x-6 gap-y-3">
				<CategoryPills
					options={NEW_ALBUM_AREAS}
					value={area}
					onChange={setArea}
				/>
				<CategoryPills
					options={ALBUM_TYPE_OPTIONS}
					value={type}
					onChange={setType}
				/>
			</div>

			{type === "hot" ? (
				isTopLoading ? (
					<AlbumGridSkeleton />
				) : (
					<div className="flex flex-col gap-10">
						{topAlbum?.weekData?.length ? (
							<div className="flex flex-col gap-4">
								<h2 className="text-xl font-bold">本周新碟</h2>
								<AlbumList albumList={topAlbum.weekData} showDate />
							</div>
						) : null}
						{topAlbum?.monthData?.length ? (
							<div className="flex flex-col gap-4">
								<h2 className="text-xl font-bold">本月新碟</h2>
								<AlbumList albumList={topAlbum.monthData} showDate />
							</div>
						) : null}
						{!topAlbum?.weekData?.length && !topAlbum?.monthData?.length && (
							<p className="py-16 text-center text-sm text-foreground/50">
								暂时无法获取新碟
							</p>
						)}
					</div>
				)
			) : isLoading && size === 1 ? (
				<AlbumGridSkeleton />
			) : (
				<div className="flex flex-col gap-4">
					<h2 className="text-xl font-bold">全部新碟</h2>
					<AlbumList albumList={newAlbums} showDate />
					<LoadMore
						visible={hasMore}
						onClick={() => setSize(size + 1)}
						isLoading={isLoading}
					/>
				</div>
			)}
		</div>
	);
}

function AlbumGridSkeleton() {
	return (
		<div className="grid grid-cols-[repeat(auto-fill,minmax(8rem,1fr))] gap-12">
			{Array.from({ length: 10 }, (_, i) => (
				<div key={i} className="flex w-full flex-col gap-3">
					<Skeleton className="aspect-square w-full rounded-md" />
					<Skeleton className="h-4 w-4/5" />
				</div>
			))}
		</div>
	);
}

// ==================== 歌手 Tab ====================

const ARTIST_INITIALS = Array.from({ length: 26 }, (_, i) =>
	String.fromCharCode(65 + i),
);

function ArtistTab() {
	const [area, setArea] = useState(-1);
	const [type, setType] = useState(1);
	const [initial, setInitial] = useState("");

	const { data, size, setSize, isLoading } = useSWRInfinite(
		(index: number) => ["explore/artistList", area, type, initial, index],
		([, , , , index]: [string, number, number, string, number]) =>
			getArtistList(type, area, initial, PAGE_SIZE, index * PAGE_SIZE),
		{ revalidateOnFocus: false },
	);

	const artists =
		data?.flatMap((d: { artists: Artist[] }) => d.artists) ?? [];
	const hasMore = data?.[data.length - 1]?.hasMore ?? false;

	return (
		<div className="flex flex-col gap-6">
			<CategoryPills options={ARTIST_AREAS} value={area} onChange={setArea} />
			<CategoryPills options={ARTIST_TYPES} value={type} onChange={setType} />

			<div className="flex flex-wrap items-center gap-2">
				<button
					type="button"
					onClick={() => setInitial("")}
					className={cn(
						"rounded-full px-3 py-1 text-sm transition-all duration-200 select-none",
						initial === ""
							? "bg-primary text-primary-foreground shadow-sm"
							: "bg-fill-subtle text-foreground/70 hover:bg-fill-hover",
					)}
				>
					全部
				</button>
				{ARTIST_INITIALS.map((letter) => (
					<button
						type="button"
						key={letter}
						onClick={() => setInitial(letter)}
						className={cn(
							"rounded-full px-2 py-1 text-sm tabular-nums transition-all duration-200 select-none",
							initial === letter
								? "bg-primary text-primary-foreground shadow-sm"
								: "bg-fill-subtle text-foreground/60 hover:bg-fill-hover hover:text-foreground",
						)}
					>
						{letter}
					</button>
				))}
			</div>

			{isLoading && size === 1 ? (
				<div className="grid grid-cols-[repeat(auto-fill,minmax(8rem,1fr))] gap-12">
					{Array.from({ length: 15 }, (_, i) => (
						<div key={i} className="flex flex-col items-center gap-3">
							<Skeleton className="size-28 rounded-full" />
							<Skeleton className="h-4 w-16" />
						</div>
					))}
				</div>
			) : artists?.length ? (
				<ArtistList artistList={artists as Artist[]} />
			) : (
				<p className="py-16 text-center text-sm text-foreground/50">
					暂时无法获取歌手列表
				</p>
			)}

			<LoadMore
				visible={hasMore}
				onClick={() => setSize(size + 1)}
				isLoading={isLoading}
			/>
		</div>
	);
}
