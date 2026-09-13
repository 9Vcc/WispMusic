import { getHighQualityPlaylist } from "@/lib/services/explore";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { Skeleton } from "@/components/ui/skeleton";
import type { Playlist } from "@/lib/types";
import { GetThumbnail } from "@/lib/utils";
import { Link } from "react-router-dom";
import { ChevronRight24Regular, PlayCircle24Filled } from "@fluentui/react-icons";
import { useEffect, useState } from "react";

function formatPlayCount(count: number) {
	if (count >= 100000000) return `${(count / 100000000).toFixed(1)}亿`;
	if (count >= 10000) return `${(count / 10000).toFixed(1)}万`;
	return String(count);
}

export function HighQualitySection() {
	const playList = usePlayerStore((s) => s.playList);
	const [playlists, setPlaylists] = useState<Playlist[] | null>(null);

	useEffect(() => {
		let cancelled = false;
		getHighQualityPlaylist(12)
			.then((list) => {
				if (!cancelled) setPlaylists(list ?? []);
			})
			.catch(() => {
				if (!cancelled) setPlaylists([]);
			});
		return () => {
			cancelled = true;
		};
	}, []);

	// 加载中先渲染同布局骨架，保证区块顺序稳定、内容按序填充
	if (playlists === null) {
		return (
			<section className="flex flex-col gap-4">
				<h2 className="text-xl font-semibold">精品歌单</h2>
				<div className="grid grid-cols-[repeat(auto-fill,minmax(7.5rem,1fr))] gap-4">
					{Array.from({ length: 12 }, (_, i) => (
						<div key={i} className="flex w-full flex-col gap-2">
							<Skeleton className="aspect-square w-full rounded-lg" />
							<Skeleton className="h-4 w-4/5" />
						</div>
					))}
				</div>
			</section>
		);
	}
	if (playlists.length === 0) return null;

	return (
		<section className="flex flex-col gap-4">
			<div className="flex items-center justify-between">
				<h2 className="text-xl font-semibold">精品歌单</h2>
				<Link
					to="/explore?tab=playlist&cat=%E7%B2%BE%E5%93%81%E6%AD%8C%E5%8D%95"
					draggable={false}
					className="flex items-center gap-1 text-sm text-foreground/60 transition-colors duration-200 hover:text-foreground"
				>
					查看更多
					<ChevronRight24Regular className="size-4" />
				</Link>
			</div>

			<div className="grid grid-cols-[repeat(auto-fill,minmax(7.5rem,1fr))] gap-4">
				{playlists.map((playlist) => (
					<div key={playlist.id} className="flex w-full flex-col gap-2">
						<div className="relative aspect-square w-full overflow-hidden rounded-lg border shadow-md transition-transform duration-300 hover:scale-[1.02]">
							<Link
								to={`/detail/playlist?id=${playlist.id}`}
								draggable={false}
								className="block size-full"
							>
								<img
									src={GetThumbnail(playlist.coverImgUrl)}
									alt={`${playlist.name} 封面`}
									loading="lazy"
									draggable={false}
									className="size-full object-cover transition duration-300 group-hover:brightness-50"
								/>
								<div className="absolute inset-0 flex items-center justify-center bg-black/50 opacity-0 transition duration-300 hover:opacity-100">
									<PlayCircle24Filled
										className="size-8 text-white transition-all duration-300 hover:scale-110"
										onClick={(e) => {
											e.preventDefault();
											e.stopPropagation();
											playList(playlist.id, "list");
										}}
									/>
								</div>
							</Link>
							{playlist.playCount > 0 && (
								<span className="absolute top-2 right-2 rounded-sm bg-black/50 px-1.5 py-0.5 text-[10px] font-medium text-white backdrop-blur-sm select-none">
									{formatPlayCount(playlist.playCount)}
								</span>
							)}
						</div>
						<Link
							to={`/detail/playlist?id=${playlist.id}`}
							draggable={false}
							className="line-clamp-2 text-sm font-medium hover:text-primary"
						>
							{playlist.name}
						</Link>
					</div>
				))}
			</div>
		</section>
	);
}
