import { ArrowSync24Regular } from "@fluentui/react-icons";
import Pinyin from "pinyin-match";
import { useMemo, useRef, useState } from "react";
import { PinnedBar, usePinned } from "@/components/pinned-bar";
import { SongList } from "@/components/song/song-list";
import { WispButton } from "@/components/wisp-button";
import {
	WispCommandBar,
	WispCommandBarPlay,
	WispSearchField,
	WispSortMenu,
} from "@/components/wisp-command-bar";
import { LocalTrackToSong } from "@/lib/services/localMusic";
import { useLocalMusicStore } from "@/lib/store/localMusicStore/localMusicStore";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";

function matchesPinyin(text: string, query: string): boolean {
	const normalizedQuery = query.toLowerCase();
	return (
		text.toLowerCase().includes(normalizedQuery) || !!Pinyin.match(text, query)
	);
}

export default function LocalPage() {
	const localTracks = useLocalMusicStore((s) => s.tracks);
	const isScanning = useLocalMusicStore((s) => s.isScanning);
	const scanAll = useLocalMusicStore((s) => s.scanAll);
	const scanDirs = useLocalMusicStore((s) => s.scanDirs);
	const playQueue = usePlayerStore((s) => s.playQueue);

	const [searchOpen, setSearchOpen] = useState(false);
	const [searchQuery, setSearchQuery] = useState("");
	const [sortOption, setSortOption] = useState("date");

	// 页面顶部的哨兵元素：滚动超过阈值后 isPinned，顶栏浮现模糊层和标题
	const sentinelRef = useRef<HTMLDivElement>(null);
	const isPinned = usePinned(sentinelRef);

	const songs = useMemo(() => localTracks.map(LocalTrackToSong), [localTracks]);

	const visibleSongs = useMemo(() => {
		const query = searchQuery.trim();
		let result = [...songs];

		if (query) {
			result = result.filter(
				(song) =>
					matchesPinyin(song.name, query) ||
					(song.al?.name && matchesPinyin(song.al.name, query)) ||
					song.ar?.some((artist) => matchesPinyin(artist.name, query)),
			);
		}

		if (result.length === 0) return result;

		return [...result].sort((a, b) => {
			switch (sortOption) {
				case "name":
					return a.name.localeCompare(b.name, "zh-CN");
				case "artist":
					return (a.ar?.[0]?.name || "").localeCompare(
						b.ar?.[0]?.name || "",
						"zh-CN",
					);
				case "album":
					return (a.al?.name || "").localeCompare(b.al?.name || "", "zh-CN");
				case "duration":
					return (b.dt || 0) - (a.dt || 0);
				case "date":
				default:
					return 0;
			}
		});
	}, [searchQuery, songs, sortOption]);

	function handlePlayVisibleSongs() {
		if (visibleSongs.length === 0) return;
		void playQueue(visibleSongs);
	}

	const emptyText =
		songs.length > 0
			? "没有找到匹配的本地歌曲"
			: scanDirs.length === 0
				? "前往设置添加音乐文件夹"
				: "未找到音乐文件";

	return (
		<div className="relative flex min-h-full w-full flex-1 flex-col pb-8">
			<div
				ref={sentinelRef}
				aria-hidden="true"
				className="absolute top-4 left-0 h-px w-px"
			/>

			<PinnedBar isPinned={isPinned} title="本地歌曲">
				<WispCommandBar
					leading={
						<>
							<WispCommandBarPlay
								onClick={handlePlayVisibleSongs}
								disabled={visibleSongs.length === 0}
							/>

							<WispButton
								icon={
									<ArrowSync24Regular
										className={isScanning ? "animate-spin" : ""}
									/>
								}
								variant="glass"
								size="lg"
								onClick={scanAll}
								disabled={isScanning || scanDirs.length === 0}
								aria-label={isScanning ? "扫描中" : "重新扫描"}
							/>
						</>
					}
					trailing={
						<>
							<WispSortMenu value={sortOption} onChange={setSortOption} />

							<WispSearchField
								ariaLabel="搜索本地歌曲"
								value={searchQuery}
								onChange={setSearchQuery}
								open={searchOpen}
								onOpenChange={setSearchOpen}
							/>
						</>
					}
				/>
			</PinnedBar>

			<div className="h-full w-full flex-1 px-8">
				{visibleSongs.length === 0 && !isScanning ? (
					<div className="flex h-48 flex-col items-center justify-center gap-2 text-muted-foreground">
						<span className="text-sm">{emptyText}</span>
					</div>
				) : (
					<SongList songList={visibleSongs} showCover={true} showAlbum={true} />
				)}
			</div>
		</div>
	);
}
