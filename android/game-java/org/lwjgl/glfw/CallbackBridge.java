package org.lwjgl.glfw;

/** Game-VM side of the FCL input bridge. libpojavexec owns the native methods. */
public final class CallbackBridge {
    // SDLInit passes these to nativeNotifyLauncher.
    public static final int SDL = 0;
    public static final int INIT = 0;

    public static volatile int windowWidth;
    public static volatile int windowHeight;
    public static volatile int physicalWidth;
    public static volatile int physicalHeight;
    public static float mouseX;
    public static float mouseY;
    public static float deltaX;
    public static float deltaY;
    public static volatile boolean holdingAlt;
    public static volatile boolean holdingCapslock;
    public static volatile boolean holdingCtrl;
    public static volatile boolean holdingNumlock;
    public static volatile boolean holdingShift;
    public static boolean sGamepadDirectInput;

    static {
        // Only the shared grab flag is needed from the game VM now.
        System.loadLibrary("tessera_bridge");
    }

    private CallbackBridge() {}

    public static native void nativeSetUseInputStackQueue(boolean use);
    private static native boolean nativeSendChar(char ch);
    private static native boolean nativeSendCharMods(char ch, int mods);
    private static native void nativeSendKey(int key, int scancode, int action, int mods);
    private static native void nativeSendCursorPos(float x, float y);
    private static native void nativeSendMouseButton(int button, int action, int mods);
    private static native void nativeResetInputState();
    private static native void nativeSendScroll(double x, double y);
    private static native void nativeSendScreenSize(int width, int height);
    public static native void nativeSetWindowAttrib(int attrib, int value);
    public static native void setupBridgeWindow(Object surface);
    public static native void nativeSetGrabbing(boolean grabbing);
    public static native int getFps();

    public static boolean nativeNotifyLauncher(int type, int... action) {
        return notifyLauncher(type, action);
    }

    public static boolean notifyLauncher(int type, int... action) {
        return false;
    }

    public static String accessAndroidClipboard(int type, String text) { return null; }
    public static void onGrabStateChanged(boolean grabbing) {}
    public static void onDirectInputEnable() {}
}
