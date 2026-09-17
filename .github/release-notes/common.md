## Windows 会弹「已保护你的电脑」

安装包**没有数字签名**，所以 Windows 会弹蓝色的"已保护你的电脑"：点「更多信息」→
「仍要运行」。这是因为没有签名，不是杀毒软件误报。现在的 can-audio 也是这样。

## 装哪一个

| 文件 | 给谁 |
|---|---|
| `audio-for-can` | 管制员的语音客户端 |
| `atis-for-can` | 做通播的 |
| `xpc-for-can` | X-Plane 飞行员 |
| `msfs-for-can` | MSFS 飞行员 |

**Windows**：`.exe` 是安装程序，`.msi` 是给有组策略的机器用的，
装一个就行。

**Linux**：`.deb` 给 Debian / Ubuntu，`.rpm` 给 Fedora / openSUSE，
`.AppImage` 给别的发行版（下载后 `chmod +x` 直接跑）。

> **AppImage 有 77 MB，deb 和 rpm 只有 5 MB。** 差别是 AppImage 把
> 整个 webkit 运行时打了进去；deb/rpm 用系统自己的。**发行版对得上
> 就装 deb 或 rpm**，不要因为 AppImage 看起来省事就下那个。

> **Linux 上的 PTT 只在 X11 下有效。** 全局按键监听走的是 Xlib，
> 而 Wayland 不允许一个普通程序监听全局按键——在 Wayland 会话里
> 键盘 PTT 不会响应，手柄 PTT 照常。界面上那个「按住发话」按钮
> 任何时候都能用。

还没有 macOS 版，缺的是签名和公证那一套。

## 已知还不行的

- **MSFS 的他机用的是默认机模。** AI 机注入已经接上了，但机型对到
  MSFS 机模标题这一步只认得第一方（Asobo）那十几种——别的机型会顺着
  同族、同类机身退化，最后退到 C172。装了 FSLTL / AIG 这类 AI 机模包
  的人可以自己写一张表放在
  `%LOCALAPPDATA%\msfs-for-can\titles.json`，格式是
  `{"A20N": "你的机模标题"}` 或者 `{"A20N": ["首选", "备选"]}`。
- **X-Plane 要单独装插件**才看得见他机（需要先装
  [XPPython3](https://xppython3.readthedocs.io/)）：把
  `PI_XpcTraffic.py` 放进 X-Plane 目录的 `Resources/plugins/PythonPlugins/`。
  不装也能连上、能说话，只是天上是空的。
  **装好的 xpc-for-can 自己就带着一份**：Windows 在安装目录的
  `PythonPlugins\` 里，deb / rpm 在 `/usr/lib/xpc-for-can/PythonPlugins/`。
  客户端里「本机 → X-Plane 他机插件」装不进去时（比如 X-Plane 在要
  管理员权限的目录里），界面会给出这一份的确切位置。
  也可以用下面的 `xpc-for-can-xplane-plugin.zip`，解压后把
  `PythonPlugins/` 整个放进 `Resources/plugins/` 下面——用 AppImage 的
  选这个，AppImage 里那份只在程序开着时才看得到。
- **通播不出声**：声音由服务端的机队播，客户端这边只做稿子。
