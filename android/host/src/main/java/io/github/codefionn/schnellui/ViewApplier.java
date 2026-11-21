package io.github.codefionn.schnellui;

import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.animation.StateListAnimator;
import android.os.Build;
import android.text.Editable;
import android.text.InputType;
import android.text.Layout;
import android.text.TextUtils;
import android.text.TextWatcher;
import android.util.Log;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.accessibility.AccessibilityNodeInfo;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.SeekBar;
import android.widget.Space;
import android.widget.TextView;

import java.util.HashMap;
import java.util.Map;

/**
 * Applies decoded ops to platform views. Keeps the id to view map and one {@link Node} per
 * view. Android does the layout, so the applier only translates props into view calls and
 * layout params. Must run on the main thread.
 */
public final class ViewApplier {
    private static final String TAG = "SchnellUI";

    /** Per-view state. Layout props are kept here because they depend on the parent. */
    private static final class Node {
        final int id;
        final int kind;
        final View view;
        /** Where children go. Null for leaf kinds. The inner layout for Scroll. */
        final ViewGroup content;

        float width = Float.NaN;
        float height = Float.NaN;
        float minW = Float.NaN;
        float minH = Float.NaN;
        float maxW = Float.NaN;
        float maxH = Float.NaN;
        float grow = 0;
        float basis = Float.NaN;
        boolean fill;
        final float[] pad = {Float.NaN, Float.NaN, Float.NaN, Float.NaN};
        final int[] defPad = new int[4];
        final int defMinW;
        final int defMinH;
        float defTextPx;

        int justify;
        int align;
        float gap;

        int textWrap;
        boolean ellipsis;
        int role = -1;

        float value;
        float min;
        float max = 100;
        float step;

        boolean ghost;
        Drawable solidBg;
        ColorStateList solidText;
        StateListAnimator solidAnim;

        Node(int id, int kind, View view, ViewGroup content) {
            this.id = id;
            this.kind = kind;
            this.view = view;
            this.content = content;
            defPad[0] = view.getPaddingLeft();
            defPad[1] = view.getPaddingTop();
            defPad[2] = view.getPaddingRight();
            defPad[3] = view.getPaddingBottom();
            defMinW = view.getMinimumWidth();
            this.defMinH = view.getMinimumHeight();
            if (view instanceof TextView) {
                this.defTextPx = ((TextView) view).getTextSize();
            }
        }
    }

    private final Context context;
    private final ViewGroup host;
    private final EventGate gate;
    private final float density;
    private final Map<Integer, Node> nodes = new HashMap<>();
    private final IdTree tree = new IdTree();
    private int rootId;
    private Button ghostTemplate;

    public ViewApplier(Context context, ViewGroup host, EventGate gate) {
        this.context = context;
        this.host = host;
        this.gate = gate;
        this.density = context.getResources().getDisplayMetrics().density;
    }

    /** Drops every view and id. Call before applying a fresh full render. */
    public void reset() {
        host.removeAllViews();
        nodes.clear();
        tree.clear();
        rootId = 0;
    }

    /** Number of live nodes, for tests and diagnostics. */
    public int nodeCount() {
        return nodes.size();
    }

    /** Applies a buffer. A null buffer means nothing changed. */
    public void apply(byte[] buf) {
        if (buf == null) {
            return;
        }
        gate.begin();
        try {
            OpReader.read(buf, new Ops());
        } catch (DecodeException e) {
            Log.e(TAG, "bad op buffer: " + e.getMessage());
        } finally {
            gate.end();
        }
    }

    // Ops.

    private final class Ops implements OpReader.Handler {
        @Override
        public void create(int id, int kind) {
            createNode(id, kind);
        }

        @Override
        public void setStr(int id, int prop, String value) {
            Node n = node(id);
            if (n != null) {
                setString(n, prop, value);
            }
        }

        @Override
        public void setF32(int id, int prop, float value) {
            Node n = node(id);
            if (n != null) {
                setNumber(n, prop, value);
            }
        }

        @Override
        public void setI32(int id, int prop, int value) {
            Node n = node(id);
            if (n != null) {
                setNumber(n, prop, value);
            }
        }

        @Override
        public void clear(int id, int prop) {
            Node n = node(id);
            if (n != null) {
                clearProp(n, prop);
            }
        }

