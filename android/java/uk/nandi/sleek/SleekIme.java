package uk.nandi.sleek;

import android.app.Activity;
import android.content.Context;
import android.text.InputType;
import android.util.Log;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.FrameLayout;

/**
 * Hidden {@link EditText} IME bridge for NativeActivity.
 *
 * <p>NativeActivity can show the soft keyboard but has no {@link InputConnection},
 * so Gboard swipe/glide typing (which uses {@code setComposingText} /
 * {@code commitText}) never reaches egui. This invisible editor receives IME
 * composing events and forwards them to Rust via JNI.
 *
 * <p>Compiled into {@code sleek_activity.dex} (APK {@code classes.dex}).
 */
public final class SleekIme {
    private static final String TAG = "SleekIme";

    private static volatile ImeBridgeEditText field;

    static {
        try {
            System.loadLibrary("sleek");
        } catch (UnsatisfiedLinkError e) {
            Log.w(TAG, "libsleek not loaded yet; natives resolve after NativeActivity init", e);
        }
    }

    private SleekIme() {}

    static void nativePreedit(String text) {
        onPreedit(text);
    }

    static void nativeCommit(String text) {
        onCommit(text);
    }

    static void nativeDeleteSurrounding(int beforeLength, int afterLength) {
        onDeleteSurrounding(beforeLength, afterLength);
    }

    private static native void onPreedit(String text);

    private static native void onCommit(String text);

    private static native void onDeleteSurrounding(int beforeLength, int afterLength);

    /**
     * Show or hide the IME bridge. Called from Rust when egui wants keyboard input.
     *
     * @param activity host activity (non-null)
     * @param active true to focus the bridge and show the soft keyboard
     */
    public static void setActive(Activity activity, boolean active) {
        if (activity == null) {
            return;
        }
        activity.runOnUiThread(() -> setActiveOnUiThread(activity, active));
    }

    private static void setActiveOnUiThread(Activity activity, boolean active) {
        try {
            ensureField(activity);
            InputMethodManager imm =
                    (InputMethodManager)
                            activity.getSystemService(Context.INPUT_METHOD_SERVICE);
            if (imm == null) {
                return;
            }
            if (active) {
                field.requestFocus();
                imm.showSoftInput(field, InputMethodManager.SHOW_IMPLICIT);
            } else {
                imm.hideSoftInputFromWindow(field.getWindowToken(), 0);
                field.clearFocus();
                onPreedit("");
            }
        } catch (Throwable t) {
            Log.w(TAG, "setActive(" + active + ") failed", t);
        }
    }

    private static void ensureField(Activity activity) {
        if (field != null) {
            return;
        }
        ImeBridgeEditText edit = new ImeBridgeEditText(activity);
        edit.setFocusable(true);
        edit.setFocusableInTouchMode(true);
        // INVISIBLE (not GONE): IME still attaches; GONE can drop InputConnection.
        edit.setVisibility(View.INVISIBLE);
        edit.setBackgroundColor(0x00000000);

        View content = activity.findViewById(android.R.id.content);
        if (content instanceof FrameLayout) {
            FrameLayout.LayoutParams lp =
                    new FrameLayout.LayoutParams(1, 1);
            ((FrameLayout) content).addView(edit, lp);
        } else if (content instanceof ViewGroup) {
            ViewGroup.LayoutParams lp =
                    new ViewGroup.LayoutParams(1, 1);
            ((ViewGroup) content).addView(edit, lp);
        } else {
            Log.w(TAG, "content root is not a ViewGroup; IME bridge unavailable");
            return;
        }
        field = edit;
    }

    /** Invisible editor whose {@link InputConnection} forwards IME events to Rust. */
    private static final class ImeBridgeEditText extends EditText {
        ImeBridgeEditText(Context context) {
            super(context);
        }

        @Override
        public InputConnection onCreateInputConnection(EditorInfo outAttrs) {
            outAttrs.inputType =
                    InputType.TYPE_CLASS_TEXT
                            | InputType.TYPE_TEXT_FLAG_MULTI_LINE
                            | InputType.TYPE_TEXT_FLAG_AUTO_CORRECT;
            outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI;
            return new BridgeInputConnection(this);
        }
    }

    /** Named class (not anonymous) so d8 can dex SleekActivity.jar reliably. */
    private static final class BridgeInputConnection extends BaseInputConnection {
        BridgeInputConnection(ImeBridgeEditText target) {
            super(target, true);
        }

        @Override
        public boolean setComposingText(CharSequence text, int newCursorPosition) {
            String s = text == null ? "" : text.toString();
            nativePreedit(s);
            return true;
        }

        @Override
        public boolean finishComposingText() {
            nativePreedit("");
            return true;
        }

        @Override
        public boolean commitText(CharSequence text, int newCursorPosition) {
            if (text != null && text.length() > 0) {
                nativeCommit(text.toString());
            }
            return true;
        }

        @Override
        public boolean deleteSurroundingText(int beforeLength, int afterLength) {
            if (beforeLength > 0 || afterLength > 0) {
                nativeDeleteSurrounding(beforeLength, afterLength);
            }
            return true;
        }
    }
}
