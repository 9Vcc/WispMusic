/**
 * 全屏切换的过渡编排。
 *
 * 为什么需要手动编排：Tauri 的 `setFullscreen()` 是操作系统级的窗口模式切换，
 * 窗口尺寸、边框、任务栏可见性都由合成器瞬时更换，CSS 无法对它们做补间。
 * 若直接调用，用户会看到内容一次性回流、边框与标题栏骤变的「硬切」。
 *
 * 因此采用「遮罩式换场」，把硬切藏进不可见的那一帧：
 *
 *   ① 收束 (out) —— 一层与当前表面同色的遮罩淡入，把画面完全盖住；
 *   ② 换场 (swap) —— 在遮罩已不透明的时刻执行窗口切换，硬切被遮住；
 *   ③ 展开 (in)  —— 遮罩淡出，露出已经重排好的新布局。
 *
 * 为什么内容层不做缩放/位移：本项目的 `plus-lighter` 混合文字（歌词页、
 * 环境色详情页）要求其祖先链上不能出现 transform/filter —— 一旦出现就会形成
 * 层叠上下文，把混合从页面背景上隔离出去（见 lyric-sheet-background.tsx 与
 * lyric-sheet.tsx 的既有注释）。给 #root 加 transform 会直接影响详情页的混合，
 * 且会把含 WebGL 着色器背景的整个应用提升成一张整屏纹理。遮罩是独立于内容
 * 子树之外的 fixed 层，天然规避这两个问题，因此过渡全部由它承担。
 *
 * 时长与缓动曲线通过 CSS 变量下发给样式表（--fs-out-ms / --fs-in-ms），
 * 保证 JS 的等待与 CSS 的补间永远一致，不会因为改一处忘一处而错位。
 */

const VEIL_CLASS_NAME = "wisp-fs-veil";
const VEIL_ATTR = "data-fs-veil";

/**
 * 收束阶段时长。对应 Fluent UI 的 short (150ms) 档；略收紧到 140ms，
 * 让「响应」这一拍显得干脆。
 */
export const FS_OUT_MS = 140;
/**
 * 展开阶段时长。对应 Fluent UI 的 medium (300ms) 档；收到 260ms，
 * 既保留落位的从容感，又不至于让整次切换拖过半秒。
 */
export const FS_IN_MS = 260;

/** 歌词页打开时发布的背景基色变量（见 lyric-sheet-background.tsx） */
const PUBLISHED_COLOR_VAR = "--fs-veil-color";
/** 本次切换实际采用的遮罩色。独立变量，避免覆盖歌词页发布的值 */
const ACTIVE_COLOR_VAR = "--fs-veil-active-color";

let veilElement: HTMLDivElement | null = null;
let isRunning = false;
let timingVarsInjected = false;

function injectTimingVars(root: HTMLElement): void {
	if (timingVarsInjected) return;
	timingVarsInjected = true;
	root.style.setProperty("--fs-out-ms", `${FS_OUT_MS}ms`);
	root.style.setProperty("--fs-in-ms", `${FS_IN_MS}ms`);
}

/**
 * 遮罩挂在 <html> 下、与 <body> 平级，而不是挂在应用根节点里：
 * 这样它不受应用内任何层级变换的影响，始终铺满视口、不会在边缘露缝。
 */
function ensureVeil(): HTMLDivElement {
	if (veilElement?.isConnected) return veilElement;

	const element = document.createElement("div");
	element.className = VEIL_CLASS_NAME;
	element.setAttribute("aria-hidden", "true");
	document.documentElement.appendChild(element);
	veilElement = element;

	// 先提交一次静止态（opacity: 0）。否则首次淡入时浏览器会把
	// 「新建 + 设为目标态」合并到同一帧，补间被吞掉，表现为瞬间跳变。
	void element.offsetWidth;
	return element;
}

/**
 * 遮罩颜色要贴合「此刻屏幕上真正可见的表面」，否则切换瞬间会闪出一块异色。
 * 优先级：歌词页发布的基色 → 当前环境色详情页的 --main-background
 * → 根节点的 --main-background。
 */
function resolveVeilColor(): string {
	const root = document.documentElement;

	const published = getComputedStyle(root)
		.getPropertyValue(PUBLISHED_COLOR_VAR)
		.trim();
	if (published) return published;

	// 环境色详情页把 --main-background 覆写成封面主色，但作用域在该页元素上，
	// 根节点上的遮罩取不到，需要按当前可见页面读一次
	const ambient = document.querySelector<HTMLElement>(".ambient-page");
	if (ambient) {
		const scoped = getComputedStyle(ambient)
			.getPropertyValue("--main-background")
			.trim();
		if (scoped) return scoped;
	}

	return getComputedStyle(root).getPropertyValue("--main-background").trim();
}

/**
 * macOS 的原生全屏自带 Space 切换过渡，语义上不需要再叠一层遮罩编排
 * （叠加反而会让两段动画互相盖住，看起来更糊）。Windows / Linux 是硬切，
 * 才需要本模块的遮罩换场。
 */
function shouldSkipTransition(): boolean {
	if (typeof navigator === "undefined") return true;
	return /Mac|iPhone|iPad|iPod/i.test(navigator.userAgent);
}

function wait(ms: number): Promise<void> {
	return new Promise((resolve) => {
		window.setTimeout(resolve, ms);
	});
}

function nextFrame(): Promise<void> {
	return new Promise((resolve) => {
		requestAnimationFrame(() => resolve());
	});
}

export interface FullscreenTransitionOptions {
	/** prefers-reduced-motion：为真时不做任何过渡，直接切换 */
	reduceMotion: boolean;
	/**
	 * 真正执行窗口切换的动作。
	 * 应在 resolve 之前确认新尺寸已被呈现（例如多等两帧），
	 * 否则遮罩揭开时可能仍看到旧尺寸的画面。
	 */
	swap: () => Promise<void>;
}

export async function runFullscreenTransition({
	reduceMotion,
	swap,
}: FullscreenTransitionOptions): Promise<void> {
	// 过渡进行中忽略重复触发：连点不会再叠加一次换场。
	// 真实的全屏状态由窗口 resize 监听回填（use-app-window），UI 不会失同步。
	if (isRunning) return;

	// 减弱动态：直接切换，不再补一层淡出，避免把「不动」又变回动效
	if (reduceMotion || shouldSkipTransition()) {
		await swap();
		return;
	}

	isRunning = true;
	const root = document.documentElement;
	injectTimingVars(root);

	const veilColor = resolveVeilColor();
	if (veilColor) root.style.setProperty(ACTIVE_COLOR_VAR, veilColor);

	const veil = ensureVeil();

	try {
		// ① 收束：遮罩淡入盖住画面。只动一个 fixed 层，不触碰应用内容，
		//    因此不触发应用重排重绘，是整条链路里最便宜的一步。
		root.setAttribute(VEIL_ATTR, "on");
		void veil.offsetWidth; // 提交起始态，确保补间真的跑起来
		await wait(FS_OUT_MS);
		await nextFrame(); // 补一拍，避开合成器落后一帧导致遮罩尚未全不透明

		// ② 换场：此刻画面已被完全盖住，硬切不可见
		await swap();

		// ③ 展开：遮罩淡出，露出新布局
		root.removeAttribute(VEIL_ATTR);
		await wait(FS_IN_MS);
	} finally {
		// 兜底清理，避免异常路径把属性/变量留在 DOM 上
		root.removeAttribute(VEIL_ATTR);
		root.style.removeProperty(ACTIVE_COLOR_VAR);
		isRunning = false;
	}
}
