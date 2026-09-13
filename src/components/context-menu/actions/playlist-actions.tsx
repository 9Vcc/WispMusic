import { Album24Regular } from "@fluentui/react-icons";
import { useNavigate } from "react-router-dom";
import { useContextMenuStore } from "@/lib/store/contextMenuStore/contextMenuStore";
import { ContextMenuButton } from "../context-menu-button";
import type { ActionProps } from "./action";

export function PlaylistActions({ type, data }: ActionProps) {
	// data 可能为 null（无歌曲场景），不判空会在下方 data.resourceType 处抛 TypeError
	if (!data) return null;
	if (type !== "playlist" && data.resourceType !== "list") return null;

	const { closeMenu } = useContextMenuStore();
	const navigate = useNavigate();

	return (
		<>
			<ContextMenuButton
				id="playlist-info"
				icon={<Album24Regular className="size-4" />}
				content="查看歌单"
				onClick={(e) => {
					e.stopPropagation();

					closeMenu();
					navigate(`/detail/playlist?id=${data.id}`);
				}}
			/>
		</>
	);
}
