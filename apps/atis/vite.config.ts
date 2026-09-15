import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";

// 4330 起，接在网站那排 432x 后面（4321 can-web … 4327 can-ui）。
// 桌面端各占一个：4330 controller、4331 atis、4332 xpc、4333 msfs。
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  // Tauri 自己看这些输出，dev 时它连的是下面这个端口。
  clearScreen: false,
  server: { port: 4331, strictPort: true },
  build: { target: "es2022", emptyOutDir: true },
});
