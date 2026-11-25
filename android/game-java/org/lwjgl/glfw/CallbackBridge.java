package org.lwjgl.glfw;

/**
 * Game-VM side of the small Tessera bridge.
 *
 * SDLInit calls nativeNotifyLauncher after SDL_Init; the native SDL backend is
 * already prepared by the launcher VM, so this is intentionally a no-op.
 * SDLMouse calls nativeSetGrabbing to publish the cursor state to the app VM.
 */
public final class CallbackBridge {
    public static final int SDL = 0;
    public static final int INIT = 0;

    static {
        System.loadLibrary("tessera_bridge");
    }

    private CallbackBridge() {}

    public static native void nativeSetGrabbing(boolean grabbing);

    public static boolean nativeNotifyLauncher(int type, int... action) {
        return false;
    }
}
