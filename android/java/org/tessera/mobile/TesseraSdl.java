package org.tessera.mobile;

import android.app.Activity;
import android.view.Surface;
import android.view.ViewGroup;

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
        System.loadLibrary("pojavexec");
        FCLBridge.initializeHooks();
        initialized = true;
    }

    public static synchronized void loadSdl() {
        if (sdlLoaded) return;
        // SDL must be the exact same file the game JVM loads through
        // java.library.path (the runtime pack), not the copy in the APK.
        System.load(nativeDir + "/libSDL3.so");
        System.load(nativeDir + "/libSDL2.so");
        sdlLoaded = true;
    }

    public static void prepareSurface(Activity activity, Surface surface, ViewGroup layout) {
        SdlBridge.prepareSurface(activity, surface, layout, activity);
    }

    public static void syncResolution(int width, int height) {
        SdlBridge.syncResolution(width, height);
    }

    public static void surfaceDestroyed() {
        SdlBridge.surfaceDestroyed();
    }

}
