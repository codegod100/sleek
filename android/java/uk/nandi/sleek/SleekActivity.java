package uk.nandi.sleek;

import android.app.Activity;
import android.app.NativeActivity;
import android.content.Intent;
import android.content.res.Resources;
import android.graphics.Color;
import android.graphics.Insets;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.Settings;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowManager;

/**
 * NativeActivity subclass so OAuth deep links ({@code freeq://auth?…}) are
 * delivered while the app is already running.
 *
 * <p>Plain {@code android.app.NativeActivity} never overrides
 * {@link #onNewIntent}, so a warm return from the browser would leave
 * {@link #getIntent()} stale and Rust would never see the callback URI.
 * We stash the latest {@code freeq://} URI in a static that Rust polls via JNI
 * (same pattern as {@link PickFragment}).
 *
 * <p>Also forces <b>edge-to-edge</b> drawing: the native surface fills the
 * physical display (under transparent status / nav bars and into the cutout).
 * Rust reserves real safe-area insets from {@code content_rect} so UI chrome
 * does not sit under the clock or gesture bar — without oversized fixed pads.
 *
 * <p>Exposes {@link #navigationMode()} so Rust can tell 3-button vs gesture
 * nav (for chrome fallbacks — measured insets remain the primary source).
 *
 * <p>Manifest: {@code launchMode=singleTask} + VIEW intent-filter on scheme
 * {@code freeq}. Broker login uses {@code mobile=1} so the redirect is
 * {@code freeq://auth?…} instead of loopback.
 */
public class SleekActivity extends NativeActivity {
    /** Latest unconsumed {@code freeq://…} deep link (or null). */
    public static volatile String pendingDeepLink = null;

