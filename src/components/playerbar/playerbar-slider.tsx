import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { useProgressStore } from "@/lib/store/playerStore/progressStore";
import { WispSlider } from "../wisp-slider";
import { formatDuration } from "@/lib/utils";
import { useEffect, useState } from "react";
import { Vibrant } from "node-vibrant/browser";
import { useSettingStore } from "@/lib/store/settingStore/settingStore";

export function PlayerBarSlider() {
  const progress = useProgressStore((s) => s.progress);
  const seek = usePlayerStore((s) => s.seek);
  const duration = usePlayerStore((s) => s.duration);

  const currentSong = usePlayerStore((s) => s.currentSong);
  const coverUrl = currentSong?.al?.picUrl || currentSong?.album?.picUrl;
  const [coverColor, setCoverColor] = useState("");

  const theme = useSettingStore((s) => s.appearance.theme);

  useEffect(() => {
    if (!coverUrl) return;

    const v = new Vibrant(coverUrl);
    v.getPalette().then((palette) => {
      const vibrant =
        theme === "dark" ? palette.Vibrant?.hex : palette.DarkVibrant?.hex;
      setCoverColor(vibrant || "rgba(0, 0, 0, 0)");
    });
  }, [coverUrl]);

  return (
		// 贴顶边的全宽进度条：绝对定位于播放栏顶端，与顶部分隔线融合。
		// 时间信息只通过悬停气泡查看，不在栏上常驻显示。
		<div
			className="absolute inset-x-0 top-0 z-10"
			style={
				{
					"--dynamic-cover-color": coverColor || "black",
				} as React.CSSProperties
			}
		>
			{/* 容器高度与轨道同高（h-0.75），轨道填满容器即完全贴合栏顶边；
			    用透明伪元素上下扩展命中区域，避免 3px 细线难以悬停 */}
			<WispSlider
				value={[progress]}
				onValueChange={seek}
				max={100}
				step={0.1}
				hoverTooltip={(value) => formatDuration((duration * value) / 100)}
				className="h-0.75 after:absolute after:-inset-y-2.5 after:inset-x-0 after:content-['']"
				trackClassName="h-0.75! bg-fill-strong"
				rangeClassName="bg-[var(--dynamic-cover-color)]"
				thumbClassName="relative block size-2.5 rounded-full border border-black/10 bg-white opacity-0 scale-50 shadow-[0_1px_2px_rgba(0,0,0,0.08)] transition-[opacity,scale] duration-200 ease-out group-hover:opacity-100 group-hover:scale-110 group-active:opacity-100 group-active:scale-100 data-[dragging]:opacity-100 data-[dragging]:scale-110 focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-foreground/20"
				showThumb
				showThumbTooltip={false}
			/>
		</div>
	);
}
