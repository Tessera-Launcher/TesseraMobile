# Tessera Mobile

**胆大心细、敢想敢做的 Minecraft: Java Edition 启动器，Android 前端。**

Slint 原生绘制，不使用 WebView。面向支持 Vulkan 的 Android 设备，不提供 OpenGL 回退。

## 构建

桌面预览：

```sh
cargo run
```

Android：

```sh
rustup target add aarch64-linux-android
cargo install cargo-apk2

export ANDROID_HOME=$HOME/java/android-sdk
export ANDROID_NDK_ROOT=$HOME/android-ndk-r30
export ANDROID_NDK_HOME=$ANDROID_NDK_ROOT
export PATH="$ANDROID_HOME/build-tools/34.0.0:$PATH"

cargo apk2 build --target aarch64-linux-android --lib
adb install -r target/debug/apk/tessera.apk
adb shell am force-stop org.tessera.mobile
adb shell am start -n org.tessera.mobile/android.app.NativeActivity
```

需要 **NDK r30 或更新**（Slint 的预编译 Skia 依赖 r30 的 C++ 运行时）。构建 APK 时
建议把 `JAVA_HOME` 指向 JDK 21；当前 d8 不支持 JDK 28 产出的 class 文件版本。

## 运行库

Android 端的 LWJGL、SDL 和 OpenAL 暂用 FCL 的 Android 构建，由应用内的 aria2c
下载到私有目录，不写入 `/sdcard`。

Mojang 客户端 jar 的 Vulkan-only 补丁在安装时写入，不需要外部脚本或 `javac`。

## 许可

GPL-3.0-or-later。其中 `vendor/mdt-minecraft` 为 MIT，
`vendor/minecraft-auth` 为 LGPL-3.0-or-later；FCL 运行库以及 SDL、LWJGL、OpenAL
遵循各自的许可。
