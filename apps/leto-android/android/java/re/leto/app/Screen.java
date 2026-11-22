package re.leto.app;

import android.app.Activity;
import android.os.Build;
import android.util.Log;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;

/**
 * What a stream asks of Android's window, which Android only takes on the
 * thread that draws its windows: the app itself is Rust on a thread of its
 * own, and calls these.
 */
public final class Screen {
    private static final String TAG = "leto";

    private Screen() {}

    /**
     * Hides the status bar, the navigation bar and a desktop's taskbar and
     * window header while {@code hide}, so that the whole screen is the
     * PC's and its edges are not Android's. A swipe from an edge shows them
     * for a moment.
     */
    public static void bars(final Activity activity, final boolean hide) {
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                try {
                    Window window = activity.getWindow();
                    if (Build.VERSION.SDK_INT >= 30) {
                        WindowInsetsController bars = window.getInsetsController();
                        if (bars == null) {
                            Log.w(TAG, "the window has nothing to hide Android's bars with");
                            return;
                        }
                        if (hide) {
                            bars.setSystemBarsBehavior(
                                    WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
                            bars.hide(WindowInsets.Type.systemBars());
                        } else {
                            bars.show(WindowInsets.Type.systemBars());
                        }
                    } else {
                        legacy(window.getDecorView(), hide);
                    }
                    Log.i(TAG, hide ? "Android's bars are hidden" : "Android's bars are back");
                } catch (RuntimeException error) {
                    Log.w(TAG, "Android's bars stay as they are: " + error);
                }
            }
        });
    }

    @SuppressWarnings("deprecation")
    private static void legacy(View view, boolean hide) {
        view.setSystemUiVisibility(hide
                ? View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                        | View.SYSTEM_UI_FLAG_FULLSCREEN
                        | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                        | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                : View.SYSTEM_UI_FLAG_VISIBLE);
    }

    /**
     * Asks for Android's own keyboard shortcuts while {@code on} (the
     * Windows key, Alt+Tab), which then reach the window as any key does,
     * and gives them back when not. Android 16's second quarterly release
     * has this; earlier ones keep their shortcuts.
     */
    public static void keys(final Activity activity, final boolean on) {
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                try {
                    Window window = activity.getWindow();
                    WindowManager.LayoutParams attributes = window.getAttributes();
                    // Looked up by name: the app is also built with, and
                    // runs on, versions of Android that do not have it.
                    WindowManager.LayoutParams.class
                            .getMethod("setKeyboardCaptureEnabled", boolean.class)
                            .invoke(attributes, on);
                    window.setAttributes(attributes);
                    Log.i(TAG, on
                            ? "Android's own keyboard shortcuts are asked for the PC"
                            : "Android's own keyboard shortcuts are given back");
                } catch (ReflectiveOperationException | RuntimeException error) {
                    Log.i(TAG, "Android keeps its own keyboard shortcuts: " + error);
                }
            }
        });
    }
}
