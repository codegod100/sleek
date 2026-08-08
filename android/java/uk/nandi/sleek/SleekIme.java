package uk.nandi.sleek;

import android.app.Activity;
import android.content.Context;
import android.text.Editable;
import android.text.InputType;
import android.text.Selection;
import android.text.TextWatcher;
import android.util.Log;
import android.view.View;
import android.view.ViewGroup;
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
 * <p>Deletes are forwarded via a {@link TextWatcher}: Gboard may shrink the
 * Editable through {@code deleteSurroundingText}, {@code sendKeyEvent}, or
 * direct key dispatch — all of those change the Editable without a single
 * reliable callback.
 *
 * <p>Compiled into {@code sleek_activity.dex} (APK {@code classes.dex}).
 */
public final class SleekIme {
    private static final String TAG = "SleekIme";

    private static volatile ImeBridgeEditText field;

    /** True while Gboard holds a composing span — {@link #syncField} must not clobber it. */
    private static volatile boolean composing;

    /** True while {@link #syncField} is rewriting the Editable (do not treat as delete). */
    private static volatile boolean updatingFromSync;

    /** True while BridgeInputConnection is applying an IME mutation that already
     * has an explicit JNI forward (preedit/commit). Suppresses TextWatcher deletes.
     */
    private static volatile boolean updatingFromIme;

