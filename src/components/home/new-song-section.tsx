import { getTopSong } from "@/lib/services/explore";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { Skeleton } from "@/components/ui/skeleton";
import type { Song } from "@/lib/types";
import { GetThumbnail, formatDuration } from "@/lib/utils";
import { Link } from "react-router-dom";
import {
  CaretLeft24Filled,
  CaretRight24Filled,
  PlayCircle24Filled,
} from "@fluentui/react-icons";
import { useEffect, useRef, useState } from "react";
import { WispButton } from "../wisp-button";

function getNewSongItemsPerPage(width: number) {
	if (width >= 1760) return 9;
	if (width >= 1440) return 8;
	if (width >= 1024) return 6;
	if (width >= 768) return 5;
	if (width >= 520) return 4;
	return 3;
}

/** 新歌速递：横向滚动卡片，点击卡片直接播放 */
export function NewSongSection() {
	const playSong = usePlayerStore((s) => s.playSong);
	const [songs, setSongs] = useState<Song[] | null>(null);
	const containerRef = useRef<HTMLDivElement>(null);
	const [hasOverflow, setHasOverflow] = useState(false);
	const [canScrollPrev, setCanScrollPrev] = useState(false);
	const [canScrollNext, setCanScrollNext] = useState(false);

	useEffect(() => {
		let cancelled = false;
		getTopSong(0)
			.then((list) => {
				if (!cancelled) setSongs(list.slice(0, 18));
			})
			.catch(() => {
				if (!cancelled) setSongs([]);
			});
		return () => {
			cancelled = true;
		};
	}, []);

	const updateScrollState = () => {
		const container = containerRef.current;
		if (!container) return;
		const maxScrollLeft = container.scrollWidth - container.clientWidth;
		const epsilon = 1;
		setHasOverflow(maxScrollLeft > epsilon);
		setCanScrollPrev(container.scrollLeft > epsilon);
		setCanScrollNext(container.scrollLeft < maxScrollLeft - epsilon);
	};

	useEffect(() => {
		if (!songs) return;
		updateScrollState();

		const container = containerRef.current;
		if (!container) return;

		const observer = new ResizeObserver(updateScrollState);
		observer.observe(container);
		container.addEventListener("scroll", updateScrollState, {
			passive: true,
		});
		return () => {
			observer.disconnect();
			container.removeEventListener("scroll", updateScrollState);
		};
	}, [songs]);

	// 加载中先渲染同布局骨架，保证区块顺序稳定、内容按序填充
	if (songs === null) {
		return (
			<section className="flex flex-col gap-4">
				<h2 className="text-xl font-semibold">新歌速递</h2>
				<div className="flex gap-5 overflow-hidden">
					{Array.from({ length: 8 }, (_, i) => (
						<div key={i} className="flex w-32 shrink-0 flex-col gap-2">
							<Skeleton className="aspect-square w-full rounded-lg" />
							<Skeleton className="h-4 w-4/5" />
							<Skeleton className="h-3 w-3/5" />
						</div>
					))}
				</div>
			</section>
		);
	}
	if (songs.length === 0) return null;

	const scrollByPage = (direction: 1 | -1) => {
		const container = containerRef.current;
		if (!container) return;
		const pageSize =
			(container.firstElementChild as HTMLElement | null)?.offsetWidth ?? 200;
		container.scrollBy({
			left: direction * pageSize * getNewSongItemsPerPage(container.clientWidth),
			behavior: "smooth",
		});
	};

	return (
		<section className="flex flex-col gap-4">
			<div className="flex items-center justify-between">
				<h2 className="text-xl font-semibold">新歌速递</h2>

				{hasOverflow && (
					<div className="flex items-center gap-2">
						<WispButton
							variant="ghost"
							icon={<CaretLeft24Filled className="size-3" />}
							className="size-6 rounded-full bg-card border! border-border! text-muted-foreground hover:text-muted-foreground"
							onClick={() => scrollByPage(-1)}
							disabled={!canScrollPrev}
						/>
						<WispButton
							variant="ghost"
							icon={<CaretRight24Filled className="size-3" />}
							className="size-6 rounded-full bg-card border! border-border! text-muted-foreground hover:text-muted-foreground"
							onClick={() => scrollByPage(1)}
							disabled={!canScrollNext}
						/>
					</div>
				)}
			</div>

			<div
				ref={containerRef}
				className="w-full overflow-x-auto scroll-smooth [&::-webkit-scrollbar]:hidden"
				style={{
					scrollbarWidth: "none",
					msOverflowStyle: "none",
				}}
			>
				<div
					className="flex gap-5"
					style={{
						// 与最近常听区块一致：整行宽度收缩为内容宽度，避免最后一页留白
						width: "fit-content",
					}}
				>
					{songs.map((song) => (
						<NewSongCard
							key={song.id}
							song={song}
							onPlay={() => playSong(song)}
						/>
					))}
				</div>
			</div>
		</section>
	);
}

function NewSongCard({
	song,
	onPlay,
}: {
	song: Song;
	onPlay: () => void;
}) {
	const cover = song.al?.picUrl || "";

	return (
		<div className="group flex w-32 shrink-0 flex-col gap-2">
			<div className="relative aspect-square w-full overflow-hidden rounded-lg border shadow-md transition-transform duration-300 hover:scale-[1.02]">
				<img
					src={GetThumbnail(cover)}
					alt={`${song.al?.name ?? ""} 封面`}
					loading="lazy"
					draggable={false}
					className="size-full object-cover transition duration-300 group-hover:brightness-50"
				/>
				<button
					type="button"
					aria-label={`播放 ${song.name}`}
					className="absolute inset-0 flex items-center justify-center bg-black/50 opacity-0 transition duration-300 group-hover:opacity-100"
					onClick={onPlay}
				>
					<PlayCircle24Filled
						className="size-8 text-white transition-all duration-300 hover:scale-110"
					/>
				</button>
			</div>
			<div className="flex min-w-0 flex-col gap-0.5">
				<span className="line-clamp-1 text-sm font-medium" title={song.name}>
					{song.name}
				</span>
				<div className="flex items-center justify-between gap-2">
					<div className="line-clamp-1">
						{song.ar?.map((ar, idx) => (
							<Link
								to={`/detail/artist?id=${ar.id}`}
								key={`${ar.id}-${idx}`}
								draggable={false}
								onClick={(e) => e.stopPropagation()}
							>
								<span className="text-xs text-foreground/60 hover:text-foreground/80">
									{ar.name}
									{idx < (song.ar?.length ?? 0) - 1 && "、"}
								</span>
							</Link>
						))}
					</div>
					{song.dt > 0 && (
						<span className="shrink-0 text-[10px] text-foreground/40 tabular-nums select-none">
							{formatDuration(song.dt / 1000)}
						</span>
					)}
				</div>
			</div>
		</div>
	);
}
