package uk.nandi.sleek;

import android.app.Activity;
import android.app.Fragment;
import android.app.FragmentManager;
import android.content.Intent;

/**
 * Invisible helper used by NativeActivity (which has no {@code onActivityResult}
 * override). {@link #startPick} adds this fragment and calls
 * {@link #startActivityForResult} so the framework routes the result back here
 * via {@code Activity.dispatchActivityResult(who, …)}.
 *
 * <p>Request codes must fit in 16 bits (fragment results mask with {@code 0xffff}).
 */
public class PickFragment extends Fragment {
    public static final String TAG = "sleek_pick_fragment";
    /** Must stay within 0x0000..0xFFFF for Fragment result routing. */
    public static final int REQ = 0x1EE6;

    public static volatile boolean done = false;
    public static volatile int resultCode = Integer.MIN_VALUE;
    public static volatile Intent data = null;
    public static volatile String error = null;

    public static void reset() {
        done = false;
        resultCode = Integer.MIN_VALUE;
        data = null;
        error = null;
    }

    /**
     * Ensure the helper fragment is attached and open the system picker.
     * Always hops to the main thread (FragmentManager / startActivity rules).
     */
    public static void startPick(final Activity activity, final Intent intent) {
        // Clear any previous result *before* posting to the UI thread so a
        // Rust poller cannot observe a stale done=true from the last pick.
        reset();
        if (activity == null || intent == null) {
            error = "null activity or intent";
            resultCode = 0;
            data = null;
            done = true;
            return;
        }
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                try {
                    FragmentManager fm = activity.getFragmentManager();
                    PickFragment f = (PickFragment) fm.findFragmentByTag(TAG);
                    if (f == null) {
                        f = new PickFragment();
                        fm.beginTransaction().add(f, TAG).commit();
                        fm.executePendingTransactions();
                    }
                    f.startActivityForResult(intent, REQ);
                } catch (Throwable t) {
                    error = t.getMessage() != null ? t.getMessage() : t.toString();
                    resultCode = 0;
                    data = null;
                    done = true;
                }
            }
        });
    }

    @Override
    public void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode != REQ) {
            return;
        }
        PickFragment.resultCode = resultCode;
        PickFragment.data = data;
        PickFragment.done = true;
    }
}
