package org.lwjgl.glfw;

import com.tungsten.fcl.game.sdl.SdlBridge;
import org.tessera.mobile.TesseraSdl;

/**
 * The app-side bridge libpojavexec calls when the game VM hits SDL_Init.
 *
 * <p>Only the SDL callback is implemented here.  The game VM gets a separate
 * copy with the native declarations in fcl-callbackbridge.jar.</p>
 */
public final class CallbackBridge {
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

    public static final int NOTIF_TYPE_SDL = 0;
    public static final int ACTION_INIT_LAUNCHER_INTEGRATION = 0;
    public static final int ACTION_SEND_TEXTBOX_RECT = 1;

    private CallbackBridge() {}

    // libpojavexec registers these through RegisterNatives.  It logs
    // "CriticalNative is not available" on current GrapheneOS, so they are
    // ordinary JNI entry points here.
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

    /** Called from libpojavexec when the game VM calls SDL_InitSubSystem. */
    public static boolean notifyLauncher(int type, int... action) {
        if (action == null || action.length == 0) return false;
        if (type != NOTIF_TYPE_SDL || action[0] != ACTION_INIT_LAUNCHER_INTEGRATION) {
            return false;
        }
        if (!SdlBridge.markSdlInitialized()) return true;
        try {
            TesseraSdl.loadSdl();
            SdlBridge.setupJNI();
            SdlBridge.setSdlEnabled(true);
            org.libsdl.app.SDLSurface surface = org.libsdl.app.SDLActivity.getSDLSurface();
            if (surface != null) {
                surface.surfaceCreated(null);
                surface.surfaceChanged();
                if (windowWidth > 0 && windowHeight > 0) {
                    surface.nativeResize(windowWidth, windowHeight);
                }
            }
            return true;
        } catch (Throwable problem) {
            android.util.Log.e("TesseraSDL", "SDL launcher integration failed", problem);
            SdlBridge.setSdlEnabled(false);
            SdlBridge.clearSdlInitialized();
            return false;
        }
    }

    /** Game VM talks to libpojavexec through this native method. */
    public static native boolean nativeNotifyLauncher(int type, int... action);

    public static String accessAndroidClipboard(int type, String text) { return null; }

    /**
     * libpojavexec calls this when the game changes cursor mode.  Tessera is
     * touch-only: the state is deliberately not translated into a synthetic
     * mouse; TouchController owns all touch-to-game input.
     */
    public static void onGrabStateChanged(boolean grabbing) {}

    public static void onDirectInputEnable() {}
}
