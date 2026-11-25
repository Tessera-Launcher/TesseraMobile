package org.lwjgl.glfw;

import android.view.MotionEvent;

/**
 * App-VM side of the SDL touch/menu bridge.
 *
 * Tessera no longer uses Pojav's GLFW input queue.  The game receives SDL
 * finger/mouse events directly; only the cross-VM cursor-grab flag and the
 * absolute menu pointer are kept here.
 */
public final class CallbackBridge {
    public static volatile int windowWidth;
    public static volatile int windowHeight;

    private static final int GLFW_MOUSE_LEFT = 0;
    private static final int GLFW_MOUSE_RIGHT = 1;
    private static final int GLFW_MOUSE_MIDDLE = 2;

    private static float mouseX;
    private static float mouseY;
    private static int sMouseButtonState;

    static {
        // Shared with the game VM; see android/bridge-shim.
        System.loadLibrary("tessera_bridge");
    }

    private CallbackBridge() {}

    /** Cross-VM cursor-grab state, set by SDL_SetWindowRelativeMouseMode. */
    public static native boolean nativeIsGrabbing();

    public static boolean isGrabbing() {
        return nativeIsGrabbing();
    }

    /** Move the virtual pointer. In-world motion is sent as a relative SDL delta. */
    public static void sendCursorPos(float x, float y) {
        float dx = x - mouseX;
        float dy = y - mouseY;
        mouseX = x;
        mouseY = y;
        if (isGrabbing()) {
            org.libsdl.app.SDLActivity.onNativeMouse(
                    0, MotionEvent.ACTION_MOVE, dx, dy, true);
        } else {
            org.libsdl.app.SDLActivity.onNativeMouse(
                    0, MotionEvent.ACTION_MOVE, clampX(x), clampY(y), false);
        }
    }

    /** GLFW button index: 0 left, 1 right, 2 middle. */
    public static void sendMouseButton(int button, boolean down) {
        int sdlButton = 0;
        if (button == GLFW_MOUSE_LEFT) sdlButton = MotionEvent.BUTTON_PRIMARY;
        else if (button == GLFW_MOUSE_RIGHT) sdlButton = MotionEvent.BUTTON_SECONDARY;
        else if (button == GLFW_MOUSE_MIDDLE) sdlButton = MotionEvent.BUTTON_TERTIARY;

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