    /** Three-button navigation (Back / Home / Recents). */
    public static final int NAV_MODE_THREE_BUTTON = 0;
    /** Two-button navigation (Home pill + Back). */
    public static final int NAV_MODE_TWO_BUTTON = 1;
    /** Full gesture navigation (edge back + home swipe). */
    public static final int NAV_MODE_GESTURES = 2;
    /** Could not determine mode. */
    public static final int NAV_MODE_UNKNOWN = -1;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        applyEdgeToEdge();
        captureIntent(getIntent());
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            // Re-apply after system UI changes (keyboard, multi-window, etc.).
            applyEdgeToEdge();
        }
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        // Required so later getIntent() / scavengers see the OAuth callback.
        setIntent(intent);
        captureIntent(intent);
    }

    /**
     * Draw the NativeActivity surface under system bars / display cutout so
     * the app can use the whole screen. Status and nav remain visible
     * (transparent) — we do not hide them with immersive sticky.
     */
    private void applyEdgeToEdge() {
        Window window = getWindow();
        if (window == null) {
            return;
        }
        window.setStatusBarColor(Color.TRANSPARENT);
        window.setNavigationBarColor(Color.TRANSPARENT);
        if (Build.VERSION.SDK_INT >= 28) {
            WindowManager.LayoutParams lp = window.getAttributes();
            lp.layoutInDisplayCutoutMode =
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            window.setAttributes(lp);
        }
        View decor = window.getDecorView();
        if (decor == null) {
            return;
        }
        // LAYOUT_* flags: content extends under status/nav without hiding them.
        int flags = decor.getSystemUiVisibility();
        flags |= View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION;
        decor.setSystemUiVisibility(flags);
    }

    private static void captureIntent(Intent intent) {
        if (intent == null) {
            return;
        }
        Uri data = intent.getData();
        if (data == null) {
            return;
        }
        if (!"freeq".equals(data.getScheme())) {
            return;
        }
        String s = data.toString();
        if (s != null && !s.isEmpty()) {
            pendingDeepLink = s;
        }
    }

    /**
     * Atomically take the pending deep link (null if none). Called from Rust
     * over JNI after the browser redirects back to the app.
     */
    public static String takePendingDeepLink() {
        String s = pendingDeepLink;
        pendingDeepLink = null;
        return s;
    }

    /**
     * System navigation interaction mode for this activity.
     *
     * <p>Values match Android's internal {@code config_navBarInteractionMode}:
     * {@link #NAV_MODE_THREE_BUTTON}, {@link #NAV_MODE_TWO_BUTTON},
     * {@link #NAV_MODE_GESTURES}, or {@link #NAV_MODE_UNKNOWN}.
     *
     * <p>Called from Rust over JNI each frame (cheap after resource lookup) so
     * chrome can avoid reserving 3-button space under gesture nav.
     */
    public int navigationMode() {
        return detectNavigationMode(this);
    }

    /**
     * Static entry for JNI when only a class + Activity jobject is handy.
     *
     * @param activity host activity (may be null → {@link #NAV_MODE_UNKNOWN})
     */
    public static int navigationModeOf(Activity activity) {
        return detectNavigationMode(activity);
    }

    /**
     * Status bar (+ display cutout) top inset in <b>physical pixels</b>.
     *
     * <p>Prefer this over NativeActivity {@code content_rect}, which often
     * reports top=0 under edge-to-edge and lets UI draw under the clock.
     *
     * @param activity host activity (may be null → 0)
     */
    public static int statusBarInsetPx(Activity activity) {
        if (activity == null) {
            return 0;
        }
        int top = 0;
        try {
            Window window = activity.getWindow();
            View decor = window != null ? window.getDecorView() : null;
            WindowInsets root = decor != null ? decor.getRootWindowInsets() : null;
            if (root != null) {
                if (Build.VERSION.SDK_INT >= 30) {
                    Insets status = root.getInsets(WindowInsets.Type.statusBars());
                    Insets cutout = root.getInsets(WindowInsets.Type.displayCutout());
                    top = Math.max(status.top, cutout.top);
                } else {
                    top = root.getSystemWindowInsetTop();
                    if (Build.VERSION.SDK_INT >= 28 && root.getDisplayCutout() != null) {
                        top = Math.max(top, root.getDisplayCutout().getSafeInsetTop());
                    }
                }
            }
        } catch (Throwable ignored) {
            // fall through
        }
        if (top <= 0) {
            top = dimenPx(activity, "status_bar_height");
        }
        return Math.max(top, 0);
    }

    /**
     * Navigation bar bottom inset in <b>physical pixels</b>.
     *
     * @param activity host activity (may be null → 0)
     */
    public static int navigationBarInsetPx(Activity activity) {
        if (activity == null) {
            return 0;
        }
        int bottom = 0;
        try {
            Window window = activity.getWindow();
            View decor = window != null ? window.getDecorView() : null;
            WindowInsets root = decor != null ? decor.getRootWindowInsets() : null;
            if (root != null) {
                if (Build.VERSION.SDK_INT >= 30) {
                    Insets nav = root.getInsets(WindowInsets.Type.navigationBars());
                    bottom = nav.bottom;
                } else {
                    bottom = root.getSystemWindowInsetBottom();
                }
            }
        } catch (Throwable ignored) {
            // fall through
        }
        if (bottom <= 0) {
            int navRes = dimenPx(activity, "navigation_bar_height");
            if (navRes > 0) {
                float density = activity.getResources().getDisplayMetrics().density;
                float dp = navRes / Math.max(density, 0.01f);
                int mode = detectNavigationMode(activity);
                if (dp >= 40f || mode != NAV_MODE_GESTURES) {
                    bottom = navRes;
                } else if (dp > 0f && dp < 32f) {
                    bottom = navRes;
                }
            }
        }
        return Math.max(bottom, 0);
    }

    private static int dimenPx(Activity activity, String name) {
        try {
            Resources res = activity.getResources();
            int id = res.getIdentifier(name, "dimen", "android");
            if (id > 0) {
                return res.getDimensionPixelSize(id);
            }
        } catch (Throwable ignored) {
            // ignore
        }
        return 0;
    }

    /**
     * Detect 3-button / 2-button / gesture navigation.
     *
     * <p><b>Order matters.</b> Measured nav-bar <em>height</em> is preferred
     * over system-gesture side insets — several OEMs report non-zero left/right
     * gesture zones even with 3-button nav, which previously false-positived
     * as gestures and let the app draw over Back/Home/Recents.
     *
     * <ol>
     *   <li>{@code Settings.Secure.navigation_mode} (user choice, most reliable)</li>
     *   <li>Internal resource {@code config_navBarInteractionMode} (AOSP Q+)</li>
     *   <li>WindowInsets nav-bar bottom height (≥40dp buttons, &lt;32dp gesture)</li>
     *   <li>System-gesture side insets — only when bottom is not button-sized</li>
     *   <li>Resource {@code navigation_bar_height} dimen as last resort</li>
     * </ol>
     */
    static int detectNavigationMode(Activity activity) {
        if (activity == null) {
            return NAV_MODE_UNKNOWN;
        }

        // 1) Secure setting — reflects the user's actual nav mode on most builds.
        // Prefer this over the internal config integer, which some OEMs leave
        // stale when the user switches gestures ↔ buttons.
        try {
            int mode = Settings.Secure.getInt(
                    activity.getContentResolver(), "navigation_mode", -1);
            if (mode == NAV_MODE_THREE_BUTTON
                    || mode == NAV_MODE_TWO_BUTTON
                    || mode == NAV_MODE_GESTURES) {
                return mode;
            }
        } catch (Throwable ignored) {
            // fall through
        }

        // 2) AOSP internal integer (not public API; present on Pixel / stock).
        try {
            Resources res = activity.getResources();
            int id = res.getIdentifier(
                    "config_navBarInteractionMode", "integer", "android");
            if (id > 0) {
                int mode = res.getInteger(id);
                if (mode == NAV_MODE_THREE_BUTTON
                        || mode == NAV_MODE_TWO_BUTTON
                        || mode == NAV_MODE_GESTURES) {
                    return mode;
                }
            }
        } catch (Throwable ignored) {
            // fall through
        }

        // 3) WindowInsets: bottom height first, then side gesture zones.
        try {
            Window window = activity.getWindow();
            View decor = window != null ? window.getDecorView() : null;
            WindowInsets root = decor != null ? decor.getRootWindowInsets() : null;
            if (root != null && Build.VERSION.SDK_INT >= 29) {
                float density = activity.getResources().getDisplayMetrics().density;
                float bottomDp = 0f;
                if (Build.VERSION.SDK_INT >= 30) {
                    Insets nav = root.getInsets(WindowInsets.Type.navigationBars());
                    bottomDp = nav.bottom / Math.max(density, 0.01f);
                } else {
                    bottomDp = root.getSystemWindowInsetBottom()
                            / Math.max(density, 0.01f);
                }
                // 3-button bars are typically ≥40dp; gesture handle ~16–24dp.
                if (bottomDp >= 40f) {
                    return NAV_MODE_THREE_BUTTON;
                }
                if (bottomDp > 0f && bottomDp < 32f) {
                    return NAV_MODE_GESTURES;
                }

                // Side system-gesture zones: only trust when the bottom bar is
                // not already button-sized (tall bottoms win above).
                if (Build.VERSION.SDK_INT >= 30) {
                    Insets g = root.getInsets(WindowInsets.Type.systemGestures());
                    if ((g.left > 0 || g.right > 0) && bottomDp < 40f) {
                        return NAV_MODE_GESTURES;
                    }
                } else {
                    android.graphics.Insets g = root.getSystemGestureInsets();
                    if ((g.left > 0 || g.right > 0) && bottomDp < 40f) {
                        return NAV_MODE_GESTURES;
                    }
                }
            }
        } catch (Throwable ignored) {
            // fall through
        }

        // 4) Resource nav_bar height when present.
        try {
            Resources res = activity.getResources();
            int id = res.getIdentifier("navigation_bar_height", "dimen", "android");
            if (id > 0) {
                int px = res.getDimensionPixelSize(id);
                float density = res.getDisplayMetrics().density;
                float dp = px / Math.max(density, 0.01f);
                if (dp >= 40f) {
                    return NAV_MODE_THREE_BUTTON;
                }
                if (dp > 0f && dp < 32f) {
                    return NAV_MODE_GESTURES;
                }
            }
        } catch (Throwable ignored) {
            // fall through
        }

        return NAV_MODE_UNKNOWN;
    }
}
