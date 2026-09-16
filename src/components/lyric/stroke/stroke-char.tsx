import {
	type CSSProperties,
	useCallback,
	useEffect,
	useRef,
	useState,
} from "react";
import { loadHanziCharData } from "@/lib/utils/hanzi-strokes";
import {
	exitDistancePx,
	FLY_PATH_NATURAL_MS,
	FLY_STAGGER_SPACE_MS,
	type FlyTimeline,
	fitFlyTimeline,
	STROKE_LINE_SCATTER_MS,
} from "./stroke-columns";

/**
 * 笔画样式：默认完全透明，数据就绪与定妆前不露形；显示以 opacity 内联
 * 定妆（定妆即纯色不透明），散开动画以 transform + opacity 全程接管。
 * 填充/描边一律纯色无 alpha——合并为整字后不保留任何透明成分，歌词页
 * （无 --dl-* 变量，走回退值）与桌面歌词（变量本身已不透明）观感一致。
 * 同色细描边适当增粗笔画（viewBox 1024 坐标系，30 单位 ≈ 字高 3.3%，
 * 屏幕上随字号等比），round join/cap 避免笔画尖端出现毛刺。
 */
const PATH_STYLE: CSSProperties = {
	fill: "var(--dl-glyph-fill-color, rgb(255 255 255))",
	stroke: "var(--dl-glyph-fill-color, rgb(255 255 255))",
	strokeWidth: 30,
	strokeLinejoin: "round",
	strokeLinecap: "round",
	opacity: 0,
	transformBox: "fill-box",
	transformOrigin: "center",
	willChange: "transform, opacity",
};

/** 无笔画数据（字母/标点/生僻字）的静态字兜底：与合并整字同为纯色不透明 */
const FALLBACK_STYLE: CSSProperties = {
	color: "var(--dl-glyph-fill-soft, rgb(255 255 255))",
	fontFamily: '"Kaiti SC", "KaiTi", "STKaiti", serif',
	userSelect: "none",
};

/**
 * 散开起始幅度（屏幕像素，与字号无关）。每笔随机分配一个深度层
 * z∈[0,1]：近层飞得近、起始更大；远层飞得远、起始更小。旋转幅度按
 * 深度递增，让每笔以明显不同的倾斜角度旋转飞出。
 */
const SCATTER_RADIUS_NEAR_PX = 120;
const SCATTER_RADIUS_FAR_PX = 480;
const SCATTER_SCALE_NEAR = 1.14;
const SCATTER_SCALE_FAR = 0.78;
const SCATTER_ROTATE_BASE_DEG = 24;
const SCATTER_ROTATE_DEPTH_DEG = 48;

/** 散开缓动：ease-in——起步蓄力加速飞出，配合短时长提升节奏感 */
const SCATTER_EASING = "cubic-bezier(.4, 0, 1, 1)";

const IDENTITY_TRANSFORM = "translate(0px, 0px) rotate(0deg) scale(1)";

/**
 * 进入动画（逐字按笔画飞入拼合）参数——与散开互为镜像的观感语言：
 * 每笔从页面边界之外的随机起点出发，带随机旋转/深度缩放/速度沿各自
 * 轨迹减速飞向定妆位；起飞顺序乱序，起点/角度/速度均不呈现统一规律。
 * 追赶容差：播放位置越过本字触发点但在容差内仍立即飞入，超出直接定妆。
 */
const STROKE_CATCHUP_INSTANT_MS = 600;
/**
 * 飞入缓动：easeOutCubic 长尾减速——起步快、速度全程连续递减、渐近
 * 落定，无分段拼接带来的速度跳变；与散开的 ease-in 起飞互为镜像。
 */
const FLY_EASING = "cubic-bezier(0.215, 0.61, 0.355, 1)";
/** 单笔飞行时长随机浮动：在句级飞行份额的 85%~100% 内（速度随机、不超窗口） */
const FLY_SPEED_JITTER_MIN = 0.85;
const FLY_SPEED_JITTER_RANGE = 0.15;
/** 起飞抖动：在相邻槽位步长内随机前移，避免等距的机械感 */
const FLY_STAGGER_JITTER_RATIO = 0.6;
/** 出页面边界后再飞的「场外」余量范围（屏幕像素） */
const FLY_OUT_MARGIN_MIN_PX = 40;
const FLY_OUT_MARGIN_MAX_PX = 200;

