package uk.nandi.sleek;

import android.app.NativeActivity;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;

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
 * <p>Manifest: {@code launchMode=singleTask} + VIEW intent-filter on scheme
 * {@code freeq}. Broker login uses {@code mobile=1} so the redirect is
 * {@code freeq://auth?…} instead of loopback.
 */
public class SleekActivity extends NativeActivity {
    /** Latest unconsumed {@code freeq://…} deep link (or null). */
    public static volatile String pendingDeepLink = null;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        captureIntent(getIntent());
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        // Required so later getIntent() / scavengers see the OAuth callback.
        setIntent(intent);
        captureIntent(intent);
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
}
