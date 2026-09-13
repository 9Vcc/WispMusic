import { TextAdd20Regular } from "@fluentui/react-icons";
import { useState } from "react";
import { useSongLogic } from "@/hooks/use-song-logic";
import { useUserStore } from "@/lib/store/userStore/userStore";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { cn } from "@/lib/utils";
import { WispButton } from "../wisp-button";
import { HoverPopover } from "./hover-popover";

/**
 * 播放栏“添加到歌单”控件（参考 EchoMusic 的 PlayerBar 添加到按钮）：
 * 悬停弹出当前用户的歌单列表，点击即把当前歌曲加入对应歌单。
 */
export function PlayerBarAddToPlaylist() {
	const currentSong = usePlayerStore((s) => s.currentSong);
	const isLoggedin = useUserStore((s) => s.isLoggedin);
	const favPlaylist = useUserStore((s) => s.favPlaylist);
	const createdPlaylists = useUserStore((s) => s.createdPlaylists);
	const { handleAddToPlaylist, handleNextPlay } = useSongLogic();
	const [pendingId, setPendingId] = useState<number | null>(null);

	const collectablePlaylists = favPlaylist
		? [favPlaylist, ...createdPlaylists]
		: createdPlaylists;

	const canAdd = isLoggedin && currentSong;

	function handleSelect(pid: number) {
		if (!currentSong || pendingId !== null) return;
		setPendingId(pid);
		void Promise.resolve(handleAddToPlaylist(pid, currentSong)).finally(() =>
			setPendingId(null),
		);
	}

	return (
		<HoverPopover
			contentClassName="w-56"
			align="start"
			trigger={
				<WispButton
					variant="ghost"
					aria-label="添加到"
					icon={<TextAdd20Regular className="size-5" />}
					className={cn(!canAdd && "text-foreground/40")}
				/>
			}
		>
			<div className="flex max-h-72 w-full flex-col gap-0.5 overflow-y-auto p-1">
				{!isLoggedin ? (
					<div className="px-2 py-3 text-xs text-foreground/50 select-none">
						当前仅支持登录后收藏
					</div>
				) : currentSong ? (
					<>
						<button
							type="button"
							className="flex w-full items-center justify-between gap-2 rounded-md border border-dashed border-border px-2.5 py-2 text-left transition-colors duration-200 hover:border-foreground/30 hover:text-foreground disabled:opacity-50"
							onClick={() => {
								void handleNextPlay(currentSong);
							}}
						>
							<span className="min-w-0 truncate text-xs font-medium">
								加入播放列表
							</span>
						</button>
						{collectablePlaylists.length > 0 ? (
							collectablePlaylists.map((playlist) => (
								<button
									type="button"
									key={playlist.id}
									disabled={pendingId !== null}
									className="flex w-full items-center justify-between gap-2 rounded-md px-2.5 py-2 text-left transition-colors duration-200 hover:bg-fill-hover disabled:opacity-50"
									onClick={() => handleSelect(playlist.id)}
								>
									<span className="min-w-0 truncate text-xs font-medium">
										{playlist.name}
									</span>
									<span className="shrink-0 text-[10px] text-foreground/50 tabular-nums">
										{playlist.trackCount ?? 0}
									</span>
								</button>
							))
						) : (
							<div className="px-2 py-3 text-xs text-foreground/50 select-none">
								请先创建歌单
							</div>
						)}
					</>
				) : (
					<div className="px-2 py-3 text-xs text-foreground/50 select-none">
						未在播放
					</div>
				)}
			</div>
		</HoverPopover>
	);
}