interface ScatterPose {
	/** 起始位移（屏幕像素，换算为用户单位后应用） */
	dx: number;
	dy: number;
	rot: number;
	sc: number;
}

/** 散开位姿：随机方向（页面任意位置）+ 按深度分层的距离/大小 */
function randomScatterPose(): ScatterPose {
	const z = Math.random(); // 0 近层 → 1 远层
	const ang = Math.random() * Math.PI * 2;
	const r =
		SCATTER_RADIUS_NEAR_PX +
		(SCATTER_RADIUS_FAR_PX - SCATTER_RADIUS_NEAR_PX) * z;
	const rotMag = SCATTER_ROTATE_BASE_DEG + SCATTER_ROTATE_DEPTH_DEG * z;
	return {
		dx: Math.cos(ang) * r,
		dy: Math.sin(ang) * r,
		rot: Math.random() * rotMag * 2 - rotMag,
		sc: SCATTER_SCALE_FAR + (SCATTER_SCALE_NEAR - SCATTER_SCALE_FAR) * (1 - z),
	};
}

interface StrokeCharCellProps {
	char: string;
	cellSize: number;
	/** 显示相位：in=进入（飞入或直接显示）；out=句尾四散飞开渐隐（父级整句驱动） */
	phase: "in" | "out";
	/** 进入方式：fly=按 triggerAt 跟播放进度逐笔飞入；instant=免飞入整句直接显示 */
	enter: "fly" | "instant";
	/** 本字的逐字触发时刻（歌词时间 ms，enter=fly 时驱动飞入时机） */
	triggerAt: number;
	/**
	 * 整句飞入的时间预算（ms）：view 层按「句窗口 − 尾部余量 − 末字触发
	 * 偏移」算好，整句所有字共享同一预算 → fitFlyTimeline 求出同一时间
	 * 线（句级统一缩放，节奏均匀）；追赶立即起飞时按已流逝量再压，保证
	 * 切下一句前完整播放完毕。
	 */
	flyBudgetMs: number;
	/**
	 * 上报本字动画的预计完成时刻（performance.now 时钟，含起飞延迟）：
	 * 父级取全句最大值，切句时等动画播完才散开——动画不被切句截断。
	 */
	onFlyEndAt?: (endAtMs: number) => void;
	/** 读取当前歌词时间（ms，含偏移）；数据就绪那一刻取一次即可 */
	getNowMs: () => number;
}

/**
 * 单字笔画格：SVG 逐笔渲染（hanzi-writer-data）。进入动画按行的免飞入
 * 判定分两路：fly——每笔从页面外随机起点带随机角度平缓飞入、汇聚拼合
 * 成字（观感与句尾散开互为镜像，时长按句级预算自适应，追赶时再压已
 * 流逝量并上报完成时刻），字的时机跟播放进度（triggerAt）走，已过点
 * 超出容差的字追赶直接定妆；instant——整句一次性以完成态呈现。定妆
 * 统一为纯色不透明（合并为整字后不保留任何透明成分）；句尾由父级等
 * 动画播完后切到 out 相位，笔画四散飞开并渐隐至完全消失。无笔画数据
 * 的字符以静态楷体直接显示、散开时同步渐隐。
 */
