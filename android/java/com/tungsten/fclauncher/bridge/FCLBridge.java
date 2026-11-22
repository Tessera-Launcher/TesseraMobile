package com.tungsten.fclauncher.bridge;

/** The parts of FCL's bridge that the SDL Java layer touches. */
public final class FCLBridge {
    private FCLBridge() {}
    public static native void initializeHooks();
    public static void openLink(String url) {}
}
