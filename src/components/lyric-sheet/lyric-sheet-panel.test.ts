import { describe, expect, it } from "vitest";
import {
	type LyricSheetPanel,
	toggleLyricSheetPanel,
} from "./lyric-sheet-panel";

describe("toggleLyricSheetPanel", () => {
	it("收起态点击任一面板即展开该面板", () => {
		expect(toggleLyricSheetPanel(null, "lyric")).toBe("lyric");
		expect(toggleLyricSheetPanel(null, "playlist")).toBe("playlist");
	});

	it("另一面板展开时点击会切换过去，而不是并存", () => {
		expect(toggleLyricSheetPanel("lyric", "playlist")).toBe("playlist");
		expect(toggleLyricSheetPanel("playlist", "lyric")).toBe("lyric");
	});

	it("点击已展开的面板则全部收起", () => {
		expect(toggleLyricSheetPanel("lyric", "lyric")).toBeNull();
		expect(toggleLyricSheetPanel("playlist", "playlist")).toBeNull();
	});

	it("任意点击序列下状态恒为「单个面板或 null」", () => {
		const panels: LyricSheetPanel[] = ["lyric", "playlist"];
		const states: (LyricSheetPanel | null)[] = [];
		let current: LyricSheetPanel | null = null;

		// 穷举两轮点击的全部组合（每个状态都作为后续点击的起点）
		for (const first of panels) {
			for (const second of panels) {
				for (const third of panels) {
					current = null;
					for (const target of [first, second, third]) {
						current = toggleLyricSheetPanel(current, target);
						states.push(current);
						// 单一状态：不可能是「两个面板同时展开」
						expect([null, "lyric", "playlist"]).toContain(current);
					}
				}
			}
		}

		// 覆盖到「切换」与「收起」两种结果，确认上面的穷举确实走到过分支
		expect(states).toContain("playlist");
		expect(states).toContain(null);
	});
});
