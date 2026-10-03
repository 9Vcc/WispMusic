import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
	// 源码统一用 `@/` 别名（见 tsconfig paths）。vitest.config.ts 会整体顶替
	// vite.config.ts，所以别名必须在这里再声明一次，否则被测模块里的 `@/` 导入
	// 会在收集阶段报 Cannot find package。
	resolve: {
		alias: {
			"@": fileURLToPath(new URL("./src", import.meta.url)),
		},
	},
	test: {
		// 仓库的 src-tauri/target 里有 vendored 的 unm-server 构建产物，
		// 自带大量 tap/ava 体系的 *.test.js，vitest 无法运行它们；
		// 测试发现范围限定在 src/（纯前端单测）
		include: ["src/**/*.{test,spec}.{ts,tsx}"],
	},
});
