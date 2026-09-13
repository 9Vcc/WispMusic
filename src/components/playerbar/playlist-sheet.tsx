import {
	Delete16Regular,
	Dismiss16Regular,
	List24Regular,
} from "@fluentui/react-icons";

import {
	SheetTrigger,
	SheetContent,
	SheetHeader,
	SheetTitle,
	Sheet,
} from "../ui/sheet";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { PlaylistSongPreview } from "./playlist-song-preview";
import { useUserStore } from "@/lib/store/userStore/userStore";
import { useMemo, useState } from "react";
import { WispButton } from "../wisp-button";
import { Virtuoso } from "react-virtuoso";

export function PlaylistSheet() {
	const playlist = usePlayerStore((s) => s.playlist);
	const currentSong = usePlayerStore((s) => s.currentSong);
	const clearPlaylist = usePlayerStore((s) => s.clearPlaylist);

	const { likeListSet } = useUserStore();
	const [open, setOpen] = useState(false);

	const initialIndex = useMemo(
		() =>
			currentSong ? playlist.findIndex((s) => s.id === currentSong.id) : 0,
		[currentSong, playlist],
	);

	return (
		<Sheet open={open} onOpenChange={setOpen} modal={false}>
			<div className="relative">
				<SheetTrigger asChild>
					<WispButton variant="ghost" icon={<List24Regular className="size-5" />} />
				</SheetTrigger>
				{playlist.length > 0 && (
					<span className="absolute -top-0.5 -right-1 z-10 flex h-4 min-w-4 items-center justify-center rounded-full bg-primary px-1 text-[9px] font-medium leading-none text-primary-foreground select-none">
						{playlist.length > 99 ? "99+" : playlist.length}
					</span>
				)}
			</div>
			<SheetContent
				className="bg-card/90 p-2 pr-0 w-full backdrop-blur-xl"
				showCloseButton={false}
				container={document.getElementById("main-wrapper")}
			>
				<SheetHeader>
					<div className="flex justify-between items-center">
						<SheetTitle>播放列表</SheetTitle>
						<div className="flex gap-2">
							<WispButton
								variant="ghost"
								onClick={clearPlaylist}
								icon={<Delete16Regular />}
							/>
							<WispButton
								variant="ghost"
								onClick={() => setOpen(false)}
								icon={<Dismiss16Regular />}
							/>
						</div>
					</div>
				</SheetHeader>

				<div className="flex-1 w-full -mt-8 no-scrollbar!">
					<Virtuoso
						className="no-scrollbar!"
						data={playlist}
						initialTopMostItemIndex={initialIndex !== -1 ? initialIndex : 0}
						itemContent={(_, song) => (
							<div className="px-4 py-4">
								<PlaylistSongPreview
									song={song}
									isPlaying={song.id === currentSong?.id}
									isLike={likeListSet.has(Number(song.id))}
									setOpen={setOpen}
								/>
							</div>
						)}
					/>
				</div>
			</SheetContent>
		</Sheet>
	);
}
