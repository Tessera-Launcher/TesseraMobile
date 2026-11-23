package org.tessera.mobile;

import android.app.Activity;
import android.content.pm.ActivityInfo;
import android.graphics.Color;
import android.graphics.SurfaceTexture;
import android.os.Bundle;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.Gravity;
import android.view.Surface;
import android.view.TextureView;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;

/** Full-screen game host. Slint stays in the launcher's NativeActivity. */
public final class GameActivity extends Activity implements TextureView.SurfaceTextureListener {
    private TextureView view;
    private FrameLayout root;
    private boolean started;
    private boolean menuPointerDown;

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
        // The SDLSurface created by SdlBridge is not in the view hierarchy, so
        // its touch listener never runs.  Feed the visible TextureView's raw
        // finger stream into SDL; TouchController handles the SDL finger
        // events in 26.3, while its own mouse synthesis still serves menus.
        view.setOnTouchListener((v, event) -> {
            handleTouch(event, v.getWidth(), v.getHeight());
            return true;
        });
        root.addView(view, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        addLauncherKeys();
        setContentView(root);
    }

    /** Launcher-owned keys for things TouchController does not expose. */
    private void addLauncherKeys() {
        LinearLayout bar = new LinearLayout(this);
        bar.setOrientation(LinearLayout.HORIZONTAL);
        bar.setPadding(dp(4), dp(2), dp(4), dp(2));
        bar.setBackgroundColor(Color.argb(90, 0, 0, 0));

        Button escape = keyButton("ESC", KeyEvent.KEYCODE_ESCAPE);
        Button f3 = keyButton("F3", KeyEvent.KEYCODE_F3);
        bar.addView(escape, new LinearLayout.LayoutParams(dp(54), dp(38)));
        LinearLayout.LayoutParams f3Params = new LinearLayout.LayoutParams(dp(40), dp(38));
        f3Params.setMargins(dp(4), 0, 0, 0);
        bar.addView(f3, f3Params);

        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP | Gravity.START);
        params.setMargins(dp(12), dp(12), 0, 0);
        root.addView(bar, params);
    }

    private Button keyButton(String label, int keyCode) {
        Button button = new Button(this);
        button.setText(label);
        button.setTextSize(12);
        button.setTextColor(Color.WHITE);
        button.setAllCaps(false);
        button.setPadding(0, 0, 0, 0);
        button.setMinWidth(0);
        button.setMinimumWidth(0);
        button.setMinHeight(0);
        button.setMinimumHeight(0);
        button.setBackgroundColor(Color.argb(150, 0, 0, 0));
        button.setOnClickListener(v -> sendKeyTap(keyCode));
        return button;
    }

    private void sendKeyTap(int keyCode) {
        org.libsdl.app.SDLActivity.onNativeKeyDown(keyCode);
        root.postDelayed(() -> org.libsdl.app.SDLActivity.onNativeKeyUp(keyCode), 50);
    }

    private int dp(int value) {
        return (int) (value * getResources().getDisplayMetrics().density + 0.5f);
    }


    /**
     * TouchController owns the SDL finger stream.  Minecraft's menus do not
     * have TouchController widgets, so keep the old absolute mouse path for
     * non-grabbed screens as a menu pointer; in-world touches stay finger-only.
     */
    private void handleTouch(MotionEvent event, int width, int height) {
        TesseraSdl.sendTouch(event, width, height);
        // Once Minecraft has grabbed the pointer, TouchController owns the
        // finger stream.  Do not also inject an absolute mouse button: a held
        // fallback button from the forward widget would keep attacking and
        // prevent the look gesture from taking effect.
        if (org.lwjgl.glfw.CallbackBridge.isGrabbing()) {
            menuPointerDown = false;
            return;
        }
        // launch.rs starts the game with an 854x480 logical window; the
        // TextureView is the physical 2264x1080 surface.
        float x = event.getX() * 854f / width;
        float y = event.getY() * 480f / height;
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                org.lwjgl.glfw.CallbackBridge.sendCursorPos(x, y);
                org.lwjgl.glfw.CallbackBridge.sendMouseButton(0, true);
                menuPointerDown = true;
                break;
            case MotionEvent.ACTION_MOVE:
                org.lwjgl.glfw.CallbackBridge.sendCursorPos(x, y);
                break;
            case MotionEvent.ACTION_CANCEL:
            case MotionEvent.ACTION_UP:
                if (menuPointerDown) {
                    org.lwjgl.glfw.CallbackBridge.sendMouseButton(0, false);
                    menuPointerDown = false;
                }
                break;
            default:
                break;
        }
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        // GameActivity is not an SDLActivity, so SDL never hears about the
        // window focus change unless the host forwards it.
        try {
            org.libsdl.app.SDLActivity.nativeFocusChanged(hasFocus);
        } catch (Throwable problem) {
            android.util.Log.e("TesseraTouch", "SDL focus failed", problem);
        }
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        if (event.getAction() == KeyEvent.ACTION_DOWN) {
            org.libsdl.app.SDLActivity.onNativeKeyDown(event.getKeyCode());
        } else if (event.getAction() == KeyEvent.ACTION_UP) {
            org.libsdl.app.SDLActivity.onNativeKeyUp(event.getKeyCode());
        }
        return true;
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
