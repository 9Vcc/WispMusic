/**
 * 歌词页两侧视图面板（歌词 / 播放列表）的互斥状态。
 *
 * 两个面板在布局上占用的是同一块区域：
 * - 歌词面板 = 右半区去掉右缘 48px 工具槽；
 * - 播放列表 = 右半区左移 48px（`right-24` + `w-[calc(50%-48px)]`）。
 * 换句话说它们**不可能并存**——一旦同时渲染，两整块文字就会直接叠在
 * 一起（用户可见的「歌词和播放列表互相遮挡」）。
 *
 * 因此这里用单一状态表达「当前展开哪个面板」而不是两个各自独立的
 * boolean：并存状态在类型上就不成立，任何入口（底部栏按钮、打开/收起
 * 歌词页、外部关闭事件）都只能改写同一个值，不会再出现两个标志位
 * 各自漂移（例如收起时只复位了其中一个）导致的重叠渲染。
 *
 * `null` = 两个面板都收起，此时封面信息区独占整宽。
 */
export type LyricSheetPanel = "lyric" | "playlist";

/**
 * 底部栏面板切换按钮的行为：
 * - 点击已展开的面板 → 全部收起（回到 `null`，信息区展开为整宽）；
 * - 点击另一个面板 / 在收起态点击 → 切换到该面板。
 *
 * 返回值恒为「单一面板或 null」，天然满足互斥约束。
 */
export function toggleLyricSheetPanel(
	current: LyricSheetPanel | null,
	target: LyricSheetPanel,
): LyricSheetPanel | null {
	return current === target ? null : target;
}
