package org.tessera.mobile;

import android.app.Activity;
import android.util.Log;
import android.view.MotionEvent;
import android.view.Surface;
import android.view.ViewGroup;

import java.io.File;

import com.tungsten.fcl.game.sdl.SdlBridge;
import com.tungsten.fclauncher.bridge.FCLBridge;

/** Loads FCL's SDL hook library in the app VM, before the game JVM exists. */
public final class TesseraSdl {
    private static boolean initialized;
    private static boolean sdlLoaded;
    private static String nativeDir;

    private TesseraSdl() {}

    public static synchronized void initialize(Activity activity) {
        if (initialized) return;
        // Hook library is bundled in the APK. The game classpath points its
        // java.library.path at the same native dir, so both VMs share it.
        nativeDir = activity.getFilesDir().getAbsolutePath()
                + "/runtime/native/arm64-v8a";
        System.loadLibrary("c++_shared");
        System.loadLibrary("tessera_bridge");
        System.loadLibrary("pojavexec");
        FCLBridge.initializeHooks();
        initialized = true;
    }

    public static synchronized void loadSdl() {
        if (sdlLoaded) return;
        Log.i("TesseraSdl", "loadSdl: loading SDL3");
        // SDL must be the exact same file the game JVM loads through
        // java.library.path (the runtime pack), not the copy in the APK.
        System.load(nativeDir + "/libSDL3.so");
        Log.i("TesseraSdl", "loadSdl: SDL3 loaded");
        File sdl2 = new File(nativeDir, "libSDL2.so");
        if (sdl2.isFile()) {
            System.load(sdl2.getAbsolutePath());
        }
        sdlLoaded = true;
        Log.i("TesseraSdl", "loadSdl: done");
    }

    /**
     * Forward Android touch events to SDL as finger events.
     *
     * <p>Minecraft's own SDLEventHandler ignores finger events, but
     * TouchController's 26.3 WindowMixin consumes them from
     * Window.handleEvent.  Keep this as a real multi-pointer stream: the mod
     * needs stable per-finger IDs and normalized [0, 1] coordinates.</p>
     */
    public static void sendTouch(MotionEvent event, int width, int height) {
        if (width <= 0 || height <= 0) return;
        int action = event.getActionMasked();
        int touchDeviceId = event.getDeviceId();
        boolean move = action == MotionEvent.ACTION_MOVE;
        boolean cancel = action == MotionEvent.ACTION_CANCEL;
        int index = event.getActionIndex();

        for (int i = 0; i < event.getPointerCount(); i++) {
            if (!move && !cancel && i != index) continue;
            float nx = clamp(event.getX(i) / width);
            float ny = clamp(event.getY(i) / height);
            float pressure = Math.min(1f, Math.max(0f, event.getPressure(i)));
            int pointerId = event.getPointerId(i);
            int pointerAction = move ? MotionEvent.ACTION_MOVE
                    : cancel ? MotionEvent.ACTION_CANCEL
                    : (action == MotionEvent.ACTION_DOWN
                            || action == MotionEvent.ACTION_POINTER_DOWN)
                            ? MotionEvent.ACTION_DOWN : MotionEvent.ACTION_UP;
            org.libsdl.app.SDLActivity.onNativeTouch(
                    touchDeviceId, pointerId, pointerAction, nx, ny, pressure);
        }
    }

    private static float clamp(float value) {
        return Math.max(0f, Math.min(1f, value));
    }

    public static void prepareSurface(Activity activity, Surface surface, ViewGroup layout) {
        SdlBridge.prepareSurface(activity, surface, layout, activity);
        // The game VM's SDL_Init hook does not necessarily reach this app-side
        // CallbackBridge, so enable the SDL mouse path explicitly for the host.
        SdlBridge.setSdlEnabled(true);
    }

    public static void syncResolution(int width, int height) {
        SdlBridge.syncResolution(width, height);
    }

    public static void surfaceDestroyed() {
        SdlBridge.surfaceDestroyed();
    }

}
