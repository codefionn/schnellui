package io.github.codefionn.schnellui;

import android.app.Activity;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.graphics.Color;
import android.os.Build;
import android.os.Bundle;
import android.util.Log;
import android.util.TypedValue;
import android.view.View;
import android.view.WindowInsets;
import android.widget.FrameLayout;
import android.widget.TextView;

/**
 * Hosts a native schnellui app in platform views. The library to load comes from the
 * {@code schnellui.lib} meta-data entry of the activity, for example {@code android_native}.
 *
 * <p>Declare {@code android:configChanges} for orientation, screen size, density and uiMode
 * on the activity so a change reaches {@link #onConfigurationChanged} and the views survive.
 */
public class SchnellActivity extends Activity {
    private static final String TAG = "SchnellUI";
    private static final String META_LIB = "schnellui.lib";

    private FrameLayout container;
    private ViewApplier applier;
    private long app;
    private boolean dark;
    private float lastW = -1;
    private float lastH = -1;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        container = new FrameLayout(this);
        setContentView(container);
        container.setOnApplyWindowInsetsListener(SchnellActivity::applyInsets);
        container.addOnLayoutChangeListener(
                (v, l, t, r, b, ol, ot, or, ob) -> onContainerSize());

        String lib = readLibName();
        if (lib == null) {
            showError("Missing meta-data " + META_LIB + " in the manifest.");
            return;
        }
        try {
            SchnellBridge.load(lib);
        } catch (UnsatisfiedLinkError e) {
            Log.e(TAG, "cannot load lib" + lib + ".so", e);
            showError("Cannot load lib" + lib + ".so.\nBuild it with android/build-rust.sh.");
            return;
        }
        mount();
    }

    private String readLibName() {
        try {
            ActivityInfo info = getPackageManager().getActivityInfo(
                    getComponentName(), PackageManager.GET_META_DATA);
            return info.metaData == null ? null : info.metaData.getString(META_LIB);
        } catch (PackageManager.NameNotFoundException e) {
            return null;
        }
    }

    private void showError(String message) {
        TextView t = new TextView(this);
        t.setText(message);
        t.setTextSize(TypedValue.COMPLEX_UNIT_SP, 16);
        t.setPadding(48, 96, 48, 48);
        container.addView(t);
    }

    private boolean isDark() {
        int night = getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK;
        return night == Configuration.UI_MODE_NIGHT_YES;
    }

    private void mount() {
        dark = isDark();
        float density = getResources().getDisplayMetrics().density;
        Configuration c = getResources().getConfiguration();
        lastW = c.screenWidthDp;
        lastH = c.screenHeightDp;
        app = SchnellBridge.nativeMount(lastW, lastH, density, dark);
        applier = new ViewApplier(this, container, new EventGate(SchnellBridge.sinkFor(app)));
        applier.apply(SchnellBridge.nativeRender(app));
    }

    private void unmount() {
        if (app != 0) {
            SchnellBridge.nativeUnmount(app);
            app = 0;
        }
        if (applier != null) {
            applier.reset();
            applier = null;
        }
    }

    /** Reports the content size, minus the insets padding, in dp. */
    private void onContainerSize() {
        if (app == 0) {
            return;
        }
        float density = getResources().getDisplayMetrics().density;
        int w = container.getWidth() - container.getPaddingLeft() - container.getPaddingRight();
        int h = container.getHeight() - container.getPaddingTop() - container.getPaddingBottom();
        if (w <= 0 || h <= 0) {
            return;
        }
        resize(w / density, h / density);
    }

    private void resize(float wDp, float hDp) {
        if (Math.abs(wDp - lastW) < 0.01f && Math.abs(hDp - lastH) < 0.01f) {
            return;
        }
        lastW = wDp;
        lastH = hDp;
        applier.apply(SchnellBridge.nativeResize(app, wDp, hDp));
    }

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        if (app == 0) {
            return;
        }
        if (isDark() != dark) {
            // The theme is fixed at mount, so a night mode change builds the app again.
            unmount();
            mount();
        } else {
            onContainerSize();
        }
    }

    @Override
    protected void onDestroy() {
        unmount();
        super.onDestroy();
    }

    /** Keeps content out of the system bars, display cutout and the keyboard. */
    private static WindowInsets applyInsets(View v, WindowInsets insets) {
        int l;
        int t;
        int r;
        int b;
        if (Build.VERSION.SDK_INT >= 30) {
            android.graphics.Insets i = insets.getInsets(
                    WindowInsets.Type.systemBars()
                            | WindowInsets.Type.displayCutout()
                            | WindowInsets.Type.ime());
            l = i.left;
            t = i.top;
            r = i.right;
            b = i.bottom;
        } else {
            l = insets.getSystemWindowInsetLeft();
            t = insets.getSystemWindowInsetTop();
            r = insets.getSystemWindowInsetRight();
            b = insets.getSystemWindowInsetBottom();
        }
        v.setPadding(l, t, r, b);
        return Build.VERSION.SDK_INT >= 30 ? WindowInsets.CONSUMED : insets.consumeSystemWindowInsets();
    }
}
