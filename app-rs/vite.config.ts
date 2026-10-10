import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 前端：固定端口（tauri.conf.json devUrl 对齐）；构建产物出 dist/
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "es2022",
    outDir: "dist",
    rollupOptions: {
      output: {
        // echarts 占产物约 2/3 且跨版本基本不变：拆独立 chunk，发版时命中缓存；
        // react 同理。也消除 >500kB 单 chunk 告警。
        manualChunks: {
          echarts: ["echarts", "echarts-for-react"],
          react: ["react", "react-dom"],
        },
      },
    },
    // echarts 按需注册后仍有 ~540KB（折线/柱状 + 组件族），这是下限了；
    // 阈值提到 600 让告警继续盯其他 chunk 的异常膨胀
    chunkSizeWarningLimit: 600,
  },
});
