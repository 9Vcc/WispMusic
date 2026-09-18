import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useState } from "react";
import { usePlayerStore } from "@/lib/store/playerStore/playerStore";
import { GetThumbnail } from "@/lib/utils";
import {
	extractBackgroundColors,
	getCachedBackgroundColors,
} from "@/lib/utils/color-extractor";
import { MeshGradient } from "../mesh-gradient/mesh-gradient-background";
import { lyricSheetEase } from "./lyric-sheet-motion";

interface LyricSheetBackgroundProps {
	onColorsChange?: (colors: [number, number, number][]) => void;
}

const BACKGROUND_PIPELINE_VERSION = 5;

interface BackgroundFrame {
	key: string;
	order: number;
	colors: [number, number, number][];
	textureData: ImageData | null;
}

export const DEFAULT_LYRIC_SHEET_COLORS: [number, number, number][] = [
	[0.1, 0.1, 0.18],
	[0.09, 0.07, 0.24],
	[0.06, 0.2, 0.38],
	[0.12, 0.08, 0.3],
	[0.1, 0.1, 0.18],
];

export function LyricSheetBackground({
	onColorsChange,
}: LyricSheetBackgroundProps) {
	const currentSong = usePlayerStore((s) => s.currentSong);
	const coverUrl = currentSong?.al?.picUrl || currentSong?.album?.picUrl;

	const [background, setBackground] = useState<BackgroundFrame | null>(() => {
		if (!coverUrl) return null;
		const cached = getCachedBackgroundColors(coverUrl);
		if (!cached) return null;
		return {
			key: `${coverUrl}-${BACKGROUND_PIPELINE_VERSION}`,
			order: 1,
			colors: cached.mesh as [number, number, number][],
			textureData: cached.texture,
		};
	});

	useEffect(() => {
		let active = true;

		async function getColors() {
			if (!coverUrl) {
				setBackground(null);
				return;
			}
			const nextColors = await extractBackgroundColors(coverUrl);
			if (!active) return;
			setBackground((previous) => ({
				key: `${coverUrl}-${BACKGROUND_PIPELINE_VERSION}`,
				order: (previous?.order ?? 0) + 1,
				colors: nextColors.mesh as [number, number, number][],
				textureData: nextColors.texture,
			}));
			onColorsChange?.(nextColors.palette as [number, number, number][]);
		}

		getColors();

		return () => {
			active = false;
		};
	}, [coverUrl, onColorsChange]);

	const baseColor = getBackgroundBase(
		background?.colors ?? DEFAULT_LYRIC_SHEET_COLORS,
	);

	// 把背景基色发布到根节点，供全屏切换遮罩取用
	// （见 src/lib/window/fullscreenTransition.ts）：全屏按钮就在歌词页标题栏上，
	// 遮罩必须贴合当前背景，否则切换瞬间会闪出一块异色。
	useEffect(() => {
		const root = document.documentElement;
		root.style.setProperty("--fs-veil-color", baseColor);
		return () => {
			root.style.removeProperty("--fs-veil-color");
		};
	}, [baseColor]);

	return (
		<motion.div
			className="absolute inset-0 z-0 overflow-hidden"
			// 缩放只作用于背景层（plus-lighter 文字的 backdrop，而非其祖先），
			// 不会隔离混合；从 1.06 收敛到 1 形成“展开”的推进感。
			initial={{ opacity: 0, scale: 1.06 }}
			animate={{ opacity: 1, scale: 1 }}
			transition={{ duration: 0.62, ease: lyricSheetEase }}
			style={{
				backgroundColor: baseColor,
			}}
		>
			<div className="relative h-full w-full overflow-hidden">
				{coverUrl && (
					<div
						className="absolute -inset-[8%]"
						style={{
							// 这一层只是渐入期垫底的模糊色块（上面会淡入 mesh 渐变），
							// 52px 模糊下 400px 与原图肉眼无差；用原图（常 1300px，
							// 解码后 ~7MB）渲染纯属浪费，而且它和下面的 WebGL 画布
							// 是歌词页内存峰值的主要来源
							backgroundImage: `url("${GetThumbnail(coverUrl, 400).replace(/"/g, "%22")}")`,
							backgroundPosition: "center",
							backgroundSize: "cover",
							// Mirrors applyBgRenderGrade so the pre-fade cover layer
							// matches the mesh texture that fades in above it.
							filter:
								"blur(52px) contrast(40%) saturate(300%) contrast(170%) brightness(75%)",
							transform: "scale(1.12)",
						}}
					/>
				)}
				<div
					className="absolute inset-0"
					style={{
						backgroundColor: "rgb(10 12 16 / 12%)",
						zIndex: 1,
					}}
				/>
				<AnimatePresence initial={false}>
					{background && (
						<motion.div
							key={background.key}
							className="absolute inset-0"
							initial={{ opacity: 0 }}
							animate={{ opacity: 1 }}
							exit={{ opacity: 1 }}
							transition={{ duration: 0.65, ease: "easeOut" }}
							style={{ zIndex: background.order + 2 }}
						>
							<MeshGradient
								colors={background.colors}
								textureData={background.textureData}
							/>
						</motion.div>
					)}
				</AnimatePresence>
			</div>
		</motion.div>
	);
}

function getBackgroundBase(colors: [number, number, number][]) {
	const average = colors.reduce(
		(result, color) => {
			result[0] += color[0];
			result[1] += color[1];
			result[2] += color[2];
			return result;
		},
		[0, 0, 0],
	);
	const divisor = Math.max(1, colors.length);
	return `rgb(${Math.round((average[0] / divisor) * 200)} ${Math.round((average[1] / divisor) * 200)} ${Math.round((average[2] / divisor) * 200)})`;
}