export function StrokeCharCell({
	char,
	cellSize,
	phase,
	enter,
	triggerAt,
	flyBudgetMs,
	onFlyEndAt,
	getNowMs,
}: StrokeCharCellProps) {
	const [strokes, setStrokes] = useState<string[] | null>(null);
	const [noData, setNoData] = useState(false);
	const cellRef = useRef<HTMLDivElement | null>(null);
	const fallbackRef = useRef<HTMLSpanElement | null>(null);
	const pathRefs = useRef<(SVGPathElement | null)[]>([]);
	const animsRef = useRef<Animation[]>([]);
	const playedRef = useRef(false);
	const scatteredRef = useRef(false);

	const cancelAnims = useCallback(() => {
		for (const a of animsRef.current) a.cancel();
		animsRef.current = [];
	}, []);

	/**
	 * 把位移（屏幕像素）夹进整个页面边界，并换算为 viewBox 用户单位：
	 * 按当前格相对页面的位置算出各方向允许的最大位移，保证笔画飞出
	 * 全程不出页面。
	 */
	const clampOffset = useCallback(
		(dx: number, dy: number): [number, number] => {
			const cell = cellRef.current;
			if (!cell) return [dx, dy];
			const cr = cell.getBoundingClientRect();
			const sr = document.documentElement.getBoundingClientRect();
			if (sr.width < 10 || sr.height < 10 || cr.width < 1) return [dx, dy];
			const cx = cr.left + cr.width / 2;
			const cy = cr.top + cr.height / 2;
			const margin = cr.width * 0.5;
			const maxX = Math.max(
				0,
				Math.min(cx - (sr.left + margin), sr.right - margin - cx),
			);
			const maxY = Math.max(
				0,
				Math.min(cy - (sr.top + margin), sr.bottom - margin - cy),
			);
			const k = 1024 / cr.width; // 屏幕像素 → viewBox 用户单位
			return [
				Math.max(-maxX, Math.min(maxX, dx)) * k,
				Math.max(-maxY, Math.min(maxY, dy)) * k,
			];
		},
		[],
	);

	/** 整句直接显示：全部笔画立即以完成态纯色定妆（不保留任何透明度） */
	const assembleInstantly = useCallback(() => {
		if (playedRef.current) return;
		playedRef.current = true;
		cancelAnims();
		for (const p of pathRefs.current) {
			if (!p) continue;
			p.style.opacity = "1";
			p.style.transform = "none";
		}
	}, [cancelAnims]);

	/**
	 * 逐笔飞入拼合（散开的镜像观感）：每笔从页面边界之外的随机方向起点
	 * 出发，携带随机旋转与深度缩放，沿各自轨迹飞向定妆位。时长结构延续
	 * 句级 FlyTimeline 统一压缩：飞行份额承担提速、tailMs 保障减速长尾
	 * 的时间余量——减速由 easeOutCubic 单段曲线全程连续承担，速度渐近
	 * 归零、无中途拼接跳变，落定丝滑无顿挫。起飞顺序乱序、方向/角度/
	 * 速度随机，不呈现统一规律。定妆与整句直接显示一致（纯色不透明），
	 * 散开可从飞行中途平滑接续。
	 */
	const flyIn = useCallback(
		(startDelayMs: number, timeline: FlyTimeline) => {
			const paths = pathRefs.current.filter((p): p is SVGPathElement =>
				Boolean(p),
			);
			playedRef.current = true;
			cancelAnims();
			if (paths.length === 0) return;

			const { scale, tailMs } = timeline;
			// 飞行份额：句级缩放后的飞行自然时长；错峰步长按笔数自适应
			//（多笔字起飞更密），整字自然窗口恒定、不随笔画数变化
			const flyShareMs = FLY_PATH_NATURAL_MS * scale;
			const step =
				paths.length > 1 ? FLY_STAGGER_SPACE_MS / (paths.length - 1) : 0;

			// 页面几何：飞入起点必须落在页面之外，位移按屏幕距离换算用户单位
			const cell = cellRef.current;
			const cr = cell?.getBoundingClientRect();
			const sr = document.documentElement.getBoundingClientRect();
			const usable =
				cr !== undefined && cr.width > 1 && sr.width > 10 && sr.height > 10;
			const k = cr !== undefined && usable ? 1024 / cr.width : 1;
			const cx = cr !== undefined ? cr.left + cr.width / 2 : 0;
			const cy = cr !== undefined ? cr.top + cr.height / 2 : 0;

			// 起飞顺序乱序（Fisher–Yates）：笔画不按笔序进场
			const order = paths.map((_, i) => i);
			for (let i = order.length - 1; i > 0; i--) {
				const j = Math.floor(Math.random() * (i + 1));
				[order[i], order[j]] = [order[j], order[i]];
			}

			const anims: Animation[] = [];
			order.forEach((strokeIndex, flySlot) => {
				const p = paths[strokeIndex];
				if (!p) return;
				// 全向随机方向 + 场外余量：起点必在页面之外（飞行覆盖全页面）
				const ang = Math.random() * Math.PI * 2;
				const dx = Math.cos(ang);
				const dy = Math.sin(ang);
				const margin =
					FLY_OUT_MARGIN_MIN_PX +
					Math.random() * (FLY_OUT_MARGIN_MAX_PX - FLY_OUT_MARGIN_MIN_PX);
				const distPx = usable
					? exitDistancePx(
							cx,
							cy,
							dx,
							dy,
							sr.left,
							sr.top,
							sr.right,
							sr.bottom,
						) + margin
					: 600 + margin;
				// 深度分层（与散开同构）：远层起始更小、旋转更大
				const z = Math.random();
				const depthScale = 0.62 + 0.26 * z;
				const rot = (Math.random() * 2 - 1) * (24 + 48 * z);
				// 起飞时刻：槽位错峰 + 抖动（自然时间线）→ 句级统一压缩
				const delayLocal =
					Math.min(
						FLY_STAGGER_SPACE_MS,
						flySlot * step + Math.random() * step * FLY_STAGGER_JITTER_RATIO,
					) * scale;
				// 飞行时长：句级飞行份额内随机浮动（速度随机、不超窗口）
				const flyMs =
					flyShareMs *
					(FLY_SPEED_JITTER_MIN + Math.random() * FLY_SPEED_JITTER_RANGE);
				// 定妆即纯色不透明：飞入只承担进场过渡，落位后不留透明成分
				const landOpacity = 1;
				// 单段连续减速：两帧 + easeOutCubic 长尾——速度全程连续递减、
				// 渐近落定，无中途帧拼接的速度跳变；tailMs 已并入总时长，
				// 为压缩场景保障减速长尾的时间余量
				anims.push(
					p.animate(
						[
							{
								transform: `translate(${(dx * distPx * k).toFixed(2)}px, ${(dy * distPx * k).toFixed(2)}px) rotate(${rot.toFixed(2)}deg) scale(${depthScale.toFixed(3)})`,
								opacity: 0,
							},
							{
								transform: IDENTITY_TRANSFORM,
								opacity: landOpacity,
							},
						],
						{
							duration: flyMs + tailMs,
							delay: startDelayMs + delayLocal,
							easing: FLY_EASING,
							fill: "both",
						},
					),
				);
			});
			// 无笔画数据的兜底静态字：跟随整字节奏淡入
			if (fallbackRef.current) {
				anims.push(
					fallbackRef.current.animate([{ opacity: 0 }, { opacity: 1 }], {
						duration: Math.max(240, 420 * scale),
						delay: startDelayMs,
						easing: FLY_EASING,
						fill: "both",
					}),
				);
			}
			if (anims.length > 0) {
				animsRef.current = anims;
				// 上报最晚动画的预计完成时刻（endTime 含起飞延迟，相对
				// animate 调用即时间轴原点）：父级据此等动画播完再切句
				let endAt = 0;
				for (const a of anims) {
					const t = a.effect?.getComputedTiming().endTime;
					if (typeof t === "number") endAt = Math.max(endAt, t);
				}
				if (endAt > 0) onFlyEndAt?.(performance.now() + endAt);
			}
		},
		[cancelAnims, onFlyEndAt],
	);

	/** 句尾散开：所有笔画向页面四周加速飞出，并逐渐透明直至完全消失 */
	const scatter = useCallback(() => {
		const paths = pathRefs.current.filter((p): p is SVGPathElement =>
			Boolean(p),
		);
		// cancel 前先读每笔当前实际位姿：定妆/散开任意状态下都从当前位置
		// 平滑接续，不跳变
		const states = paths.map((p) => {
			const cs = window.getComputedStyle(p);
			return {
				transform: cs.transform === "none" ? IDENTITY_TRANSFORM : cs.transform,
				opacity: Number.parseFloat(cs.opacity) || 0,
			};
		});
		cancelAnims();
		const anims: Animation[] = [];
		paths.forEach((p, i) => {
			const pose = randomScatterPose();
			const [ox, oy] = clampOffset(pose.dx, pose.dy);
			anims.push(
				p.animate(
					[
						{ transform: states[i].transform },
						{
							transform: `translate(${ox}px, ${oy}px) rotate(${pose.rot}deg) scale(${pose.sc})`,
						},
					],
					{
						duration: STROKE_LINE_SCATTER_MS,
						easing: SCATTER_EASING,
						fill: "both",
					},
				),
			);
			anims.push(
				p.animate([{ opacity: states[i].opacity }, { opacity: 0 }], {
					duration: STROKE_LINE_SCATTER_MS,
					easing: "linear",
					fill: "both",
				}),
			);
		});
		// 无笔画数据的兜底静态字：散开时同步渐隐
		if (fallbackRef.current) {
			anims.push(
				fallbackRef.current.animate([{ opacity: 1 }, { opacity: 0 }], {
					duration: STROKE_LINE_SCATTER_MS,
					easing: "linear",
					fill: "both",
				}),
			);
		}
		if (anims.length > 0) {
			scatteredRef.current = true;
			animsRef.current = anims;
		}
	}, [cancelAnims, clampOffset]);

	// 按字加载数据；换字时重置状态
	useEffect(() => {
		let cancelled = false;
		setStrokes(null);
		setNoData(false);
		pathRefs.current = [];
		playedRef.current = false;
		scatteredRef.current = false;
		cancelAnims();

		void loadHanziCharData(char).then((data) => {
			if (cancelled) return;
			if (data) setStrokes(data.strokes);
			else setNoData(true);
		});
		return () => {
			cancelled = true;
			cancelAnims();
		};
	}, [char, cancelAnims]);

	// 相位 out：整句四散飞开、渐隐消失
	useEffect(() => {
		if (phase === "out") scatter();
	}, [phase, scatter]);

	// 数据就绪且相位 in：进入动画（含跳回同句的复位——必须先于进入判定
	// 执行，避免 ref 复位后错过本轮 effect 导致笔画停在不可见态）
	useEffect(() => {
		if (strokes === null && !noData) return;
		if (phase !== "in") return;
		if (scatteredRef.current) {
			scatteredRef.current = false;
			playedRef.current = false;
			cancelAnims();
			for (const p of pathRefs.current) {
				if (!p) continue;
				p.style.opacity = "";
				p.style.transform = "";
			}
		}
		if (playedRef.current) return;
		if (enter === "instant") {
			assembleInstantly();
			return;
		}
		// fly：跟播放进度逐字飞入。延迟 = 触发点 − 当前歌词时间：
		// 未到点 → 到点飞入；刚过点（容差内）→ 立即飞入；早过点 → 追赶定妆
		const delayMs = triggerAt - getNowMs();
		if (delayMs < -STROKE_CATCHUP_INSTANT_MS) {
			assembleInstantly();
			return;
		}
		// 追赶立即起飞时按已流逝量再压整句时间线（数据晚到 / 渲染调度
		// 延迟都会让起飞晚于触发点），否则完成时刻会越过切句点被散开截断
		const elapsed = Math.max(0, -delayMs);
		flyIn(Math.max(0, delayMs), fitFlyTimeline(flyBudgetMs - elapsed));
	}, [
		strokes,
		noData,
		phase,
		enter,
		triggerAt,
		flyBudgetMs,
		getNowMs,
		assembleInstantly,
		flyIn,
		cancelAnims,
	]);

	return (
		<div
			ref={cellRef}
			className="relative shrink-0"
			style={{ width: cellSize, height: cellSize }}
		>
			{noData && (
				<span
					ref={fallbackRef}
					className="absolute inset-0 flex items-center justify-center"
					style={{ ...FALLBACK_STYLE, fontSize: cellSize * 0.62 }}
				>
					{char}
				</span>
			)}
			{strokes && (
				<svg
					viewBox="0 0 1024 1024"
					className="relative block h-full w-full overflow-visible"
					aria-hidden
				>
					<title>汉字笔画演示</title>
					{/* 数据坐标系 y 轴朝上，翻转铺满 viewBox（与 hanzi-writer 官方一致） */}
					<g transform="translate(0, 900) scale(1, -1)">
						{strokes.map((d, i) => (
							<path
								// biome-ignore lint/suspicious/noArrayIndexKey: 笔画列表静态不重排，索引即稳定标识
								key={`${i}-${d.slice(0, 8)}`}
								ref={(el) => {
									pathRefs.current[i] = el;
								}}
								d={d}
								style={PATH_STYLE}
							/>
						))}
					</g>
				</svg>
			)}
		</div>
	);
}
