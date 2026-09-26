import { useEffect, useRef, useState, type ReactNode } from "react";
import { Popover, PopoverContent, PopoverAnchor } from "../ui/popover";

const CLOSE_DELAY_MS = 150;

interface HoverPopoverProps {
	trigger: ReactNode;
	children: ReactNode;
	/** 传给 PopoverContent 的额外 className */
	contentClassName?: string;
	align?: "start" | "center" | "end";
	side?: "top" | "bottom";
	sideOffset?: number;
}

/**
 * 悬停触发的 Popover：移入触发区或内容区保持打开，移出后延迟关闭。
 * 用 PopoverAnchor 而非 Trigger，触发区点击行为完全由调用方控制
 * （如音量按钮点击是静音，而不是切换弹层）。
 */
export function HoverPopover({
	trigger,
	children,
	contentClassName,
	align = "center",
	side = "top",
	sideOffset = 12,
}: HoverPopoverProps) {
	const [open, setOpen] = useState(false);
	const closeTimer = useRef<number | null>(null);

	const cancelClose = () => {
		if (closeTimer.current !== null) {
			clearTimeout(closeTimer.current);
			closeTimer.current = null;
		}
	};
	const handleOpen = () => {
		cancelClose();
		setOpen(true);
	};
	const handleClose = () => {
		cancelClose();
		closeTimer.current = window.setTimeout(() => {
			setOpen(false);
			closeTimer.current = null;
		}, CLOSE_DELAY_MS);
	};

	// 卸载时清掉待决关闭定时器：移出后 150ms 内卸载（切页/歌单刷新重挂载）
	// 会让回调挂在已卸载组件上
	useEffect(() => {
		return () => {
			if (closeTimer.current !== null) {
				clearTimeout(closeTimer.current);
				closeTimer.current = null;
			}
		};
	}, []);

	return (
		<Popover open={open} onOpenChange={(next) => (next ? handleOpen() : handleClose())}>
			<PopoverAnchor asChild>
				<span
					className="inline-flex"
					onMouseEnter={handleOpen}
					onMouseLeave={handleClose}
				>
					{trigger}
				</span>
			</PopoverAnchor>
			<PopoverContent
				side={side}
				align={align}
				sideOffset={sideOffset}
				className={contentClassName}
				onMouseEnter={handleOpen}
				onMouseLeave={handleClose}
			>
				{children}
			</PopoverContent>
		</Popover>
	);
}
