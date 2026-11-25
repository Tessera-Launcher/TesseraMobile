package org.tessera.mobile;

import android.app.Activity;
import android.view.Surface;
import android.view.ViewGroup;

import org.libsdl.app.SDL;
import org.libsdl.app.SDLActivity;
import org.libsdl.app.SDLSurface;

/**
 * The launcher half of FCL's SDL integration.
 *
 * <p>SDL3's Android backend is allowed to ask its Java host to set up JNI and
 * bind a Surface.  This class keeps that host tiny: game logic and input stay
 * in Rust/Tessera.</p>
 */
public final class SdlBridge {
    private static boolean sdlEnabled;
    private static boolean jniReady;
    private static boolean sdlInitialized;
    private static Surface currentSurface;
    private static Activity activity;
    private static ViewGroup layout;

    private SdlBridge() {}

    public static synchronized boolean setupJNI() {
        if (!jniReady) {
            SDL.setupJNI();
            jniReady = true;
        }
        return true;
    }

    public static synchronized boolean markSdlInitialized() {
        if (sdlInitialized) return false;
        sdlInitialized = true;
        return true;
    }

    public static synchronized void clearSdlInitialized() {
        sdlInitialized = false;
    }

    public static boolean getSdlEnabled() {
        return sdlEnabled;
    }

    public static void setSdlEnabled(boolean enabled) {
        sdlEnabled = enabled;
    }

    public static boolean getSdlImeAutoShowEnabled() {
        return true;
    }

    public static boolean setNativeTextInputActive(boolean active) {
        return true;
    }

    public static boolean isNativeTextInputActive() {
        return true;
    }

    public static boolean isSdlRenderActive() {
        return sdlEnabled;
    }

    public static synchronized void prepareSurface(Activity owner, Surface surface,
                                                   ViewGroup parent, Object source) {
        activity = owner;
        layout = parent;
        currentSurface = surface;
        if (SDLActivity.getSDLSurface() == null) {
            SDL.initialize();
            SDL.setContext(owner);
            SDLSurface sdlSurface = new SDLSurface(owner);
            SDLActivity.externalInitialize(sdlSurface, parent, surface);
        } else {
            SDLSurface.setNativeSurface(surface);
        }
    }

    public static synchronized void syncResolution(int width, int height) {
        if (!sdlEnabled) return;
        SDLSurface surface = SDLActivity.getSDLSurface();
        if (surface != null) {
            surface.surfaceChanged();
            surface.nativeResize(width, height);
        }
    }

    public static synchronized void unregisterSurface(Surface surface) {
        if (surface != null && currentSurface == surface) {
            currentSurface = null;
        }
    }

    public static synchronized void surfaceDestroyed() {
        if (SDLActivity.getSDLSurface() != null) {
            SDLActivity.getSDLSurface().surfaceDestroyed();
        }
        currentSurface = null;
    }
}
