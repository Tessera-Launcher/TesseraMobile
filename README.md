# Tessera Mobile

Tessera 的手机端界面。**Slint 渲染，不用 WebView。**

这一版只有骨架：底部三个页签（主页 / 下载 / 设置）可以切换，每个页面除标题外是空的。

## 结构

```text
TesseraMobile/
├── Cargo.toml          # 依赖、Android 打包元数据
├── build.rs            # 编译 ui/app.slint
├── ui/
│   ├── theme.slint     # 颜色与尺寸，唯一的字面量出处
│   ├── app.slint       # 窗口：页面区 + 底部栏，路由页签
│   ├── navbar.slint    # 底部栏、页签、三个手绘图标
│   ├── page.slint      # 三个页面共用的背景 + 标题
│   ├── home.slint
│   ├── download.slint
│   └── settings.slint
└── src/
    ├── lib.rs          # android_main / run()
    └── main.rs         # 桌面预览入口
```

## 跑起来

桌面（不用模拟器就能看布局，窗口按手机比例开）：

```sh
cargo run
```

Android（真机或模拟器接着 `adb` 就行）：

```sh
./scripts/deploy.sh          # 编译 → 安装 → 启动
```

首次在新机器上要准备：

```sh
rustup target add aarch64-linux-android
cargo install cargo-apk2
# Android SDK（build-tools + platforms）与 NDK r30+
export ANDROID_HOME=$HOME/java/android-sdk
export ANDROID_NDK_ROOT=$HOME/android-ndk-r30
```

两个容易踩的点：

- **NDK 必须 r30 或更新。** Slint 的 Android 渲染器只有 Skia 后端，用的是预编译
  Skia，链接的是 r30 的 C++ 运行时。用旧 NDK 能编过，但一启动就崩。
  （或者 `FORCE_SKIA_BUILD=1` 从源码编 Skia，非常慢。）
- **`minSdkVersion` 和 `targetSdkVersion` 都固定为 28。** 写在
  `[package.metadata.android.sdk]` 里。targetSdk 28 让应用留在旧的存储模型里
  （以后读写游戏目录不用和 scoped storage 打架），系统也不会强制边到边布局。
  cargo-apk2 会拿 `platforms;android-28` 当编译平台，所以 SDK 里要有这一份。
- **`cargo-apk2` 的元数据键名和 `cargo-apk` 不一样。** 现在用的是
  `[package.metadata.android.sdk]` 下的 `min_sdk_version` / `target_sdk_version`，
  以及 `[package.metadata.android.application]` 下的 `label`。写成
  `min-sdk` / `label`（顶格）会被静默忽略。

发布签名（debug 会自己生成 `debug.keystore`）：

```sh
export CARGO_APK_RELEASE_KEYSTORE=/path/to/key.jks
export CARGO_APK_RELEASE_KEYSTORE_PASSWORD=...
cargo apk2 build --release --target aarch64-linux-android --lib
```

## 真机截图

ZTE 7552N / Android 13 / 480×854：

| 主页 | 下载 | 设置 |
|---|---|---|
| ![主页](docs/phone-home.png) | ![下载](docs/phone-download.png) | ![设置](docs/phone-settings.png) |

## 几个刻意的决定

- **不用 WebView。** 全部 Slint 原生绘制。
- **页面先空着。** 只有标题，用来确认页签切换。
- **底部栏是手绘的**，没有用 `std-widgets`：它是所有页面共用的那一块，
  等设计 token 接进来时要保持现在的形状。
- **位置全部显式给 x/y。** Slint 对只声明了大小的子元素会做居中摆放，
  页签栏需要的是精确坐标，所以没有依赖布局推断。
- **三等分用 `root.width / 3` 而不是 `horizontal-stretch`**：后者在这里没有生效。
- **字体用系统默认。** 桌面版那套像素字体不复刻，手机上也不需要。
- **安全区**：`Window.safe-area-insets` 从窗口传给页面（顶部）和底栏（底部），
  所以刘海和手势条不会盖住内容。桌面上这两个值都是 0。
  实测那台 ZTE 上顶部 inset 正常（约 24dp），**底部报 0**（手势导航模式），
  底栏因此贴到屏幕最下沿。如果后面发现手势区抢点击，再补一个最小底部留白。
- **窗口尺寸不在 `.slint` 里写死。** 写在 `Window` 上的 `width/height` 在 Android
  上会变成一个比屏幕还高的窗口，底栏直接被挤到屏幕外——第一版就是这么翻车的。
  现在只有桌面入口调 `set_size()`。

## 下一步

页面内容接 `tessera-core` 时，只需要复用它的 `Effect` 队列：点击产生意图，
后台线程执行，报告回主线程。桌面版的状态机（`app::App`）是围绕鼠标和窗口写的，
手机端不复用它，只复用领域逻辑。
