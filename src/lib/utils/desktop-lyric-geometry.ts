/**
 * 桌面歌词窗口几何计算（纯函数，主窗口同步引擎与控件窗口共用）。
 *
 * 这里的"设计尺寸"是方向/字号确定时的标准窗口逻辑尺寸：主窗口按它
 * 驱动 show/resize 落位；控件窗口按它算内容缩放比（用户手动拖拽窗口
 * 边缘改变实际尺寸后，内容等比适配，字号视觉随窗口缩放）。
 */
import type { DesktopLyricDirection } from "@/lib/store/settingStore/settingStore";

export type { DesktopLyricDirection };

/** 桌面歌词窗口逻辑尺寸（随方向/字号计算，主窗口单点下发） */
export interface DesktopLyricGeometry {
	width: number;
	height: number;
}

/**
 * 横向窗口顶部工具栏（歌名 / 传输控制 / 显示工具，EchoMusic 式三区
 * 头部）的保留高度：窗口几何把它算进去，控件窗口算歌词舞台设计高度
 * 时再扣除（竖向无头部、不扣）。
 */
export const DESKTOP_LYRIC_HEADER_RESERVE = 48;

/** 横向窗口：宽度吃满合理上限、高度容纳头部 + 双行（当前句 + 下一句预览）；竖向窗口：高度吃满合理上限、宽度跟随字号（含多列） */
export function computeDesktopLyricGeometry(
	direction: DesktopLyricDirection,
	fontSize: number,
): DesktopLyricGeometry {
	if (direction === "horizontal") {
		// 双行：2 × 1.35 行高 + 行距 + 纵向呼吸空间 ≈ 3.4em，外加顶部工具栏
		return {
			width: 1000,
			height:
				Math.min(320, Math.max(140, Math.round(fontSize * 3.4))) +
				DESKTOP_LYRIC_HEADER_RESERVE,
		};
	}
	// 竖排：宽度容纳最多 3 列（笔画动效按句长拆列），高度是纵向卷轴的主轴
	return {
		width: Math.min(440, Math.max(220, Math.round(fontSize * 4.8))),
		height: 620,
	};
}
