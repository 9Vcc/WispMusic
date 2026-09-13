import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { getToplistDetail, type ToplistDetailItem } from "@/lib/services/explore";
import { Skeleton } from "@/components/ui/skeleton";
import { Link } from "react-router-dom";
import { PlayCircle24Filled } from "@fluentui/react-icons";
import { useEffect, useState } from "react";

// 首页展示的三个榜单（网易云官网经典三列：飙升/新歌/热歌）
const FEATURED_TOPLIST_NAMES = ["飙升榜", "新歌榜", "热歌榜"];

function pickFeaturedToplists(list: ToplistDetailItem[]) {
	const picked: ToplistDetailItem[] = [];
	for (const name of FEATURED_TOPLIST_NAMES) {
		const found = list.find((item) => item.name?.includes(name));
		if (found) picked.push(found);
	}
	// 接口榜单名变动时兜底：用前三个榜单补齐
	for (const item of list) {
		if (picked.length >= 3) break;
		if (!picked.includes(item)) picked.push(item);
	}
	return picked.slice(0, 3);
}

export function ToplistSection() {
	const playList = usePlayerStore((s) => s.playList);
	const [toplists, setToplists] = useState<ToplistDetailItem[] | null>(null);

	useEffect(() => {
		let cancelled = false;
		getToplistDetail()
			.then((list) => {
				if (!cancelled) setToplists(pickFeaturedToplists(list ?? []));
			})
			.catch(() => {
				if (!cancelled) setToplists([]);
			});
		return () => {
			cancelled = true;
		};
	}, []);

	// 加载中先渲染同布局骨架，保证区块顺序稳定、内容按序填充
	if (toplists === null) {
		return (
			<section className="flex flex-col gap-4">
				<h2 className="text-xl font-semibold">热门排行榜</h2>
				<div className="grid grid-cols-1 gap-4 md:grid-cols-3">
					{Array.from({ length: 3 }, (_, i) => (
						<Skeleton key={i} className="h-58 rounded-lg" />
					))}
				</div>
			</section>
		);
	}
	if (toplists.length === 0) return null;

	return (
		<section className="flex flex-col gap-4">
			<h2 className="text-xl font-semibold">热门排行榜</h2>

			<div className="grid grid-cols-1 gap-4 md:grid-cols-3">
				{toplists.map((toplist) => (
					<ToplistCard key={toplist.id} toplist={toplist} onPlay={playList} />
				))}
			</div>
		</section>
	);
}

function ToplistCard({
	toplist,
	onPlay,
}: {
	toplist: ToplistDetailItem;
	onPlay: (listId: number | string, listType: "list") => void;
}) {
	const tracks = (toplist.tracks ?? []).slice(0, 5);

	return (
		<Link
			to={`/detail/playlist?id=${toplist.id}`}
			draggable={false}
			className="group/card flex flex-col gap-3 rounded-lg border border-border bg-card/60 p-4 backdrop-blur-xl transition-colors duration-300 hover:border-foreground/20"
		>
			<div className="flex items-center justify-between">
				<div className="flex min-w-0 flex-col">
					<span className="truncate text-sm font-bold group-hover/card:text-primary">
						{toplist.name}
					</span>
					<span className="text-xs text-foreground/50 select-none">
						{toplist.updateFrequency}
					</span>
				</div>
				<button
					type="button"
					aria-label={`播放${toplist.name}`}
					className="shrink-0 text-foreground/60 transition-all duration-200 hover:scale-110 hover:text-foreground active:scale-95"
					onClick={(e) => {
						e.preventDefault();
						e.stopPropagation();
						onPlay(toplist.id, "list");
					}}
				>
					<PlayCircle24Filled className="size-7" />
				</button>
			</div>

			<div className="flex flex-col">
				{tracks.map((track, index) => (
					<div
						key={`${track.first}-${index}`}
						className="flex items-center gap-3 rounded-md px-1 py-1.5 transition-colors duration-200 group-hover/card:bg-transparent hover:bg-fill-hover!"
					>
						<span
							className={
								"w-5 shrink-0 text-center text-sm font-bold tabular-nums select-none " +
								(index < 3 ? "text-primary" : "text-foreground/40")
							}
						>
							{index + 1}
						</span>
						<span className="min-w-0 flex-1 truncate text-sm">
							{track.first ?? track.name}
						</span>
						<span className="max-w-[40%] shrink-0 truncate text-xs text-foreground/50">
							{track.second ?? track.ar?.map((a) => a.name).join("、")}
						</span>
					</div>
				))}
			</div>
		</Link>
	);
}
