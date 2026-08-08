package uk.nandi.sleek;

import android.app.Activity;
import android.content.Context;
import android.text.InputType;
import android.util.Log;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewTreeObserver;
import android.view.Window;
import android.view.WindowManager;
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
    /** Avoid racing hideSoftInput against a posted showSoftInput. */
    private static volatile boolean showPending;

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
            if (field == null) {
                Log.w(TAG, "setActive(" + active + "): IME bridge field unavailable");
                return;
            }
            InputMethodManager imm =
                    (InputMethodManager)
                            activity.getSystemService(Context.INPUT_METHOD_SERVICE);
            if (imm == null) {
                Log.w(TAG, "setActive(" + active + "): InputMethodManager null");
                return;
            }
            if (active) {
                showPending = true;
                showKeyboardWhenReady(activity, field, imm);
            } else {
                if (showPending) {
                    field.removeCallbacks(SHOW_KEYBOARD);
                    showPending = false;
                }
                hideKeyboard(field, imm);
            }
        } catch (Throwable t) {
            Log.w(TAG, "setActive(" + active + ") failed", t);
        }
    }

    private static final ShowKeyboardTask SHOW_KEYBOARD = new ShowKeyboardTask();

    private static final class ShowKeyboardTask implements Runnable {
        private Activity activity;
        private ImeBridgeEditText target;
        private InputMethodManager imm;
        private int attempts;

        void bind(Activity activity, ImeBridgeEditText target, InputMethodManager imm) {
            this.activity = activity;
            this.target = target;
            this.imm = imm;
            this.attempts = 0;
        }

        @Override
        public void run() {
            if (!showPending || target == null || imm == null) {
                return;
            }
            if (!target.isAttachedToWindow() || target.getWindowToken() == null) {
                if (++attempts < 20) {
                    target.postDelayed(this, 16);
                } else {
                    Log.w(TAG, "showSoftInput: bridge never attached");
                    showPending = false;
                }
                return;
            }
            Window window = activity != null ? activity.getWindow() : null;
            if (window != null) {
                window.setSoftInputMode(
                        WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE
                                | WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE);
            }
            target.requestFocus();
            boolean shown = imm.showSoftInput(target, InputMethodManager.SHOW_IMPLICIT);
            Log.d(
                    TAG,
                    "showSoftInput implicit="
                            + shown
                            + " focus="
                            + target.isFocused()
                            + " bounds="
                            + target.getWidth()
                            + "x"
                            + target.getHeight());
            if (!shown && ++attempts < 8) {
                target.postDelayed(this, 50);
                return;
            }
            showPending = false;
        }
    }

    private static void showKeyboardWhenReady(
            Activity activity, ImeBridgeEditText field, InputMethodManager imm) {
        SHOW_KEYBOARD.bind(activity, field, imm);
        if (field.isAttachedToWindow() && field.getWidth() > 0 && field.getHeight() > 0) {
            field.post(SHOW_KEYBOARD);
            return;
        }
        ViewTreeObserver observer = field.getViewTreeObserver();
        if (observer.isAlive()) {
            observer.addOnGlobalLayoutListener(
                    new ViewTreeObserver.OnGlobalLayoutListener() {
                        @Override
                        public void onGlobalLayout() {
                            ViewTreeObserver obs = field.getViewTreeObserver();
                            if (obs.isAlive()) {
                                obs.removeOnGlobalLayoutListener(this);
                            }
                            if (showPending) {
                                field.post(SHOW_KEYBOARD);
                            }
                        }
                    });
        } else {
            field.post(SHOW_KEYBOARD);
        }
    }

    private static void hideKeyboard(ImeBridgeEditText field, InputMethodManager imm) {
        imm.hideSoftInputFromWindow(field.getWindowToken(), 0);
        field.clearFocus();
        onPreedit("");
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
            // Non-zero size so layout/IME serving succeeds (1×1 was "not served").
            FrameLayout.LayoutParams lp =
                    new FrameLayout.LayoutParams(
                            ViewGroup.LayoutParams.MATCH_PARENT,
                            ViewGroup.LayoutParams.WRAP_CONTENT);
            ((FrameLayout) content).addView(edit, lp);
        } else if (content instanceof ViewGroup) {
            ViewGroup.LayoutParams lp =
                    new ViewGroup.LayoutParams(
                            ViewGroup.LayoutParams.MATCH_PARENT,
                            ViewGroup.LayoutParams.WRAP_CONTENT);
            ((ViewGroup) content).addView(edit, lp);
        } else {
            Log.w(TAG, "content root is not a ViewGroup; IME bridge unavailable");
            return;
        }
        edit.setShowSoftInputOnFocus(true);
        field = edit;
        Log.d(TAG, "IME bridge EditText attached to content root");
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
            outAttrs.imeOptions =
                    EditorInfo.IME_ACTION_NONE | EditorInfo.IME_FLAG_NO_EXTRACT_UI;
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
            // Gboard glide typing requires composing spans in the Editable; super
            // maintains that state while we forward preedit to egui via JNI.
            boolean ok = super.setComposingText(text, newCursorPosition);
            String s = text == null ? "" : text.toString();
            nativePreedit(s);
            return ok;
        }

        @Override
        public boolean finishComposingText() {
            boolean ok = super.finishComposingText();
            nativePreedit("");
            return ok;
        }

        @Override
        public boolean commitText(CharSequence text, int newCursorPosition) {
            boolean ok = super.commitText(text, newCursorPosition);
            if (text != null && text.length() > 0) {
                nativeCommit(text.toString());
            }
            return ok;
        }

        @Override
        public boolean deleteSurroundingText(int beforeLength, int afterLength) {
            boolean ok = super.deleteSurroundingText(beforeLength, afterLength);
            if (beforeLength > 0 || afterLength > 0) {
                nativeDeleteSurrounding(beforeLength, afterLength);
            }
            return ok;
        }
    }
}
