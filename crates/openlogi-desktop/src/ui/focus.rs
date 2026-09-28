//! The focus lifecycle: where keyboard focus lands when a screen changes, and
//! what catches it when the focused element disappears.
//!
//! GPUI dispatches every keystroke from the focused element's path. When that
//! element leaves the tree — a route change, a closed panel, a device going
//! offline — the window still holds its id, dispatch falls back to the window
//! root, and every context-bound binding (Tab, Back, Escape) goes silent. A
//! screen reader is left on the bare window. So focus is never left to chance:
//!
//! - a transition that removes the focused element names its landing target
//!   and moves focus there itself (`AppView::navigate`), and
//! - [`keep_focus`] catches any removal nobody declared.

use std::collections::HashMap;
use std::hash::Hash;

use gpui::{App, Context, FocusHandle, Subscription, Window};

/// One stable [`FocusHandle`] per domain identity, for controls rebuilt every
/// frame (device cards, section tabs) that a transition must be able to focus
/// by name. Handles are created on first use and live as long as the owner.
pub(crate) struct FocusTargets<K> {
    handles: HashMap<K, FocusHandle>,
}

impl<K> Default for FocusTargets<K> {
    fn default() -> Self {
        Self {
            handles: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash + Clone> FocusTargets<K> {
    /// The handle for `key`, created on first use.
    pub(crate) fn handle(&mut self, key: &K, cx: &App) -> FocusHandle {
        self.handles
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }
}

/// Keep keyboard focus inside `cx`'s window when the focused element is
/// removed without a declared landing target.
///
/// `landing` names the element that should take focus now; it must return a
/// handle only for an element the next frame renders. `None` falls back to
/// `root`, the handle the window's root view tracks on its always-rendered
/// root element, so focus is never left on nothing and the window's bindings
/// stay live. GPUI calls this after the frame in which focus was lost, and
/// does not re-fire it for a move the listener itself makes.
pub(crate) fn keep_focus<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    root: FocusHandle,
    landing: impl Fn(&mut T, &mut Window, &mut Context<T>) -> Option<FocusHandle> + 'static,
) -> Subscription {
    cx.on_focus_lost(window, move |view, window, cx| {
        let target = landing(view, window, cx).unwrap_or_else(|| root.clone());
        target.focus(window, cx);
    })
}

/// [`keep_focus`] for a window with no landing target of its own: focus
/// returns to its root, from which Tab reaches the first control.
pub(crate) fn keep_focus_on_root<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    root: FocusHandle,
) -> Subscription {
    keep_focus(window, cx, root, |_, _, _| None)
}

/// Where a key moves the selection of a tab list of `count` tabs from the
/// tab at `current`, or `None` when the key is not a tab-list key.
///
/// A tab list is one Tab stop — the selected tab — and the arrows move
/// between its tabs, selecting each as focus arrives (automatic activation),
/// so the screen reader announces the tab and the page under it together.
/// Both axes are honoured whatever the list's orientation: a screen-reader
/// user cannot see which way the tabs run. Movement stops at the ends
/// rather than wrapping, and Home and End jump to them. Modified keys are
/// left alone, so shortcuts such as Alt+Left keep working.
pub(crate) fn tab_list_step(
    keystroke: &gpui::Keystroke,
    current: usize,
    count: usize,
) -> Option<usize> {
    if count == 0 || keystroke.modifiers.modified() {
        return None;
    }
    let last = count - 1;
    let next = match keystroke.key.as_str() {
        "up" | "left" => current.saturating_sub(1),
        "down" | "right" => (current + 1).min(last),
        "home" => 0,
        "end" => last,
        _ => return None,
    };
    Some(next)
}
