package org.lwjgl.glfw;

import android.view.MotionEvent;

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

    /** Game cursor mode: true while the game owns the pointer (in-world look). */
    private static volatile boolean isGrabbing;
    /** SDL mouse button mask (MotionEvent.BUTTON_*), not GLFW button index. */
    private static int sMouseButtonState;

    private static final int GLFW_MOUSE_LEFT = 0;
    private static final int GLFW_MOUSE_RIGHT = 1;
    private static final int GLFW_MOUSE_MIDDLE = 2;

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
     * Called by libpojavexec on the Android UI thread when the game changes
     * cursor mode.  In-world (grabbed) touches become relative look deltas;
     * menus (not grabbed) use an absolute virtual cursor.
     */
    public static void onGrabStateChanged(boolean grabbing) {
        isGrabbing = grabbing;
        deltaX = 0f;
        deltaY = 0f;
    }

    public static void onDirectInputEnable() {}

    public static boolean isGrabbing() {
        return isGrabbing;
    }

    /**
     * Move the virtual pointer.  Cursor mode sends absolute window pixels;
     * grabbed mode sends the same position to GLFW and a relative delta to
     * SDL, because SDL's Android mouse backend treats absolute motion while
     * grabbed as a mouse jump.
     */
    public static void sendCursorPos(float x, float y) {
        float dx = x - mouseX;
        float dy = y - mouseY;
        mouseX = x;
        mouseY = y;
        deltaX = dx;
        deltaY = dy;
        nativeSendCursorPos(x, y);
        if (isGrabbing) {
            org.libsdl.app.SDLActivity.onNativeMouse(0, MotionEvent.ACTION_MOVE, dx, dy, true);
        } else {
            org.libsdl.app.SDLActivity.onNativeMouse(0, MotionEvent.ACTION_MOVE,
                    clampX(x), clampY(y), false);
        }
    }

    /** Relative look delta (finger drag in-world). */
    public static void sendCursorDelta(float x, float y) {
        deltaX = x;
        deltaY = y;
        mouseX += x;
        mouseY += y;
        nativeSendCursorPos(mouseX, mouseY);
        org.libsdl.app.SDLActivity.onNativeMouse(0, MotionEvent.ACTION_MOVE, x, y, true);
    }

    /** GLFW button index: 0 left, 1 right, 2 middle. */
    public static void sendMouseButton(int button, boolean down) {
        int sdlButton = 0;
        if (button == GLFW_MOUSE_LEFT) sdlButton = MotionEvent.BUTTON_PRIMARY;
        else if (button == GLFW_MOUSE_RIGHT) sdlButton = MotionEvent.BUTTON_SECONDARY;
        else if (button == GLFW_MOUSE_MIDDLE) sdlButton = MotionEvent.BUTTON_TERTIARY;

        nativeSendMouseButton(button, down ? 1 : 0, 0);
        if (sdlButton == 0) return;

        if (down) sMouseButtonState |= sdlButton;
        else sMouseButtonState &= ~sdlButton;
        org.libsdl.app.SDLActivity.onNativeMouse(
                sMouseButtonState,
                down ? MotionEvent.ACTION_DOWN : MotionEvent.ACTION_UP,
                clampX(mouseX), clampY(mouseY), false);
    }

    private static float clampX(float x) {
        int width = windowWidth;
        return width > 0 ? Math.max(0f, Math.min(width - 1f, x)) : x;
    }

    private static float clampY(float y) {
        int height = windowHeight;
        return height > 0 ? Math.max(0f, Math.min(height - 1f, y)) : y;
    }
}
