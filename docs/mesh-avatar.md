# 主窗口 Mesh 头像

主窗口左侧 76×76 的圆形头像默认显示 2D 网格头像 **xingyin**，由 [mesh-avatar-studio](https://github.com/shinshin86/mesh-avatar-studio) 制作，用 `mesh-avatar-element` 的 `<mesh-avatar>` 自定义元素渲染。圆里只显示脸部特写。头像待机时会呼吸、眨眼、轻微晃动，并随对话状态和回复情绪变化。

设计文档：`.auto-work/docs/plans/design-20261028-145935.md`(主窗口头像)、`.auto-work/docs/plans/design-20261028-155955.md`(设置页)

渲染逻辑在 `src/lib/components/MeshFace.svelte`，主窗口的 `AvatarIcon.svelte` 和设置页预览共用。

## 设置页

设置 → 头像 → 「动画头像(mesh)」卡片。除了头像文件，其他项和别的设置一样，点「保存」后才生效，主窗口收到 `settings-changed` 后更新。

| 设置项 | 字段 | 默认值 | 范围 |
|---|---|---|---|
| 开关 | `mesh_avatar_enabled` | 开 | — |
| 缩放 | `mesh_avatar_scale` | 1.0 | 0.5–2.0(以脸中心为基准缩放) |
| 水平偏移 | `mesh_avatar_offset_x` | 0 | -40–40 px |
| 垂直偏移 | `mesh_avatar_offset_y` | 0 | -40–40 px |
| 各状态的表情 | `mesh_avatar_expressions` | 见「状态联动」 | 键为 4 个状态；值为 `neutral smile shy surprise halfLidded angry sad wink` 之一 |
| 回复情绪对应(「高级」区域) | `mesh_avatar_emotions` | 见「回复情绪」 | 键为 8 个 PA emotion；值为 `happy sad angry surprised relaxed neutral` 之一 |

- 开关打开时，mesh 头像会覆盖下面的「静态头像 / 动画头像」;关掉后，主窗口改用这些回退头像(见「回退」)。
- 预览圆和主窗口用同一个组件，拖动滑块后预览立刻变化;点状态按钮可以预览各状态的表情和口型。「重置画面」把缩放和偏移恢复为默认值。
- 所有范围和白名单由 `settings_repository::validate` 校验，越界、未知值或多余的键都会被拒绝。旧设置库没有这些字段时用默认值，不需要迁移。

### 自定义头像文件

「选择 .mavatar」上传一个自己的头像，「恢复内置」删除它，回到 xingyin。

- 文件存在设置数据库的 `assets` 表(key `mesh_avatar`，MIME `application/octet-stream`)，**不放进 `AppConfig`**,以免每次 `get_config` 都传几 MB 的 base64。主窗口在启动和每次收到 `settings-changed` 时用 `get_setting_asset` 读取，只有内容变化才重新加载。
- 存储层只允许 `mesh_avatar` 使用 octet-stream，图片 key 不能用;数据必须以 zip 文件头 `PK\x03\x04` 开头;上限 10 MB。设置页选择文件时也会先检查扩展名、大小和文件头，不符合的当场提示。
- 自定义文件通过 `meshEl.load(Uint8Array)` 加载，`src` 始终保留内置路径 `/avatars/xingyin.mavatar`,所以不会去网上下载示例头像。恢复内置时组件会重建 `<mesh-avatar>` 元素，重新加载 `src`。
- 自定义文件加载失败时，同样回退到静态图、GIF 或 CSS 脸;重新选择文件或恢复内置后会再次尝试。

## 状态联动

默认对应如下，可在设置页修改表情(`mesh_avatar_expressions`):

| 窗口状态 (`characterState`) | 头像表现 |
|---|---|
| `idle` | 默认待机（运行时自带的呼吸、眨眼），情绪清空 |
| `listening` | 表情 `surprise`（睁大眼，表示在听） |
| `thinking` | 表情 `halfLidded`（半闭眼） |
| `speaking` | 张嘴说话：前端用模拟音量驱动口型（约 6 Hz 起伏、偶尔停顿），**和真实语音不同步** |

离开 `speaking`、被 barge-in 打断、组件卸载时，口型循环都会停止，音量归零，嘴巴闭上。barge-in 时 `App.svelte` 通过 `interrupted` prop 通知 `AvatarIcon`，并清空回复情绪：此时 `characterState` 可能还停在 `speaking`（要等后端 Ready 后才切到 `listening`），但嘴巴会立刻停下。

WebGL 上下文丢失后恢复时，元素会换一个新的头像实例。组件在每次 `load` 事件里把当前的情绪和说话状态重新设置到新实例上，所以正在说话时恢复，嘴巴会继续动。

## 回复情绪

`App.svelte` 从 `chat-speaking-start` 和 `api-push` 事件里取 `emotion`，通过 `emotion` prop 传给 `AvatarIcon`。`chat-audio-done` 时清空，回到 `idle` 时也会清空。默认对应如下，可在设置页「高级」区域修改(`mesh_avatar_emotions`):

| PA emotion | 头像情绪 |
|---|---|
| cheerful、excited、friendly | `happy` |
| sad | `sad` |
| angry | `angry` |
| calm、whisper | `relaxed` |
| serious | `neutral` |
| 其他未知值(不可配置) | `neutral` |

切换情绪时不播放额外的动作（`playMotion: false`）。

## 回退

设置页关掉 mesh 开关，或满足以下任一情况时，头像会回退到原来的显示方式，优先级依次为：按状态设置的图片或视频皮肤 → 「Avatar image」→ CSS 脸。

- `public/avatars/xingyin.mavatar`(或自定义文件)不存在或无法读取
- 系统不支持 WebGL2
- WebView 缺少 `Array.prototype.at`、`Object.hasOwn` 或 `structuredClone`。依赖直接调用这三个 API，要 Safari 15.4 起才有。app 最低支持 macOS 12.0，更早的 WKWebView 可能没有，且项目没有为它们提供 polyfill。组件在创建 `<mesh-avatar>` 之前先检测，缺任何一个就直接用回退头像，不会创建元素，也不打印警告

加载失败时控制台只会打印一行 `[avatar] mesh avatar unavailable, using fallback skin`，不会出现未处理的错误。头像加载完成前，圆里先显示回退头像，所以不会出现空白。

## 默认值在哪里（开发者）

- **默认设置值**：Rust `src-tauri/src/commands/config.rs`(`AppConfig::default`、`default_mesh_avatar_expressions`、`default_mesh_avatar_emotions`)和 TS `src/lib/stores/settings.ts`(`SETTINGS_DEFAULTS`)两边要保持一致;白名单在 `settings_repository.rs` 和 `settings.ts` 里各有一份。
- **内置头像文件**：`MeshFace.svelte` 的常量 `MESH_SRC`。只用同源的 `/avatars/...` 路径。`<mesh-avatar>` 在 `src` 为空时会去作者网站下载示例头像，所以 `src` 不能留空。
- **基准画面**：`MeshFace.svelte` 里缩放 1、偏移 0 时为 `96px` × `96px`,`left: -12px`、`top: -10px`,即原图 1600px 缩放 0.06，脸中心落在圆心下方 6px。设置页的缩放和偏移都叠加在这个基准上。

`.mavatar` 文件可以用 mesh-avatar-studio 的 `npm run pack-avatar -- projects/<name> --no-source` 生成。加上 `--no-source` 后，文件里不含原图，只能播放。

## 性能与安全

- 元素在屏幕外或窗口隐藏时会自动暂停渲染，模拟口型的 `requestAnimationFrame` 在页面隐藏时也会停止。
- 画布像素等于显示尺寸乘以 devicePixelRatio，运行时上限是 2。
- 依赖精确锁定在 `mesh-avatar-element@0.3.0`，会一起装上 `mesh-avatar@0.4.0` 和 `fflate@0.8.3`。三个包都是 MIT 许可证，都没有安装脚本。
- 头像文件随 app 一起打包，只从本地加载，**运行时不发任何网络请求**。
- 运行时会用 `blob:` URL 加载图层图片。目前 CSP 为 `null`；以后如果收紧 CSP，`img-src` 需要放行 `blob:`。
- `emotion` 可能来自外部推送（`api-push`），所以对应表用 `Map` 查找，未知值一律当作 `neutral`。
- 自定义 `.mavatar` 是不可信的二进制输入：只能由用户在设置页从本地选择，存储层校验 key、MIME、zip 文件头和大小(见「自定义头像文件」)。

## 以后可能做

- 管理多个自定义头像，或从内置头像库里选择。
- 由 Rust 在播放音频时发送真实音量事件（例如 `chat-audio-level`），替代模拟口型。