    /**
     * Monotonic generation for {@link #syncField} runnables. Bumped on IME edits so
     * a stale empty sync posted before commitText cannot wipe the new character.
     */
    private static volatile int syncGen;

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
     * Hardware / {@code adb keyevent} DEL is delivered to {@link NativeActivity}, not the
     * hidden EditText. Call from {@link SleekActivity#dispatchKeyEvent} while the bridge
     * is focused so egui still deletes.
     *
     * @return true if the delete was handled
     */
    public static boolean onHostKeyDelete() {
        // NativeActivity often keeps window key-focus even while the bridge
        // EditText is the IME target (served=true). Do not require hasFocus().
        if (field == null || field.getVisibility() != View.VISIBLE) {
            return false;
        }
        try {
            Editable editable = field.getText();
            if (editable == null) {
                return false;
            }
            int start = Selection.getSelectionStart(editable);
            int end = Selection.getSelectionEnd(editable);
            if (start < 0) {
                start = editable.length();
                end = start;
            }
            if (end < 0) {
                end = start;
            }
            if (start != end) {
                editable.delete(Math.min(start, end), Math.max(start, end));
                Log.i(TAG, "onHostKeyDelete selection");
                return true;
            }
            if (start > 0) {
                editable.delete(start - 1, start);
                Log.i(TAG, "onHostKeyDelete char");
                return true;
            }
            // Already empty — do not synthesize another Backspace (that Disabled
            // the IME session and left ime_cursor_range stale for the next commit).
            Log.i(TAG, "onHostKeyDelete already empty");
            return true;
        } catch (Throwable t) {
            Log.w(TAG, "onHostKeyDelete failed", t);
            return false;
        }
    }

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

    /**
     * Mirror egui compose text/cursor into the hidden editor so Gboard can glide-type
     * mid-edit. Must not call {@code restartInput} — that resets the IME each frame.
     * Skips while composing so we do not wipe Gboard's composing span mid-swipe.
     */
    public static void syncField(Activity activity, String text, int selStart, int selEnd) {
        if (activity == null) {
            return;
        }
        final String safeText = text == null ? "" : text;
        final int gen = syncGen;
        activity.runOnUiThread(
                () -> {
                    if (gen != syncGen) {
                        // Superseded by a newer IME edit or sync.
                        return;
                    }
                    syncFieldOnUiThread(activity, safeText, selStart, selEnd);
                });
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
                return;
            }
            if (active) {
                // VISIBLE (alpha 0): IMM ignores showSoftInput for INVISIBLE/"not served".
                field.setVisibility(View.VISIBLE);
                field.setAlpha(0f);
                if (activity.getWindow() != null) {
                    activity
                            .getWindow()
                            .setSoftInputMode(
                                    WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE
                                            | WindowManager.LayoutParams
                                                    .SOFT_INPUT_ADJUST_RESIZE);
                }
                field.requestFocus();
                // Focus/serve often lands after this frame; post the show.
                final ImeBridgeEditText target = field;
                target.post(
                        () -> {
                            if (!target.hasFocus()) {
                                target.requestFocus();
                            }
                            boolean shown =
                                    imm.showSoftInput(
                                            target, InputMethodManager.SHOW_IMPLICIT);
                            if (!shown) {
                                // Fallback: some NativeActivity focus fights need forced show.
                                imm.showSoftInput(target, 0);
                            }
                            Log.i(TAG, "showSoftInput served=" + target.isFocused()
                                    + " shown=" + shown);
                        });
            } else {
                composing = false;
                imm.hideSoftInputFromWindow(field.getWindowToken(), 0);
                field.clearFocus();
                field.setVisibility(View.INVISIBLE);
                if (activity.getWindow() != null) {
                    activity
                            .getWindow()
                            .setSoftInputMode(
                                    WindowManager.LayoutParams.SOFT_INPUT_STATE_HIDDEN
                                            | WindowManager.LayoutParams
                                                    .SOFT_INPUT_ADJUST_RESIZE);
                }
                onPreedit("");
            }
        } catch (Throwable t) {
            Log.w(TAG, "setActive(" + active + ") failed", t);
        }
    }

    private static void syncFieldOnUiThread(
            Activity activity, String text, int selStart, int selEnd) {
        try {
            ensureField(activity);
            if (field == null) {
                return;
            }
            // Never overwrite mid-compose: egui's committed text lags Gboard's
            // composing span, and replace() clears the span — next swipe dies.
            if (composing) {
                return;
            }
            Editable editable = field.getText();
            if (editable == null) {
                return;
            }
            if (BaseInputConnection.getComposingSpanStart(editable) >= 0
                    || BaseInputConnection.getComposingSpanEnd(editable) >= 0) {
                composing = true;
                return;
            }
            int len = text.length();
            int start = Math.max(0, Math.min(selStart, len));
            int end = Math.max(start, Math.min(selEnd, len));
            boolean changed = !text.contentEquals(editable);
            if (changed) {
                updatingFromSync = true;
                try {
                    editable.replace(0, editable.length(), text);
                } finally {
                    updatingFromSync = false;
                }
            }
            // Avoid slamming the caret to 0 on a non-empty field (egui sometimes
            // reports 0,0 before TextEdit state is ready) — that breaks backspace.
            if (start == 0 && end == 0 && len > 0) {
                int cur = Selection.getSelectionStart(editable);
                if (cur > 0) {
                    start = cur;
                    end = Math.max(cur, Selection.getSelectionEnd(editable));
                } else {
                    start = len;
                    end = len;
                }
            }
            int curStart = Selection.getSelectionStart(editable);
            int curEnd = Selection.getSelectionEnd(editable);
            if (curStart != start || curEnd != end) {
                Selection.setSelection(editable, start, end);
                changed = true;
            }
            if (changed) {
                notifySelection(activity, editable, start, end);
            }
        } catch (Throwable t) {
            Log.w(TAG, "syncField failed", t);
            updatingFromSync = false;
        }
    }

    private static void notifySelection(
            Activity activity, Editable editable, int start, int end) {
        if (field == null || !field.hasFocus()) {
            return;
        }
        InputMethodManager imm =
                (InputMethodManager) activity.getSystemService(Context.INPUT_METHOD_SERVICE);
        if (imm == null) {
            return;
        }
        imm.updateSelection(field, start, end, -1, -1);
    }

    private static void ensureField(Activity activity) {
        if (field != null) {
            return;
        }
        ImeBridgeEditText edit = new ImeBridgeEditText(activity);
        edit.setFocusable(true);
        edit.setFocusableInTouchMode(true);
        edit.setClickable(true);
        edit.setLongClickable(false);
        edit.setCursorVisible(false);
        // Keep VISIBLE at alpha 0 — IMM on modern Android refuses INVISIBLE views
        // ("Ignoring showSoftInput() … is not served").
        edit.setVisibility(View.VISIBLE);
        edit.setAlpha(0f);
        edit.setBackgroundColor(0x00000000);
        edit.addTextChangedListener(new DeleteWatcher());

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

    /**
     * Forwards Editable shortening to Rust. Covers Gboard {@code deleteSurroundingText},
     * {@code sendKeyEvent(DEL)}, and any other path that mutates the editor.
     */
    private static final class DeleteWatcher implements TextWatcher {
        private int beforeLen;

        @Override
        public void beforeTextChanged(CharSequence s, int start, int count, int after) {
            beforeLen = s == null ? 0 : s.length();
        }

        @Override
        public void onTextChanged(CharSequence s, int start, int before, int count) {}

        @Override
        public void afterTextChanged(Editable s) {
            if (updatingFromSync || updatingFromIme || composing) {
                return;
            }
            int afterLen = s == null ? 0 : s.length();
            int deleted = beforeLen - afterLen;
            if (deleted > 0) {
                Log.i(TAG, "textDeleted count=" + deleted + " len=" + afterLen);
                nativeDeleteSurrounding(deleted, 0);
            }
        }
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
            outAttrs.initialSelStart = getSelectionStart();
            outAttrs.initialSelEnd = getSelectionEnd();
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
            syncGen++; // invalidate stale syncField runnables
            updatingFromIme = true;
            try {
                composing = text != null && text.length() > 0;
                boolean ok = super.setComposingText(text, newCursorPosition);
                String s = text == null ? "" : text.toString();
                Log.i(TAG, "setComposingText len=" + s.length() + " composing=" + composing);
                nativePreedit(s);
                return ok;
            } finally {
                updatingFromIme = false;
            }
        }

        @Override
        public boolean setComposingRegion(int start, int end) {
            composing = end > start;
            return super.setComposingRegion(start, end);
        }

        @Override
        public boolean finishComposingText() {
            updatingFromIme = true;
            try {
                composing = false;
                boolean ok = super.finishComposingText();
                nativePreedit("");
                return ok;
            } finally {
                updatingFromIme = false;
            }
        }

        @Override
        public boolean commitText(CharSequence text, int newCursorPosition) {
            syncGen++; // invalidate stale syncField("") from backspace-to-empty
            updatingFromIme = true;
            try {
                composing = false;
                boolean ok = super.commitText(text, newCursorPosition);
                if (text != null && text.length() > 0) {
                    String s = text.toString();
                    Log.i(TAG, "commitText len=" + s.length());
                    nativeCommit(s);
                } else {
                    nativePreedit("");
                }
                // Keep IMM selection in sync so the next glide starts at the right cursor.
                Editable editable = getEditable();
                if (editable != null && field != null) {
                    int start = Selection.getSelectionStart(editable);
                    int end = Selection.getSelectionEnd(editable);
                    Context ctx = field.getContext();
                    if (ctx instanceof Activity) {
                        notifySelection((Activity) ctx, editable, start, end);
                    }
                }
                return ok;
            } finally {
                updatingFromIme = false;
            }
        }

        // Deletes: TextWatcher → nativeDeleteSurrounding (do not also call here).
    }
}
