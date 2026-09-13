import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tsconfigPaths from "vite-tsconfig-paths";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tsconfigPaths()],

  // vendored CJS 包（ncm-embedded）中残留的 process.env 引用兜底
  define: {
    "process.env": {},
  },

  // workspace 链接的 CJS 包默认不参与 dev 预打包，浏览器会因 require 未定义而崩溃，
  // 强制交给 esbuild 预打包以完成 CJS → ESM 转换；
  // entries 限定为根入口：Vite 默认递归扫描 **/*.html，
  // 会把仓库内任何额外的 html（例如临时放进来的参考项目）当成入口，
  // 因其依赖未安装而使预打包失败
  optimizeDeps: {
    entries: ["index.html"],
    include: ["ncm-embedded/browser"],
    // 不等待启动依赖爬取结束才服务请求：warm cache 下立即按缓存元数据返回，
    // 爬取在后台进行（发现新依赖才会触发一次 full-reload）
    holdUntilCrawlEnd: false,
  },

  build: {
    // vendored CJS 包位于 packages/（非 node_modules），需显式纳入 commonjs 转换
    commonjsOptions: {
      include: [/node_modules/, /packages[\\/]ncm-embedded/],
    },
    // WebView2 是常青 Chromium，无需为旧浏览器降级转译
    target: "chrome120",
    rollupOptions: {
      output: {
        manualChunks(id: string) {
          if (!id.includes("node_modules")) return;
          if (/[\\/]node_modules[\\/](three|@react-three)[\\/]/.test(id))
            return "three";
          if (/[\\/]node_modules[\\/](framer-motion|motion)[\\/]/.test(id))
            return "motion";
          if (
            /[\\/]node_modules[\\/](react|react-dom|react-router|react-router-dom|scheduler|@remix-run)[\\/]/.test(
              id,
            )
          )
            return "react";
        },
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      // .workbuddy 是 WorkBuddy 的项目数据目录（调试探针页、无头浏览器 profile
      // 等临时文件都写在这里）：被 watcher 盯上后，无头 Edge 持有的 Cookies 等
      // 锁定文件会让 chokidar 直接抛 EBUSY，且该错误未捕获会杀掉整个 dev 进程
      ignored: [
        "**/src-tauri/**",
        "**/.pnpm-store-tmp/**",
        "**/.workbuddy/**",
      ],
    },
  },
}));
