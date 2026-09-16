import { createApp } from "vue";
import App from "./App.vue";
import { paintStoredTheme } from "./appearance";
// 在挂载之前加载：它从 localStorage 定下语言，第一帧就是对的语言。
import "./i18n";
import "./style.css";

// 挂载之前先上色：等设置读回来再上，深色用户每次启动都会被白屏闪一下。
paintStoredTheme();

createApp(App).mount("#app");
