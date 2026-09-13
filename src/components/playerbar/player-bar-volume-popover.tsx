import { useEffect, useRef } from "react";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { cn } from "@/lib/utils";
import {
	sfSpeaker,
	sfSpeakerWave1,
	sfSpeakerWave2,
	sfSpeakerWave3,
} from "@bradleyhodges/sfsymbols";
import SFIcon from "@bradleyhodges/sfsymbols-react";
import { WispButton } from "../wisp-button";
import { Slider } from "../ui/slider";
import { HoverPopover } from "./hover-popover";

/**
 * 音量控件：悬停弹出竖向音量条，点击切换静音（记住静音前的音量）。
 */
export function PlayerBarVolumePopover() {
	const volume = usePlayerStore((s) => s.volume);
	const updateVolume = usePlayerStore((s) => s.updateVolume);
	const lastAudibleRef = useRef(0.7);

	useEffect(() => {
		if (volume > 0) lastAudibleRef.current = volume;
	}, [volume]);

	const VolumeButton =
		volume === 0
			? sfSpeaker
			: volume < 0.3
				? sfSpeakerWave1
				: volume < 0.7
					? sfSpeakerWave2
					: sfSpeakerWave3;

	const toggleMute = () => {
		if (volume > 0) updateVolume(0);
		else updateVolume(lastAudibleRef.current || 0.7);
	};

	return (
		<HoverPopover
			contentClassName="w-auto px-3 py-4"
			trigger={
				<WispButton
					variant="ghost"
					onClick={toggleMute}
					className={cn(volume === 0 && "text-foreground/50")}
					icon={
						<div className="flex items-center justify-center">
							<SFIcon icon={VolumeButton} className="size-5" />
						</div>
					}
				/>
			}
		>
			<div className="flex flex-col items-center gap-2">
				<span className="text-xs tabular-nums text-foreground/60 select-none">
					{Math.round(volume * 100)}
				</span>
				<Slider
					orientation="vertical"
					value={[volume]}
					onValueChange={(value) => updateVolume(value[0])}
					max={1}
					step={0.01}
					className="h-28"
				/>
			</div>
		</HoverPopover>
	);
}
