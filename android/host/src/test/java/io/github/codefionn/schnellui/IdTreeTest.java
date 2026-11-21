package io.github.codefionn.schnellui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.util.Arrays;
import java.util.List;
import org.junit.Test;

public class IdTreeTest {
    private static IdTree withIds(int... ids) {
        IdTree t = new IdTree();
        for (int id : ids) {
            t.create(id);
        }
        return t;
    }

    @Test
    public void insertAtIndex() {
        IdTree t = withIds(1, 2, 3, 4);
        t.insert(1, 2, 0);
        t.insert(1, 3, 1);
        t.insert(1, 4, 1);
        assertEquals(Arrays.asList(2, 4, 3), t.childrenOf(1));
        assertEquals(1, t.parentOf(4));
    }

    @Test
    public void insertClampsIndex() {
        IdTree t = withIds(1, 2);
        assertEquals(0, t.insert(1, 2, 99));
    }

    @Test
    public void insertMovesAttachedChildWithinParent() {
        IdTree t = withIds(1, 2, 3, 4);
        t.insert(1, 2, 0);
        t.insert(1, 3, 1);
        t.insert(1, 4, 2);
        t.insert(1, 2, 2);
        assertEquals(Arrays.asList(3, 4, 2), t.childrenOf(1));
    }

    @Test
    public void insertMovesChildToAnotherParent() {
        IdTree t = withIds(1, 2, 3);
        t.insert(1, 3, 0);
        t.insert(2, 3, 0);
        assertTrue(t.childrenOf(1).isEmpty());
        assertEquals(Arrays.asList(3), t.childrenOf(2));
        assertEquals(2, t.parentOf(3));
    }

    @Test
    public void removeForgetsWholeSubtree() {
        IdTree t = withIds(1, 2, 3, 4, 5);
        t.insert(1, 2, 0);
        t.insert(2, 3, 0);
        t.insert(3, 4, 0);
        t.insert(1, 5, 1);
        List<Integer> gone = t.remove(2);
        assertEquals(3, gone.size());
        assertTrue(gone.containsAll(Arrays.asList(2, 3, 4)));
        assertFalse(t.contains(2));
        assertFalse(t.contains(3));
        assertFalse(t.contains(4));
        assertTrue(t.contains(1));
        assertTrue(t.contains(5));
        assertEquals(Arrays.asList(5), t.childrenOf(1));
        assertEquals(2, t.size());
    }

    @Test
    public void removeUnknownIsEmpty() {
        assertTrue(new IdTree().remove(7).isEmpty());
    }

    @Test
    public void detachKeepsNode() {
        IdTree t = withIds(1, 2);
        t.insert(1, 2, 0);
        t.detach(2);
        assertTrue(t.contains(2));
        assertEquals(0, t.parentOf(2));
        assertTrue(t.childrenOf(1).isEmpty());
    }

    @Test
    public void isUnderWalksParentsToTheRoot() {
        IdTree t = withIds(1, 2, 3, 4);
        t.insert(1, 2, 0);
        t.insert(2, 3, 0);
        assertTrue(t.isUnder(3, 1));
        assertTrue(t.isUnder(1, 1));
        assertFalse(t.isUnder(4, 1)); // Detached.
        assertFalse(t.isUnder(3, 0));
        assertFalse(t.isUnder(9, 1)); // Unknown.
        t.detach(2);
        assertFalse(t.isUnder(3, 1));
    }

    @Test
    public void removingAnAncestorForgetsADialogSubtree() {
        // 1 root, 2 column, 3 dialog, 4 dialog content.
        IdTree t = withIds(1, 2, 3, 4);
        t.insert(1, 2, 0);
        t.insert(2, 3, 0);
        t.insert(3, 4, 0);
        assertTrue(t.isUnder(3, 1));
        List<Integer> gone = t.remove(2);
        assertTrue(gone.containsAll(Arrays.asList(2, 3, 4)));
        assertFalse(t.contains(3));
    }
}
