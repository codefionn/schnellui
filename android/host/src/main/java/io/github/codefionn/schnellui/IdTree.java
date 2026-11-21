package io.github.codefionn.schnellui;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Parent and child bookkeeping by node id, without any views. The applier keeps one next to
 * its id to view map so that moves and subtree removal can be tested on the JVM.
 */
public final class IdTree {
    private static final class Entry {
        int parent;
        final List<Integer> children = new ArrayList<>();
    }

    private final Map<Integer, Entry> entries = new HashMap<>();

    public boolean contains(int id) {
        return entries.containsKey(id);
    }

    public int size() {
        return entries.size();
    }

    public void create(int id) {
        entries.put(id, new Entry());
    }

    /** The parent id, or 0 when the node is detached or unknown. */
    public int parentOf(int id) {
        Entry e = entries.get(id);
        return e == null ? 0 : e.parent;
    }

    /** True when the node is the root or has it as an ancestor. Detached nodes are not under it. */
    public boolean isUnder(int id, int root) {
        if (root == 0 || !entries.containsKey(id)) {
            return false;
        }
        for (int a = id; a != 0; a = parentOf(a)) {
            if (a == root) {
                return true;
            }
        }
        return false;
    }

    public List<Integer> childrenOf(int id) {
        Entry e = entries.get(id);
        return e == null ? new ArrayList<>() : new ArrayList<>(e.children);
    }

    /**
     * Attaches the child so it sits at {@code index} among the parent's children after the
     * call. An already attached child is moved. The index is clamped to the child count.
     *
     * @return the index the child ended up at
     */
    public int insert(int parent, int child, int index) {
        Entry p = entries.get(parent);
        Entry c = entries.get(child);
        if (p == null || c == null) {
            throw new IllegalArgumentException("unknown id");
        }
        detach(child, c);
        int at = Math.max(0, Math.min(index, p.children.size()));
        p.children.add(at, child);
        c.parent = parent;
        return at;
    }

    /** Detaches the node from its parent but keeps it. */
    public void detach(int id) {
        Entry e = entries.get(id);
        if (e != null) {
            detach(id, e);
        }
    }

    /** Detaches the node and forgets it and all descendants. Returns every forgotten id. */
    public List<Integer> remove(int id) {
        List<Integer> gone = new ArrayList<>();
        Entry e = entries.get(id);
        if (e == null) {
            return gone;
        }
        detach(id, e);
        collect(id, gone);
        return gone;
    }

    /** Forgets everything. */
    public void clear() {
        entries.clear();
    }

    private void collect(int id, List<Integer> out) {
        Entry e = entries.remove(id);
        out.add(id);
        if (e != null) {
            for (int child : e.children) {
                collect(child, out);
            }
        }
    }

    private void detach(int id, Entry e) {
        if (e.parent != 0) {
            Entry old = entries.get(e.parent);
            if (old != null) {
                old.children.remove(Integer.valueOf(id));
            }
            e.parent = 0;
        }
    }
}
