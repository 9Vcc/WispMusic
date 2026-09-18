/**
 * 活跃 WebGL 画布登记表（诊断用）。
 *
 * 浏览器没有「列出所有 WebGL 上下文」的 API，而 WebGL 画布的内存大头是那块
 * **按画布尺寸分配的默认帧缓冲**（1440p 带 4×MSAA 可以到 70MB 量级），
 * 只靠数 <canvas> 元素看不出这块量。所以让 WebGL 画布在挂载时登记自己，
 * 诊断面板据此算出「画布数 × 绘制缓冲像素 × 每像素字节」的估算。
 *
 * 登记表持有的是强引用，因此必须保证它不会变成新的泄漏源，双保险：
 * 1. 组件在 effect cleanup 里注销（唯一登记点是 MeshGradient 的 Canvas）；
 * 2. 读取时顺手剔除已脱离文档的画布——即便某处忘了注销，卸载后的画布也会
 *    在下次读取时被清掉（tsconfig 的 lib 是 ES2020，没有 WeakRef 可用）。
 */
const liveCanvases = new Set<HTMLCanvasElement>();

/** 登记一个 WebGL 画布，返回注销函数（放进 effect cleanup） */
export function registerWebglCanvas(
	canvas: HTMLCanvasElement,
): () => void {
	liveCanvases.add(canvas);
	return () => {
		liveCanvases.delete(canvas);
	};
}

export interface WebglCanvasSnapshot {
	count: number;
	/** 绘制缓冲总像素数（device px，已含 dpr） */
	totalPixels: number;
	/** 每块画布的绘制缓冲尺寸，方便定位是哪一块特别大 */
	sizes: { width: number; height: number }[];
}

export function getWebglCanvasSnapshot(): WebglCanvasSnapshot {
	let totalPixels = 0;
	const sizes: { width: number; height: number }[] = [];

	for (const canvas of liveCanvases) {
		if (!canvas.isConnected) {
			liveCanvases.delete(canvas);
			continue;
		}
		totalPixels += canvas.width * canvas.height;
		sizes.push({ width: canvas.width, height: canvas.height });
	}

	return { count: sizes.length, totalPixels, sizes };
}
