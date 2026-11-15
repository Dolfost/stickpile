package io.github.dolfost.stickpile.ui.common

import androidx.compose.ui.Modifier
import androidx.compose.ui.node.ModifierNodeElement
import androidx.compose.ui.node.requireView
import androidx.compose.ui.platform.InspectorInfo
import androidx.core.view.doOnAttach

/**
 * Drops the touches that reach this window through another app's window
 * drawn over it (`View.filterTouchesWhenObscured`), so that an overlay
 * cannot trick the user into a tap (tapjacking). Compose's own view does
 * not apply the flag (its `dispatchTouchEvent` skips the check), so it is
 * set on the root view of the window the element is in: for an element
 * of a dialog (an `AlertDialog`'s confirm button), the dialog window's
 * decor view, which covers the whole dialog and nothing else. Touches the
 * flag cannot see (an overlay elsewhere on the screen) are not dropped.
 */
fun Modifier.filterTouchesWhenObscured(): Modifier = this then FilterTouchesWhenObscuredElement

private data object FilterTouchesWhenObscuredElement : ModifierNodeElement<FilterTouchesWhenObscuredNode>() {
    override fun create() = FilterTouchesWhenObscuredNode()

    override fun update(node: FilterTouchesWhenObscuredNode) {}

    override fun InspectorInfo.inspectableProperties() {
        name = "filterTouchesWhenObscured"
    }
}

private class FilterTouchesWhenObscuredNode : Modifier.Node() {
    override fun onAttach() {
        // Attached to its window, the view's root is that window's decor view.
        requireView().doOnAttach { view -> view.rootView.filterTouchesWhenObscured = true }
    }
}
