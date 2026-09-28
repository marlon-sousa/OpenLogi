//! Device-photo controls: a device's physical control (a mouse button, a
//! keyboard key) is bound through a labelled callout beside the photo and a
//! hotspot over the control itself.
//!
//! The pair is one control to a screen reader. [`ControlCallout`] carries the
//! name ("Bind F1"), the binding as its description, and the tab stop;
//! [`ControlTarget`] is the pointer's shortcut to the same selection and is
//! [`ControlTarget::pointer_only`] whenever a callout is on screen, so a
//! control is never announced — or tabbed through — twice.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FocusHandle, InteractiveElement, Interactivity,
    IntoElement, ParentElement, RenderOnce, SharedString, StatefulInteractiveElement,
    StyleRefinement, Styled, Window, prelude::FluentBuilder as _, rgb,
};
use gpui_base::Button as BaseButton;
use gpui_component::Selectable;

use crate::ui::theme::{self, ACCENT_BLUE};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type HoverHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

/// The accessible name every binding control uses: "Bind {control}".
fn bind_name(control_name: &SharedString) -> SharedString {
    tr!("actions.bind_control", name => control_name.to_string())
}

/// The labelled bubble for one device control. Keyboard- and screen-reader
/// reachable: named "Bind {control}", its current binding as the description.
///
/// The caller supplies size, position and content; the callout owns the
/// semantics and the interaction states (highlight, hover, focus ring).
#[derive(IntoElement)]
pub(crate) struct ControlCallout {
    base: BaseButton,
    control_name: SharedString,
    binding: Option<SharedString>,
    selected: bool,
    expanded: Option<bool>,
    highlighted: bool,
    children: Vec<AnyElement>,
    on_click: Option<ClickHandler>,
    on_hover: Option<HoverHandler>,
}

impl ControlCallout {
    /// `control_name` is the control's own name ("F1", "Middle button");
    /// the callout is announced as "Bind {control_name}".
    pub(crate) fn new(id: impl Into<ElementId>, control_name: impl Into<SharedString>) -> Self {
        Self {
            base: BaseButton::new(id),
            control_name: control_name.into(),
            binding: None,
            selected: false,
            expanded: None,
            highlighted: false,
            children: Vec::new(),
            on_click: None,
            on_hover: None,
        }
    }

    /// What the control does now ("Brightness down", "Off"), announced after
    /// the name.
    #[must_use]
    pub(crate) fn binding(mut self, binding: impl Into<SharedString>) -> Self {
        self.binding = Some(binding.into());
        self
    }

    /// For a callout that opens an editor panel: whether that panel is open.
    #[must_use]
    pub(crate) fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    /// Paint the accent without selecting, e.g. while its peer is hovered.
    #[must_use]
    pub(crate) fn highlighted(mut self, highlighted: bool) -> Self {
        self.highlighted = highlighted;
        self
    }

    /// Supply the focus handle, so a closing panel can return focus here.
    #[must_use]
    pub(crate) fn track_focus(mut self, handle: &FocusHandle) -> Self {
        self.base = self.base.track_focus(handle);
        self
    }

    #[must_use]
    pub(crate) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    #[must_use]
    pub(crate) fn on_hover(
        mut self,
        handler: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_hover = Some(Rc::new(handler));
        self
    }
}

impl Selectable for ControlCallout {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl ParentElement for ControlCallout {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for ControlCallout {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl RenderOnce for ControlCallout {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pal = theme::palette(cx);
        let selected = self.selected;
        let lit = self.highlighted || selected;
        self.base
            .selected(selected)
            .accessibility_label(bind_name(&self.control_name))
            .aria_selected(selected)
            .when_some(self.binding, StatefulInteractiveElement::aria_description)
            .when_some(self.expanded, StatefulInteractiveElement::aria_expanded)
            .rounded(pal.control_radius)
            .border_1()
            .border_color(if lit {
                rgb(ACCENT_BLUE).into()
            } else {
                pal.border
            })
            .bg(if lit {
                theme::accent_tint()
            } else {
                pal.control
            })
            .cursor_pointer()
            .hover(move |style| {
                style.bg(if lit {
                    theme::accent_tint_hover()
                } else {
                    pal.control_hover
                })
            })
            .focus_visible(move |style| {
                style
                    .bg(if lit {
                        theme::accent_tint_hover()
                    } else {
                        pal.control_hover
                    })
                    .border_color(rgb(ACCENT_BLUE))
            })
            .children(self.children)
            .when_some(self.on_click, |this, handler| {
                this.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .when_some(self.on_hover, |this, handler| {
                this.on_hover(move |hovered, window, cx| handler(hovered, window, cx))
            })
    }
}

/// The hotspot over a control on the device photo.
///
/// Accessible on its own — named "Bind {control}" — for layouts that show no
/// callouts. Where a [`ControlCallout`] names the same control, make it
/// [`Self::pointer_only`].
#[derive(IntoElement)]
pub(crate) struct ControlTarget {
    base: BaseButton,
    control_name: SharedString,
    selected: bool,
    pointer_only: bool,
    children: Vec<AnyElement>,
    on_click: Option<ClickHandler>,
    on_hover: Option<HoverHandler>,
}

impl ControlTarget {
    pub(crate) fn new(id: impl Into<ElementId>, control_name: impl Into<SharedString>) -> Self {
        Self {
            base: BaseButton::new(id),
            control_name: control_name.into(),
            selected: false,
            pointer_only: false,
            children: Vec::new(),
            on_click: None,
            on_hover: None,
        }
    }

    /// Leave the accessibility tree and the Tab order to the callout that
    /// names this control; the hotspot stays clickable and hoverable.
    #[must_use]
    pub(crate) fn pointer_only(mut self, pointer_only: bool) -> Self {
        self.pointer_only = pointer_only;
        self
    }

    #[must_use]
    pub(crate) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    #[must_use]
    pub(crate) fn on_hover(
        mut self,
        handler: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_hover = Some(Rc::new(handler));
        self
    }
}

impl Selectable for ControlTarget {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl ParentElement for ControlTarget {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for ControlTarget {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for ControlTarget {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for ControlTarget {}

impl RenderOnce for ControlTarget {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let selected = self.selected;
        let base = if self.pointer_only {
            self.base.role(None::<gpui::Role>).focusable(false)
        } else {
            self.base
                .accessibility_label(bind_name(&self.control_name))
                .aria_selected(selected)
        };
        base.selected(selected)
            .cursor_pointer()
            .focus_visible(|style| {
                style
                    .rounded_full()
                    .border_2()
                    .border_color(rgb(ACCENT_BLUE))
            })
            .children(self.children)
            .when_some(self.on_click, |this, handler| {
                this.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .when_some(self.on_hover, |this, handler| {
                this.on_hover(move |hovered, window, cx| handler(hovered, window, cx))
            })
    }
}