        @Override
        public void insert(int parent, int child, int index) {
            insertNode(parent, child, index);
        }

        @Override
        public void remove(int id) {
            removeNode(id);
        }

        @Override
        public void setRoot(int id) {
            setRootNode(id);
        }
    }

    private Node node(int id) {
        Node n = nodes.get(id);
        if (n == null) {
            Log.w(TAG, "op names unknown id " + id);
        }
        return n;
    }

    private void createNode(int id, int kind) {
        if (id == 0 || nodes.containsKey(id)) {
            Log.w(TAG, "CREATE with bad or reused id " + id);
            return;
        }
        View view;
        ViewGroup content = null;
        switch (kind) {
            case Codes.KIND_ROW: {
                FlexLinearLayout l = new FlexLinearLayout(context, LinearLayout.HORIZONTAL);
                view = l;
                content = l;
                break;
            }
            case Codes.KIND_COLUMN: {
                FlexLinearLayout l = new FlexLinearLayout(context, LinearLayout.VERTICAL);
                view = l;
                content = l;
                break;
            }
            case Codes.KIND_STACK: {
                FrameLayout f = new FrameLayout(context);
                view = f;
                content = f;
                break;
            }
            case Codes.KIND_SCROLL: {
                ScrollView s = new ScrollView(context);
                s.setFillViewport(true);
                s.setVerticalScrollBarEnabled(false);
                FlexLinearLayout inner = new FlexLinearLayout(context, LinearLayout.VERTICAL);
                s.addView(inner, new FrameLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT));
                view = s;
                content = inner;
                break;
            }
            case Codes.KIND_TEXT: {
                view = new TextView(context);
                break;
            }
            case Codes.KIND_BUTTON:
                view = new Button(context);
                break;
            case Codes.KIND_CHECKBOX:
                view = new CheckBox(context);
                break;
            case Codes.KIND_SLIDER:
                view = new SeekBar(context);
                break;
            case Codes.KIND_TEXT_INPUT: {
                EditText e = new EditText(context);
                e.setInputType(InputType.TYPE_CLASS_TEXT);
                view = e;
                break;
            }
            case Codes.KIND_SPACER:
                view = new Space(context);
                break;
            default:
                Log.w(TAG, "unknown view kind " + kind + " for id " + id + ", skipped");
                return;
        }
        Node n = new Node(id, kind, view, content);
        nodes.put(id, n);
        tree.create(id);
        initKind(n);
    }

    /** Sets the doc defaults that differ from the platform defaults, and the listeners. */
    private void initKind(final Node n) {
        final int id = n.id;
        switch (n.kind) {
            case Codes.KIND_TEXT:
                applyTextFlags(n);
                break;
            case Codes.KIND_BUTTON: {
                Button b = (Button) n.view;
                n.solidBg = b.getBackground();
                n.solidText = b.getTextColors();
                n.solidAnim = b.getStateListAnimator();
                b.setOnClickListener(v -> emit(id, Codes.EVENT_CLICK, 0, 0f, null));
                break;
            }
            case Codes.KIND_CHECKBOX:
                ((CheckBox) n.view).setOnCheckedChangeListener(
                        (b, checked) ->
                                emit(id, Codes.EVENT_TOGGLE, checked ? 1 : 0, 0f, null));
                break;
            case Codes.KIND_SLIDER: {
                SeekBar s = (SeekBar) n.view;
                syncSlider(n);
                s.setOnSeekBarChangeListener(new SeekBar.OnSeekBarChangeListener() {
                    @Override
                    public void onProgressChanged(SeekBar bar, int progress, boolean fromUser) {
                        if (fromUser) {
                            // Rust records the reported value, so keep ours in step.
                            float v = SliderMath.valueFor(progress, n.min, n.max, n.step);
                            n.value = v;
                            emit(id, Codes.EVENT_CHANGE, 0, v, null);
                        }
                    }

                    @Override
                    public void onStartTrackingTouch(SeekBar bar) {}

                    @Override
                    public void onStopTrackingTouch(SeekBar bar) {}
                });
                break;
            }
            case Codes.KIND_TEXT_INPUT:
                ((EditText) n.view).addTextChangedListener(new TextWatcher() {
                    @Override
                    public void beforeTextChanged(CharSequence s, int a, int b, int c) {}

                    @Override
                    public void onTextChanged(CharSequence s, int a, int b, int c) {}

                    @Override
                    public void afterTextChanged(Editable e) {
                        emit(id, Codes.EVENT_INPUT, 0, 0f, e.toString());
                    }
                });
                break;
            default:
                break;
        }
    }

    /** Sends an event unless ops are being applied, then applies the diff that returns. */
    private void emit(int id, int event, int i, float f, String s) {
        byte[] diff = gate.fire(id, event, i, f, s);
        if (diff != null) {
            apply(diff);
        }
    }

    private void insertNode(int parent, int child, int index) {
        Node p = node(parent);
        Node c = node(child);
        if (p == null || c == null) {
            return;
        }
        if (p.content == null) {
            Log.w(TAG, "INSERT into a leaf view, id " + parent);
            return;
        }
        for (int a = parent; a != 0; a = tree.parentOf(a)) {
            if (a == child) {
                Log.w(TAG, "INSERT would make " + child + " its own ancestor");
                return;
            }
        }
        ViewGroup old = (ViewGroup) c.view.getParent();
        if (old != null) {
            old.removeView(c.view);
        }
        if (rootId == child) {
            rootId = 0;
        }
        int at = tree.insert(parent, child, index);
        at = Math.min(at, p.content.getChildCount());
        applyLayout(c);
        p.content.addView(c.view, at);
        if (p.content instanceof FlexLinearLayout) {
            // Gap dividers and spread depend on the child list. Nothing else to do.
            p.content.requestLayout();
        }
    }

    private void removeNode(int id) {
        Node n = node(id);
        if (n == null) {
            return;
        }
        ViewGroup old = (ViewGroup) n.view.getParent();
        if (old != null) {
            old.removeView(n.view);
        }
        if (rootId == id) {
            rootId = 0;
        }
        for (int gone : tree.remove(id)) {
            nodes.remove(gone);
        }
    }

    private void setRootNode(int id) {
        Node n = node(id);
        if (n == null) {
            return;
        }
        ViewGroup old = (ViewGroup) n.view.getParent();
        if (old != null) {
            old.removeView(n.view);
        }
        tree.detach(id);
        host.removeAllViews();
        rootId = id;
        applyLayout(n);
        host.addView(n.view);
    }

    // Props.

    private void setString(Node n, int prop, String value) {
        switch (prop) {
            case Codes.PROP_TEXT:
                setText(n, value);
                break;
            case Codes.PROP_A11Y_LABEL:
                setLabel(n, value);
                break;
            default:
                Log.w(TAG, "unknown string prop " + prop + ", skipped");
        }
    }

    private void setText(Node n, String value) {
        if (!(n.view instanceof TextView)) {
            skipProp(n, Codes.PROP_TEXT);
            return;
        }
        TextView t = (TextView) n.view;
        if (value.contentEquals(t.getText())) {
            return;
        }
        t.setText(value);
        if (t instanceof EditText) {
            ((EditText) t).setSelection(t.length());
        }
    }

    private void setLabel(Node n, String value) {
        if (value == null) {
            n.view.setContentDescription(null);
            if (n.view instanceof EditText) {
                ((EditText) n.view).setHint(null);
            }
            return;
        }
        n.view.setContentDescription(value);
        if (n.view instanceof EditText) {
            ((EditText) n.view).setHint(value);
        }
    }

    private void clearProp(Node n, int prop) {
        switch (prop) {
            case Codes.PROP_WIDTH:
            case Codes.PROP_HEIGHT:
            case Codes.PROP_MIN_WIDTH:
            case Codes.PROP_MIN_HEIGHT:
            case Codes.PROP_MAX_WIDTH:
            case Codes.PROP_MAX_HEIGHT:
            case Codes.PROP_BASIS:
            case Codes.PROP_PAD_LEFT:
            case Codes.PROP_PAD_TOP:
            case Codes.PROP_PAD_RIGHT:
            case Codes.PROP_PAD_BOTTOM:
            case Codes.PROP_TEXT_SIZE:
                setNumber(n, prop, Float.NaN);
                break;
            case Codes.PROP_VISIBLE:
            case Codes.PROP_ENABLED:
                setNumber(n, prop, 1);
                break;
            case Codes.PROP_MAX:
                setNumber(n, prop, 100);
                break;
            case Codes.PROP_ROLE:
                setNumber(n, prop, -1);
                break;
            case Codes.PROP_TEXT:
                setString(n, prop, "");
                break;
            case Codes.PROP_A11Y_LABEL:
                setLabel(n, null);
                break;
            case Codes.PROP_FILL:
            case Codes.PROP_GROW:
            case Codes.PROP_JUSTIFY:
            case Codes.PROP_ALIGN:
            case Codes.PROP_GAP:
            case Codes.PROP_WRAP:
            case Codes.PROP_SCROLLBAR:
            case Codes.PROP_TEXT_WRAP:
            case Codes.PROP_TEXT_ALIGN:
            case Codes.PROP_ELLIPSIS:
            case Codes.PROP_CHECKED:
            case Codes.PROP_VALUE:
            case Codes.PROP_MIN:
            case Codes.PROP_STEP:
            case Codes.PROP_PASSWORD:
            case Codes.PROP_APPEARANCE:
                setNumber(n, prop, 0);
                break;
            default:
                Log.w(TAG, "unknown prop " + prop + " in CLEAR, skipped");
        }
    }

    /** Float, integer, enum and bool props all arrive here. NaN means "unset". */
    private void setNumber(Node n, int prop, float v) {
        boolean on = v != 0;
        switch (prop) {
            case Codes.PROP_WIDTH:
                n.width = v;
                applyLayout(n);
                break;
            case Codes.PROP_HEIGHT:
                n.height = v;
                applyLayout(n);
                break;
            case Codes.PROP_MIN_WIDTH:
                n.minW = v;
                applyLayout(n);
                break;
            case Codes.PROP_MIN_HEIGHT:
                n.minH = v;
                applyLayout(n);
                break;
            case Codes.PROP_MAX_WIDTH:
                n.maxW = v;
                applyLayout(n);
                break;
            case Codes.PROP_MAX_HEIGHT:
                n.maxH = v;
                applyLayout(n);
                break;
            case Codes.PROP_FILL:
                n.fill = on;
                applyLayout(n);
                break;
            case Codes.PROP_GROW:
                n.grow = v;
                applyLayout(n);
                break;
            case Codes.PROP_BASIS:
                n.basis = v;
                applyLayout(n);
                break;
            case Codes.PROP_PAD_LEFT:
            case Codes.PROP_PAD_TOP:
            case Codes.PROP_PAD_RIGHT:
            case Codes.PROP_PAD_BOTTOM:
                n.pad[prop - Codes.PROP_PAD_LEFT] = v;
                applyPadding(n);
                break;
            case Codes.PROP_VISIBLE: {
                int want = on ? View.VISIBLE : View.GONE;
                if (n.view.getVisibility() != want) {
                    n.view.setVisibility(want);
                }
                break;
            }
            case Codes.PROP_ENABLED:
                if (n.view.isEnabled() != on) {
                    n.view.setEnabled(on);
                }
                break;
            case Codes.PROP_JUSTIFY:
                if (isLinear(n)) {
                    n.justify = (int) v;
                    applyContainer(n);
                } else {
                    skipProp(n, prop);
                }
                break;
            case Codes.PROP_ALIGN:
                if (n.kind == Codes.KIND_ROW || n.kind == Codes.KIND_COLUMN
                        || n.kind == Codes.KIND_STACK) {
                    n.align = (int) v;
                    relayoutChildren(n);
                } else {
                    skipProp(n, prop);
                }
                break;
            case Codes.PROP_GAP:
                if (isLinear(n)) {
                    n.gap = v;
                    applyContainer(n);
                } else {
                    skipProp(n, prop);
                }
                break;
            case Codes.PROP_WRAP:
                // Hosts may ignore WRAP for now.
                break;
            case Codes.PROP_SCROLLBAR:
                if (n.kind == Codes.KIND_SCROLL) {
                    n.view.setVerticalScrollBarEnabled(on);
                } else {
                    skipProp(n, prop);
                }
                break;
            case Codes.PROP_TEXT_SIZE:
                if (n.view instanceof TextView) {
                    TextView t = (TextView) n.view;
                    if (Float.isNaN(v)) {
                        t.setTextSize(TypedValue.COMPLEX_UNIT_PX, n.defTextPx);
                    } else {
                        t.setTextSize(TypedValue.COMPLEX_UNIT_SP, v);
                    }
                } else {
                    skipProp(n, prop);
                }
                break;
            case Codes.PROP_TEXT_WRAP:
                if (n.kind == Codes.KIND_TEXT) {
                    n.textWrap = (int) v;
                    applyTextFlags(n);
                } else {
                    skipProp(n, prop);
                }
                break;
            case Codes.PROP_TEXT_ALIGN:
                if (n.kind == Codes.KIND_TEXT) {
                    applyTextAlign((TextView) n.view, (int) v);
                } else {
                    skipProp(n, prop);
                }
                break;
            case Codes.PROP_ELLIPSIS:
                if (n.kind == Codes.KIND_TEXT) {
                    n.ellipsis = on;
                    applyTextFlags(n);
                } else {
                    skipProp(n, prop);
                }
                break;
            case Codes.PROP_ROLE:
                n.role = (int) v;
                applyRole(n);
                break;
            case Codes.PROP_CHECKED:
                if (n.kind == Codes.KIND_CHECKBOX) {
                    CheckBox c = (CheckBox) n.view;
                    if (c.isChecked() != on) {
                        c.setChecked(on);
                    }
                } else {
                    skipProp(n, prop);
                }
                break;
            case Codes.PROP_VALUE:
            case Codes.PROP_MIN:
            case Codes.PROP_MAX:
            case Codes.PROP_STEP:
                if (n.kind == Codes.KIND_SLIDER) {
                    if (prop == Codes.PROP_VALUE) {
                        n.value = v;
                    } else if (prop == Codes.PROP_MIN) {
                        n.min = v;
                    } else if (prop == Codes.PROP_MAX) {
                        n.max = v;
                    } else {
                        n.step = v;
                    }
                    syncSlider(n);
                } else {
                    skipProp(n, prop);
                }
                break;
            case Codes.PROP_PASSWORD:
                if (n.kind == Codes.KIND_TEXT_INPUT) {
                    applyPassword((EditText) n.view, on);
                } else {
                    skipProp(n, prop);
                }
                break;
            case Codes.PROP_APPEARANCE:
                if (n.kind == Codes.KIND_BUTTON) {
                    applyAppearance(n, v != 0);
                } else {
                    skipProp(n, prop);
                }
                break;
            default:
                Log.w(TAG, "unknown prop " + prop + ", skipped");
        }
    }

    private void skipProp(Node n, int prop) {
        Log.w(TAG, "prop " + prop + " does not apply to kind " + n.kind + ", skipped");
    }

    private static boolean isLinear(Node n) {
        return n.kind == Codes.KIND_ROW || n.kind == Codes.KIND_COLUMN;
    }

    // Prop helpers.

    private int px(float dp) {
        return Math.round(dp * density);
    }

    private void applyPadding(Node n) {
        int[] p = new int[4];
        for (int i = 0; i < 4; i++) {
            p[i] = Float.isNaN(n.pad[i]) ? n.defPad[i] : px(n.pad[i]);
        }
        n.view.setPadding(p[0], p[1], p[2], p[3]);
    }

    private void applyTextFlags(Node n) {
        TextView t = (TextView) n.view;
        boolean noWrap = n.textWrap == 0;
        t.setMaxLines(noWrap ? 1 : Integer.MAX_VALUE);
        t.setHorizontallyScrolling(noWrap && !n.ellipsis);
        t.setEllipsize(n.ellipsis ? TextUtils.TruncateAt.END : null);
    }

    private void applyTextAlign(TextView t, int align) {
        int h;
        switch (align) {
            case 1:
                h = Gravity.CENTER_HORIZONTAL;
                break;
            case 2:
                h = Gravity.END;
                break;
            default:
                h = Gravity.START;
        }
        t.setGravity(h | Gravity.TOP);
        if (Build.VERSION.SDK_INT >= 26) {
            t.setJustificationMode(align == 3
                    ? Layout.JUSTIFICATION_MODE_INTER_WORD
                    : Layout.JUSTIFICATION_MODE_NONE);
        }
    }

    private void applyPassword(EditText e, boolean password) {
        int want = InputType.TYPE_CLASS_TEXT
                | (password ? InputType.TYPE_TEXT_VARIATION_PASSWORD : 0);
        if (e.getInputType() == want) {
            return;
        }
        e.setInputType(want);
        // Switching the input type resets the typeface to monospace for passwords.
        e.setTypeface(android.graphics.Typeface.DEFAULT);
    }

    private void applyRole(final Node n) {
        if (n.role < 0) {
            if (Build.VERSION.SDK_INT >= 28) {
                n.view.setAccessibilityHeading(false);
            }
            n.view.setAccessibilityDelegate(null);
            return;
        }
        final boolean heading = Roles.isHeading(n.role);
        final String cls = Roles.className(n.role);
        if (Build.VERSION.SDK_INT >= 28) {
            n.view.setAccessibilityHeading(heading);
        }
        if (cls == null && !heading) {
            n.view.setAccessibilityDelegate(null);
            return;
        }
        n.view.setAccessibilityDelegate(new View.AccessibilityDelegate() {
            @Override
            public void onInitializeAccessibilityNodeInfo(View v, AccessibilityNodeInfo info) {
                super.onInitializeAccessibilityNodeInfo(v, info);
                if (cls != null) {
                    info.setClassName(cls);
                }
                if (heading && Build.VERSION.SDK_INT >= 28) {
                    info.setHeading(true);
                }
            }
        });
    }

    /** Pushes slider state onto the SeekBar. Equal values are not set again. */
    private void syncSlider(Node n) {
        SeekBar s = (SeekBar) n.view;
        int max = SliderMath.positions(n.min, n.max, n.step);
        if (s.getMax() != max) {
            s.setMax(max);
        }
        int progress = SliderMath.progressFor(n.value, n.min, n.max, n.step);
        if (s.getProgress() != progress) {
            s.setProgress(progress);
        }
    }

    private void applyAppearance(Node n, boolean ghost) {
        if (n.ghost == ghost) {
            return;
        }
        n.ghost = ghost;
        Button b = (Button) n.view;
        if (ghost) {
            if (ghostTemplate == null) {
                ghostTemplate = new Button(context, null, android.R.attr.borderlessButtonStyle);
            }
            Drawable bg = ghostTemplate.getBackground();
            Drawable.ConstantState cs = bg == null ? null : bg.getConstantState();
            if (cs != null) {
                b.setBackground(cs.newDrawable(context.getResources()));
            } else {
                b.setBackgroundColor(Color.TRANSPARENT);
            }
            b.setTextColor(ghostTemplate.getTextColors());
            b.setStateListAnimator(null);
            n.defPad[0] = ghostTemplate.getPaddingLeft();
            n.defPad[1] = ghostTemplate.getPaddingTop();
            n.defPad[2] = ghostTemplate.getPaddingRight();
            n.defPad[3] = ghostTemplate.getPaddingBottom();
        } else {
            b.setBackground(n.solidBg);
            b.setTextColor(n.solidText);
            b.setStateListAnimator(n.solidAnim);
            Button plain = new Button(context);
            n.defPad[0] = plain.getPaddingLeft();
            n.defPad[1] = plain.getPaddingTop();
            n.defPad[2] = plain.getPaddingRight();
            n.defPad[3] = plain.getPaddingBottom();
        }
        applyPadding(n);
    }

    // Containers and layout.

    private void applyContainer(Node n) {
        if (!(n.content instanceof FlexLinearLayout)) {
            return;
        }
        FlexLinearLayout l = (FlexLinearLayout) n.content;
        boolean horizontal = l.getOrientation() == LinearLayout.HORIZONTAL;
        int main;
        switch (n.justify) {
            case 1:
                main = horizontal ? Gravity.CENTER_HORIZONTAL : Gravity.CENTER_VERTICAL;
                break;
            case 2:
                main = horizontal ? Gravity.END : Gravity.BOTTOM;
                break;
            default:
                main = horizontal ? Gravity.START : Gravity.TOP;
        }
        l.setGravity(main);
        l.setSpaceMode(n.justify >= 3 && n.justify <= 5 ? n.justify : 0);
        int gap = px(Math.max(0, n.gap));
        if (gap > 0) {
            GradientDrawable d = new GradientDrawable();
            d.setSize(gap, gap);
            l.setDividerDrawable(d);
            l.setShowDividers(LinearLayout.SHOW_DIVIDER_MIDDLE);
        } else {
            l.setShowDividers(LinearLayout.SHOW_DIVIDER_NONE);
            l.setDividerDrawable(null);
        }
    }

    private void relayoutChildren(Node n) {
        for (int child : tree.childrenOf(n.id)) {
            Node c = nodes.get(child);
            if (c != null) {
                applyLayout(c);
            }
        }
    }

    /** Computes layout params from the node's props and its current parent. */
    private void applyLayout(Node n) {
        if (n.minW != n.minW) {
            n.view.setMinimumWidth(n.defMinW);
        } else {
            n.view.setMinimumWidth(px(n.minW));
        }
        if (n.minH != n.minH) {
            n.view.setMinimumHeight(n.defMinH);
        } else {
            n.view.setMinimumHeight(px(n.minH));
        }
        int parentId = tree.parentOf(n.id);
        Node p = parentId == 0 ? null : nodes.get(parentId);
        if (p == null && n.id != rootId) {
            return; // Detached. INSERT or SET_ROOT will call this again.
        }
        ViewGroup content = p == null ? null : p.content;
        ViewGroup.LayoutParams lp;
        if (content instanceof LinearLayout) {
            lp = linearParams(n, p, (LinearLayout) content);
        } else {
            lp = frameParams(n, p);
        }
        n.view.setLayoutParams(lp);
    }

    /** Width of a Slider or TextInput that has no WIDTH, MIN_WIDTH, FILL or GROW. */
    private static final float DEFAULT_FIELD_WIDTH_DP = 160f;

    private static float clampMax(float v, float max) {
        return Float.isNaN(max) ? v : Math.min(v, max);
    }

    private int size(Node n, boolean horizontalAxis, boolean main, boolean stretch) {
        float explicit = horizontalAxis ? n.width : n.height;
        float max = horizontalAxis ? n.maxW : n.maxH;
        if (main) {
            if (!Float.isNaN(n.basis)) {
                return px(clampMax(n.basis, max));
            }
            if (!Float.isNaN(explicit)) {
                return px(clampMax(explicit, max));
            }
            if (n.grow > 0) {
                return 0;
            }
        } else if (!Float.isNaN(explicit)) {
            return px(clampMax(explicit, max));
        }
        if (n.fill || stretch) {
            return ViewGroup.LayoutParams.MATCH_PARENT;
        }
        boolean slim = n.kind == Codes.KIND_SLIDER || n.kind == Codes.KIND_TEXT_INPUT;
        if (horizontalAxis && slim && Float.isNaN(n.minW)) {
            // A wrap-content SeekBar is as wide as its thumb, and an empty EditText is
            // a sliver. Both get a usable width until the app sets one.
            return px(clampMax(DEFAULT_FIELD_WIDTH_DP, max));
        }
        return ViewGroup.LayoutParams.WRAP_CONTENT;
    }

    private LinearLayout.LayoutParams linearParams(Node n, Node p, LinearLayout ll) {
        boolean horizontal = ll.getOrientation() == LinearLayout.HORIZONTAL;
        // Scroll's own ALIGN does not apply, so its inner layout always aligns to the start.
        int align = p.kind == Codes.KIND_SCROLL ? 0 : p.align;
        boolean stretch = align == 3;
        int w = size(n, true, horizontal, !horizontal && stretch);
        int h = size(n, false, !horizontal, horizontal && stretch);
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(w, h);
        lp.weight = n.grow > 0 ? n.grow : 0;
        if (align == 1) {
            lp.gravity = horizontal ? Gravity.CENTER_VERTICAL : Gravity.CENTER_HORIZONTAL;
        } else if (align == 2) {
            lp.gravity = horizontal ? Gravity.BOTTOM : Gravity.END;
        } else {
            lp.gravity = horizontal ? Gravity.TOP : Gravity.START;
        }
        return lp;
    }

    private FrameLayout.LayoutParams frameParams(Node n, Node p) {
        int align = p == null ? 0 : p.align;
        boolean stretch = align == 3;
        FrameLayout.LayoutParams lp = new FrameLayout.LayoutParams(
                size(n, true, false, stretch), size(n, false, false, stretch));
        if (align == 1) {
            lp.gravity = Gravity.CENTER;
        } else if (align == 2) {
            lp.gravity = Gravity.BOTTOM | Gravity.END;
        } else {
            lp.gravity = Gravity.TOP | Gravity.START;
        }
        return lp;
    }
}
