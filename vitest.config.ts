import { defineConfig } from "vitest/config";

export default defineConfig({
	test: {
		// 仓库的 src-tauri/target 里有 vendored 的 unm-server 构建产物，
		// 自带大量 tap/ava 体系的 *.test.js，vitest 无法运行它们；
		// 测试发现范围限定在 src/（纯前端单测）
		include: ["src/**/*.{test,spec}.{ts,tsx}"],
	},
});
