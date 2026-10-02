import {
	MoreHorizontal24Regular,
	MusicNote224Filled,
	Play24Filled,
	PlayCircle24Filled,
} from "@fluentui/react-icons";
import type React from "react";
import { Link } from "react-router-dom";
import { useSongPrefetchOnHover } from "@/lib/player/prefetch";
import { useContextMenuStore } from "@/lib/store/contextMenuStore/contextMenuStore";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import type { Song } from "@/lib/types";
import { cn, formatDuration, GetThumbnail } from "@/lib/utils";
import { Button } from "../ui/button";
import {
	WispListRow,
	wispListRowActionClass,
	wispSongListGrid,
} from "../wisp-list-row";
import { AudioLinesIcon } from "./audio-lines-icon";

/** 行内所有"次要文字"的统一观感（歌手 / 专辑 / 时长） */
const secondaryTextClass = "text-foreground/60 text-sm";
const metaTextClass = "text-foreground/40 text-sm";

export function SongListItem({
	song,
	index,
	showCover = true,
	showAlbum = true,
	className,
	secondaryClassName,
	metaClassName,
}: {
	song: Song;
	index: number;
	showCover: boolean;
	showAlbum?: boolean;
	className?: string;
	secondaryClassName?: string;
	metaClassName?: string;
}) {
	const openMenu = useContextMenuStore((s) => s.openMenu);
	const playSong = usePlayerStore((s) => s.playSong);
	const currentSong = usePlayerStore((s) => s.currentSong);
	const togglePlay = usePlayerStore((s) => s.togglePlay);

	const gridTemplate = wispSongListGrid(showAlbum);

	function handleContextMenu(e: React.MouseEvent) {
		e.preventDefault();
		openMenu(e.clientX, e.clientY, "song", song);
	}

	const isPlaying = currentSong?.id === song.id;
	const coverUrl = song.al.picUrl;
	const isLocalMusic = song.localFilePath !== undefined;

	// 悬停驻留即预取：命中后点歌只剩本地解码耗时。已在播放的这一行不需要预取。
	const {
		onMouseEnter: handlePrefetchEnter,
		onMouseLeave: handlePrefetchLeave,
	} = useSongPrefetchOnHover(song, !isPlaying);

	return (
		<WispListRow
			index={index}
			className={cn("w-full", className)}
			onDoubleClick={() => playSong(song)}
			onMouseEnter={handlePrefetchEnter}
			onMouseLeave={handlePrefetchLeave}
			onDragStart={(e) => e.preventDefault()}
			onContextMenu={handleContextMenu}
		>
			<div className={cn("grid w-full items-center", gridTemplate)}>
				<div className="flex gap-4 items-center ">
					{showCover ? (
						<div className="w-10 h-10 relative rounded-sm overflow-hidden shrink-0 border">
							{coverUrl && coverUrl.length > 0 ? (
								<img
									src={GetThumbnail(song.al.picUrl!)}
									alt={`${song.al?.name} 专辑封面`}
									loading="lazy"
									draggable={false}
								/>
							) : (
								<div className="w-full h-full bg-card flex items-center justify-center">
									<MusicNote224Filled className="size-5 text-muted-foreground" />
								</div>
							)}

							{/* 封面播放遮罩：默认完全隐藏，行 hover 或键盘聚焦时浮出 */}
							{!isPlaying && (
								<button
									type="button"
									aria-label={`播放 ${song.name}`}
									className={cn(
										"absolute inset-0 flex items-center justify-center text-white",
										"opacity-0 transition-[opacity,background-color] duration-300",
										"group-hover:bg-black/50 group-hover:opacity-100",
										"focus-visible:bg-black/50 focus-visible:opacity-100",
									)}
									onClick={(e) => {
										e.stopPropagation();
										playSong(song);
									}}
								>
									<PlayCircle24Filled className="size-5" />
								</button>
							)}
							{isPlaying && (
								<button
									type="button"
									aria-label="暂停"
									className="absolute inset-0 flex items-center justify-center bg-black/50 text-white"
									onClick={(e) => {
										e.stopPropagation();
										togglePlay();
									}}
								>
									<AudioLinesIcon />
								</button>
							)}
						</div>
					) : (
						<div
							className={cn(
								"size-6 flex items-center justify-center text-foreground/40 relative",
								metaClassName,
							)}
						>
							{!isPlaying && (
								<>
									<span className="group-hover:hidden">{index + 1}</span>
									<button
										type="button"
										aria-label={`播放 ${song.name}`}
										className="hidden group-hover:flex hover:text-foreground/60"
										onClick={(e) => {
											e.stopPropagation();
											playSong(song);
										}}
									>
										<Play24Filled className="size-4" />
									</button>
								</>
							)}
							{isPlaying && (
								<button
									type="button"
									aria-label="暂停"
									className="absolute left-1/2 top-1/2 -translate-1/2 text-black"
									onClick={(e) => {
										e.stopPropagation();
										togglePlay();
									}}
								>
									<AudioLinesIcon
										className={cn(
											"text-foreground/40 hover:text-foreground/60",
											metaClassName,
										)}
									/>
								</button>
							)}
						</div>
					)}
					<span className="line-clamp-1 w-3/4 font-bold text-sm">
						{song.name}
					</span>
				</div>
				<div className="line-clamp-1 w-3/4">
					{song.ar!.map((ar, idx) =>
						isLocalMusic && !song.localNeteaseMatch ? (
							<span
								className={cn(
									secondaryTextClass,
									"font-medium",
									secondaryClassName,
								)}
							>
								{ar.name}
							</span>
						) : (
							<Link
								key={`${song.id}-${ar.id}-${idx}`}
								to={`/detail/artist?id=${ar.id}`}
								draggable={false}
							>
								<span
									className={cn(
										secondaryTextClass,
										"hover:text-foreground/80 font-medium",
										secondaryClassName,
									)}
								>
									{ar.name} {idx < song.ar!.length - 1 && "、"}
								</span>
							</Link>
						),
					)}
				</div>
				{showAlbum &&
					(song.al.name ? (
						isLocalMusic && !song.localNeteaseMatch ? (
							<span
								className={cn(
									secondaryTextClass,
									"line-clamp-1 w-3/4",
									secondaryClassName,
								)}
							>
								{song.al.name}
							</span>
						) : (
							<Link to={`/detail/album?id=${song.al.id}`} draggable={false}>
								<span
									className={cn(
										secondaryTextClass,
										"line-clamp-1 w-3/4 hover:text-foreground/80",
										secondaryClassName,
									)}
								>
									{song.al.name}
								</span>
							</Link>
						)
					) : (
						<span
							className={cn(
								secondaryTextClass,
								"line-clamp-1 w-3/4",
								secondaryClassName,
							)}
						>
							未知艺术家
						</span>
					))}
				<span className={cn(metaTextClass, metaClassName)}>
					{formatDuration((song.dt || 1) / 1000)}
				</span>
				<span>
					<Button
						variant="ghost"
						size="icon"
						className={wispListRowActionClass()}
						aria-label="更多操作"
						onClick={handleContextMenu}
					>
						<MoreHorizontal24Regular className="size-4" />
					</Button>
				</span>
			</div>
		</WispListRow>
	);
}
