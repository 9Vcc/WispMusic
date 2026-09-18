import { describe, expect, it } from "vitest";
import {
	byteLengthUtf8,
	estimateBitmapBytes,
	estimateFramebufferBytes,
	formatBytes,
	measureJsonBytes,
} from "./memory-units";

describe("formatBytes", () => {
	it("按量级切换单位", () => {
		expect(formatBytes(0)).toBe("0 B");
		expect(formatBytes(512)).toBe("512 B");
		expect(formatBytes(1024)).toBe("1.0 KB");
		expect(formatBytes(1024 * 1024)).toBe("1.0 MB");
		expect(formatBytes(1536 * 1024)).toBe("1.5 MB");
	});

	it("大数值不再保留小数，避免读数变噪音", () => {
		expect(formatBytes(256 * 1024 * 1024)).toBe("256 MB");
		expect(formatBytes(1024 * 1024 * 1024)).toBe("1.0 GB");
	});

	it("非法输入不抛错", () => {
		expect(formatBytes(Number.NaN)).toBe("0 B");
		expect(formatBytes(-1)).toBe("0 B");
		expect(formatBytes(Number.POSITIVE_INFINITY)).toBe("0 B");
	});
});

describe("estimateBitmapBytes", () => {
	it("按 RGBA 每像素 4 字节估算", () => {
		expect(estimateBitmapBytes(100, 100)).toBe(40_000);
		expect(estimateBitmapBytes(1300, 1300)).toBe(6_760_000);
	});

	it("未加载完成的图（尺寸为 0）不计入", () => {
		expect(estimateBitmapBytes(0, 0)).toBe(0);
	});

	it("非整数与非法值被安全处理", () => {
		// 宽高各自四舍五入：10.4 → 10、10.6 → 11
		expect(estimateBitmapBytes(10.4, 10.6)).toBe(10 * 11 * 4);
		expect(estimateBitmapBytes(Number.NaN, 100)).toBe(0);
	});
});

describe("estimateFramebufferBytes", () => {
	it("给出单份 color 附件到 4×MSAA 的区间", () => {
		const pixels = 2560 * 1440;
		expect(estimateFramebufferBytes(pixels)).toEqual({
			min: pixels * 4,
			max: pixels * 16,
		});
	});

	it("无画布时为 0", () => {
		expect(estimateFramebufferBytes(0)).toEqual({ min: 0, max: 0 });
		expect(estimateFramebufferBytes(Number.NaN)).toEqual({
			min: 0,
			max: 0,
		});
	});
});

describe("measureJsonBytes", () => {
	it("按 UTF-8 字节数计量（中文 3 字节）", () => {
		expect(measureJsonBytes("abc")).toBe(5); // 含引号
		expect(measureJsonBytes("中")).toBe(5); // "中" = 2 引号 + 3 字节
		expect(byteLengthUtf8("中")).toBe(3);
	});

	it("空值算 0", () => {
		expect(measureJsonBytes(null)).toBe(0);
		expect(measureJsonBytes(undefined)).toBe(0);
	});

	it("循环引用不会抛错（归因面板不能因此崩掉）", () => {
		const cyclic: Record<string, unknown> = { name: "loop" };
		cyclic.self = cyclic;
		expect(measureJsonBytes(cyclic)).toBe(0);
	});
});
