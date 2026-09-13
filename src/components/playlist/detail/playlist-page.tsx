import {
	Heart24Filled,
	Heart24Regular,
	LockClosed24Filled,
} from "@fluentui/react-icons";
import { useRef, useState } from "react";
import { Link } from "react-router-dom";
import { toast } from "sonner";
import { PinnedBar, usePinned } from "@/components/pinned-bar";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { WispButton } from "@/components/wisp-button";
import {
	WispCommandBar,
	WispCommandBarPlay,
	WispSearchField,
	WispSortMenu,
} from "@/components/wisp-command-bar";
import {
	WispDetailCoverImage,
	WispDetailHeader,
} from "@/components/wisp-detail-header";
import { subscribePlaylist } from "@/lib/services/playlist";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { useUserStore } from "@/lib/store/userStore/userStore";
import type { Playlist, Song } from "@/lib/types";
import { cn, formateDate } from "@/lib/utils";
import { PlaylistDeleteButton } from "../playlist-delete-button";
import { PlaylistEditButton } from "../playlist-edit-button";
import { PlaylistSongs } from "./playlist-songs";

export function PlaylistPage({
	playlist,
	songs,
	isSongsLoading = false,
	isMyPlaylist,
	onRefresh,
	onReorderSongs,
}: {
	playlist: Playlist;
	songs: Song[];
	isSongsLoading?: boolean;
	isMyPlaylist: boolean;
	onRefresh?: () => void;
	onReorderSongs?: (from: number, to: number) => void;
}) {
	const [searchOpen, setSearchOpen] = useState(false);
	const [searchQuery, setSearchQuery] = useState("");
	const [sortOption, setSortOption] = useState("date");

	const headerRef = useRef<HTMLDivElement>(null);
	const isPinned = usePinned(headerRef);
	const playList = usePlayerStore((s) => s.playList);

	const playlistId = playlist.id;
	const isFavList = isMyPlaylist && playlist.specialType === 5;
	const title = isFavList ? "我喜欢的音乐" : playlist.name;
	const coverImgUrl = playlist.coverImgUrl;

	const creatorName = playlist.creator.nickname;
	const creatorAvatarUrl = playlist.creator.avatarUrl;
	const createTime = playlist.createTime;

	const toggleLikePlaylist = useUserStore((s) => s.toggleLikePlaylist);
	const subscribedPlaylists = useUserStore((s) => s.subscribedPlaylists);
	const isSubscribed = subscribedPlaylists.some((pl) => pl.id === playlist.id);
	const LikeIcon = isSubscribed ? Heart24Filled : Heart24Regular;
	const isPrivacy = playlist.privacy === 10;
	const isLoggedin = useUserStore((s) => s.isLoggedin);

	async function handleLike() {
		const targetState = !isSubscribed;
		toggleLikePlaylist(playlist, targetState);
		try {
			const res = await subscribePlaylist(targetState ? 1 : 2, playlistId);
			if (!res) {
				toggleLikePlaylist(playlist, isSubscribed);
				toast.error("操作失败，请重试", { position: "top-center" });
			}
		} catch (err) {
			console.error("切换歌单喜欢状态失败", err);
			toggleLikePlaylist(playlist, isSubscribed);
			toast.error("操作失败，请重试", { position: "top-center" });
		}
	}

	return (
		<div className="flex h-full w-full flex-col">
			<WispDetailHeader
				ref={headerRef}
				cover={<WispDetailCoverImage src={coverImgUrl} alt={`${title} 封面`} />}
				title={title}
				badges={
					isPrivacy ? (
						<LockClosed24Filled className="size-6 text-foreground/40" />
					) : undefined
				}
				meta={
					<>
						<Link
							to={`/profile?uid=${playlist.creator.userId}`}
							className="group -ml-3 flex w-fit items-center gap-2 rounded-md px-3 py-2 transition-all duration-300 ease-in-out hover:translate-x-2 hover:bg-fill-hover"
						>
							<Avatar className="size-6 shadow-wisp-sm">
								<AvatarImage src={creatorAvatarUrl} />
								<AvatarFallback>CN</AvatarFallback>
							</Avatar>
							<span className="text-foreground/80 transition-colors group-hover:text-foreground">
								{creatorName}
							</span>
						</Link>
						<span className="text-foreground/60 text-sm">
							{formateDate(createTime)}
						</span>
					</>
				}
			/>

			<PinnedBar isPinned={isPinned} title={title}>
				<WispCommandBar
					leading={
						<>
							<WispCommandBarPlay
								disabled={playlist.trackCount === 0}
								onClick={() => {
									playList(playlistId, "list");
								}}
							/>
							{isMyPlaylist && !isFavList && (
								<>
									<PlaylistEditButton
										playlist={playlist}
										onSuccess={onRefresh}
									/>
									<PlaylistDeleteButton playlist={playlist} />
								</>
							)}
							{!isMyPlaylist && (
								<WispButton
									variant="glass"
									size="lg"
									aria-label={isSubscribed ? "取消收藏" : "收藏"}
									icon={
										<LikeIcon
											className={cn("size-4", isSubscribed && "text-red-500")}
										/>
									}
									onClick={() => {
										if (!isLoggedin) {
											toast.error("请先登录网易云账号");
											return;
										}
										handleLike();
									}}
								/>
							)}
						</>
					}
					trailing={
						<>
							<WispSortMenu value={sortOption} onChange={setSortOption} />

							<WispSearchField
								ariaLabel="搜索歌单内歌曲"
								value={searchQuery}
								onChange={setSearchQuery}
								open={searchOpen}
								onOpenChange={setSearchOpen}
							/>
						</>
					}
				/>
			</PinnedBar>

			<div className="relative px-8">
				<PlaylistSongs
					songs={songs}
					query={searchQuery}
					sort={sortOption}
					isLoading={isSongsLoading}
					onReorder={isMyPlaylist ? onReorderSongs : undefined}
				/>
			</div>
		</div>
	);
}
