package org.tessera.mobile;

import android.app.Activity;
import android.content.pm.ActivityInfo;
import android.graphics.Color;
import android.graphics.SurfaceTexture;
import android.os.Bundle;
import android.view.Surface;
import android.view.TextureView;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowManager;
import android.widget.FrameLayout;

/** Full-screen game host. Slint stays in the launcher's NativeActivity. */
public final class GameActivity extends Activity implements TextureView.SurfaceTextureListener {
    private TextureView view;
    private FrameLayout root;
    private boolean started;

    static {
        // NativeActivity loaded this too, but binding the library to the class
        // loader that owns GameActivity is what lets JNI resolve nativeStartGame.
        System.loadLibrary("tessera_mobile");
    }

    public static native void nativeStartGame(String version);

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        requestWindowFeature(Window.FEATURE_NO_TITLE);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        getWindow().getDecorView().setSystemUiVisibility(
                android.view.View.SYSTEM_UI_FLAG_FULLSCREEN
                        | android.view.View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                        | android.view.View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                        | android.view.View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                        | android.view.View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                        | android.view.View.SYSTEM_UI_FLAG_LAYOUT_STABLE);
        setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE);

        root = new FrameLayout(this);
        root.setBackgroundColor(Color.BLACK);
        view = new TextureView(this);
        view.setSurfaceTextureListener(this);
        root.addView(view, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        setContentView(root);
    }

    @Override
    public void onSurfaceTextureAvailable(SurfaceTexture texture, int width, int height) {
        if (started) return;
        started = true;
        Surface surface = new Surface(texture);
        TesseraSdl.initialize(this);
        // Load SDL3 in the app VM first so its JNI_OnLoad registers the Java
        // callbacks. Rust then marks the game thread as SDL's main thread.
        TesseraSdl.loadSdl();
        TesseraSdl.prepareSurface(this, surface, root);
        org.lwjgl.glfw.CallbackBridge.windowWidth = width;
        org.lwjgl.glfw.CallbackBridge.windowHeight = height;
        String version = getIntent().getStringExtra("version");
        if (version == null) version = "";
        nativeStartGame(version);
    }

    @Override
    public void onSurfaceTextureSizeChanged(SurfaceTexture texture, int width, int height) {
        org.lwjgl.glfw.CallbackBridge.windowWidth = width;
        org.lwjgl.glfw.CallbackBridge.windowHeight = height;
        TesseraSdl.syncResolution(width, height);
    }

    @Override
    public boolean onSurfaceTextureDestroyed(SurfaceTexture texture) {
        TesseraSdl.surfaceDestroyed();
        return true;
    }

    @Override
    public void onSurfaceTextureUpdated(SurfaceTexture texture) {
    }

    // SDL's native messagebox helper resolves this on GameActivity, not on
    // SDLActivity. The game only uses it for startup errors.
    public int messageboxShowMessageBox(int flags, String title, String message,
                                        int[] buttonFlags, int[] buttonIds,
                                        String[] buttonTexts, int[] colors) {
        return org.libsdl.app.SDLActivity.messageboxShowMessageBox(
                this, flags, title, message, buttonFlags, buttonIds, buttonTexts, colors);
    }
}
