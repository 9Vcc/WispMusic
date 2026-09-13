import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { cn } from "@/lib/utils";
import { sfGaugeWithNeedle } from "@bradleyhodges/sfsymbols";
import SFIcon from "@bradleyhodges/sfsymbols-react";
import { Popover, PopoverContent, PopoverTrigger } from "../ui/popover";
import { WispButton } from "../wisp-button";
import { Slider } from "../ui/slider";

const SPEED_PRESETS = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0] as const;
const SPEED_SLIDER_MIN = 1; // 内部以 0.1 为刻度整数运算，避免浮点步进误差
const SPEED_SLIDER_MAX = 50;

/**
 * 倍速播放控件：点击打开倍速面板（滑条 0.1x ~ 5.0x + 预设档位），
 * 非原速时按钮高亮。
 */
export function PlayerBarSpeedPopover() {
	const playbackRate = usePlayerStore((s) => s.playbackRate);
	const setPlaybackRate = usePlayerStore((s) => s.setPlaybackRate);
	const isActive = Math.abs(playbackRate - 1) > 0.001;

	return (
		<Popover>
			<PopoverTrigger asChild>
				<WispButton
					variant="ghost"
					icon={
						<SFIcon
							icon={sfGaugeWithNeedle}
							className={cn(
								"size-5 transition-colors",
								isActive
									? "text-primary"
									: "text-foreground/80 hover:text-foreground",
							)}
						/>
					}
				/>
			</PopoverTrigger>
			<PopoverContent
				side="top"
				sideOffset={12}
				className="w-80 p-4 gap-3"
			>
				<div className="flex items-center justify-between">
					<span className="text-[11px] font-bold text-foreground/50 select-none">
						倍速播放
					</span>
					<button
						type="button"
						onClick={() => setPlaybackRate(1)}
						className={cn(
							"rounded-md px-1.5 py-0.5 text-[13px] font-extrabold tabular-nums transition-colors",
							playbackRate === 1
								? "text-foreground/40"
								: "text-foreground hover:bg-fill-hover",
						)}
					>
						{playbackRate.toFixed(1)}x
					</button>
				</div>
				<div className="flex items-center gap-2">
					<span className="shrink-0 text-[10px] font-semibold text-foreground/40 select-none">
						0.1
					</span>
					<Slider
						value={[Math.round(playbackRate * 10)]}
						min={SPEED_SLIDER_MIN}
						max={SPEED_SLIDER_MAX}
						step={1}
						onValueChange={(value) => setPlaybackRate(value[0] / 10)}
						className="h-5 flex-1"
					/>
					<span className="shrink-0 text-[10px] font-semibold text-foreground/40 select-none">
						5x
					</span>
				</div>
				<div className="grid grid-cols-7 gap-1">
					{SPEED_PRESETS.map((rate) => {
						const active = Math.abs(playbackRate - rate) < 0.001;
						return (
							<button
								key={rate}
								type="button"
								onClick={() => setPlaybackRate(rate)}
								className={cn(
									"rounded-md py-1 text-[11px] font-semibold tabular-nums text-center transition-colors",
									active
										? "bg-primary/15 text-primary"
										: "text-foreground/50 hover:bg-fill-hover hover:text-foreground",
								)}
							>
								{rate}x
							</button>
						);
					})}
				</div>
			</PopoverContent>
		</Popover>
	);
}
