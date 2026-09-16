import { createApp } from "vue";
import App from "./App.vue";
import { paintStoredTheme } from "./appearance";
import "./style.css";

// 挂载之前先上色：等设置读回来再上，深色用户每次启动都会被白屏闪一下。
paintStoredTheme();

createApp(App).mount("#app");
