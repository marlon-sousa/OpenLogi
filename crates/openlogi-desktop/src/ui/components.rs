//! Reusable, theme-aware desktop components.
//!
//! These components own their semantic state and resolve the active palette at
//! render time, so callers do not have to thread colours through free-function
//! builders.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, Entity, InteractiveElement, Interactivity, IntoElement,
    ParentElement, Pixels, RenderOnce, Role, SharedString, Stateful, StatefulInteractiveElement,
    Styled, Window, div, prelude::FluentBuilder as _, px, rgb,
};
use gpui_base::Button as BaseButton;
use gpui_component::{
    Disableable, Icon, IconName, Selectable, Sizable, Size, h_flex,
    input::{Input, InputState},
    searchable_list::SearchableListDelegate,
    select::{Select, SelectState},
    switch::Switch,
    v_flex,
};

use super::theme::{self, ACCENT_BLUE, SelectableStyle as _, Typography as _};

/// A gpui-component button at the house control height.
///
/// gpui-component's size ladder (20/24/32/44 px) has no step at the app's
/// 30 px control rhythm, and this rev's custom `Size::Size` is broken for
/// heights (`input_h` falls through to 24 px, and the text scales with the
/// size). Standalone form controls therefore take `.small()` typography and
/// pin [`theme::CONTROL_H`] explicitly — construct them through these
/// helpers; a bare `.small()` on one is a bug. Ghost and link affordances,
/// pills, and icon-only toggles stay on the stock ladder on purpose.
pub(crate) fn control_button(id: impl Into<ElementId>) -> gpui_component::button::Button {
    gpui_component::button::Button::new(id)
        .small()
        .h(px(theme::CONTROL_H))
}

/// A single-line input at the house control height; see [`control_button`].
/// Pins via `min_h` because the inherent `Input::h` is multi-line-only.
///
/// `label` is the accessible name ("Device name"). The placeholder stays a
/// hint: it is only read while the field is empty, and a screen reader user
/// who returns to a filled field would otherwise hear no name at all.
pub(crate) fn control_input(state: &Entity<InputState>, label: impl Into<SharedString>) -> Input {
    Input::new(state)
        .aria_label(label)
        .small()
        .min_h(px(theme::CONTROL_H))
}

/// A status readout a screen reader can reach ("Connected", "Battery 90 %").
///
/// GPUI puts plain text in the accessibility tree only when it has an id, so
/// status that is not on a control is invisible to assistive technology until
/// it goes through this. `text` is what is read; the caller adds the visuals.
pub(crate) fn readout(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
) -> Stateful<gpui::Div> {
    div().id(id).role(Role::Label).aria_value(text)
}

/// A named container for a set of choices ("Wheel resolution"). gpui-kit's
/// `ButtonGroup` exposes no group of its own, so its buttons ("Standard",
/// "High resolution") would otherwise be read without the question they answer.
pub(crate) fn control_group(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<gpui::Div> {
    div().id(id).role(Role::Group).aria_label(label)
}

/// One choice among several ("Free spin" / "Ratchet"): a radio button that
/// reads "checked" when it is the current one. Windows reports a button's
/// `selected` to no one, so a choice has to carry its state as checked. Put
/// the choices in a [`control_group`] named for the question they answer.
pub(crate) fn choice_button(
    button: gpui_component::button::Button,
    selected: bool,
) -> gpui_component::button::Button {
    button
        .role(Role::RadioButton)
        .toggled(selected)
        .selected(selected)
}

/// A screen title or section title, listed among a screen reader's headings.
/// `level` 1 is the screen's own title; 2 a section of it.
pub(crate) fn heading(
    id: impl Into<ElementId>,
    level: usize,
    text: impl Into<SharedString>,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .role(Role::Heading)
        .aria_level(level)
        .aria_label(text)
}

/// An icon-only button. `label` is required: it is the accessible name and the
/// hover tooltip, so the name a screen reader reads is the one a pointer user
/// sees. Callers pick the variant and size.
pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    icon: impl Into<Icon>,
    label: impl Into<SharedString>,
) -> gpui_component::button::Button {
    let label = label.into();
    gpui_component::button::Button::new(id)
        .icon(icon)
        .accessibility_label(label.clone())
        .tooltip(label)
}

/// A bare on/off switch named for the setting it controls; its caption is
/// rendered by the caller beside it.
pub(crate) fn control_switch(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Switch {
    Switch::new(id).accessibility_label(label)
}

/// Re-derive an input's placeholder from the current locale.
///
/// Placeholders are stored inside [`InputState`] at construction, so a live
/// language switch leaves them stale — the owning view calls this from render
/// with a fresh `tr!` string to keep them current. Guarded, because
/// `set_placeholder` notifies unconditionally and a bare per-render call would
/// re-render forever.
pub(crate) fn localize_placeholder(
    state: &Entity<InputState>,
    text: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    if *state.read(cx).presentation().placeholder() != text {
        state.update(cx, |state, cx| state.set_placeholder(text, window, cx));
    }
}

/// A select trigger at the house control height; see [`control_button`].
pub(crate) fn control_select<D: SearchableListDelegate + 'static>(
    state: &Entity<SelectState<D>>,
) -> Select<D> {
    Select::new(state).small().min_h(px(theme::CONTROL_H))
}

/// A controlled on/off switch with a compact state label.
#[derive(IntoElement)]
pub(crate) struct Toggle {
    id: ElementId,
    name: SharedString,
    selected: bool,
    disabled: bool,
    size: Size,
    label: Option<SharedString>,
    icon: Option<IconName>,
    min_width: Option<Pixels>,
    on_change: Option<SwitchHandler>,
}

type SwitchHandler = std::rc::Rc<dyn Fn(&bool, &mut Window, &mut App)>;

impl Toggle {
    /// `name` is the setting the switch controls ("Lighting"); it is the
    /// accessible name, while the visible caption shows the state.
    pub(crate) fn new(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            selected: false,
            disabled: false,
            size: Size::Small,
            label: None,
            icon: None,
            min_width: None,
            on_change: None,
        }
    }

    #[must_use]
    pub(crate) fn label(mut self, label: impl Into<Option<SharedString>>) -> Self {
        self.label = label.into();
        self
    }

    #[must_use]
    pub(crate) fn icon(mut self, icon: impl Into<Option<IconName>>) -> Self {
        self.icon = icon.into();
        self
    }

    #[must_use]
    pub(crate) fn min_width(mut self, width: impl Into<Option<Pixels>>) -> Self {
        self.min_width = width.into();
        self
    }

    #[must_use]
    pub(crate) fn on_change(
        mut self,
        handler: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(std::rc::Rc::new(handler));
        self
    }
}

impl Selectable for Toggle {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl Disableable for Toggle {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl Sizable for Toggle {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let label = self.label.unwrap_or_else(|| {
            if self.selected {
                tr!("common.on")
            } else {
                tr!("common.off")
            }
        });
        let mut toggle = Switch::new(self.id)
            .accessibility_label(self.name)
            .checked(self.selected)
            .disabled(self.disabled)
            .with_size(self.size)
            .label(label)
            .color(rgb(ACCENT_BLUE));
        if let Some(handler) = self.on_change {
            toggle = toggle.on_click(move |selected, window, cx| handler(selected, window, cx));
        }
        h_flex()
            .items_center()
            .gap_2()
            .when_some(self.min_width, |row, width| {
                row.min_w(width).justify_center()
            })
            .when_some(self.icon, |row, icon| row.child(Icon::new(icon).size_3()))
            .child(toggle)
    }
}

/// A titled card used by device-detail panels.
#[derive(IntoElement)]
pub(crate) struct PanelCard {
    title: SharedString,
    icon: Icon,
    content: AnyElement,
    fill: bool,
}

impl PanelCard {
    pub(crate) fn new(
        title: impl Into<SharedString>,
        icon: Icon,
        content: impl IntoElement,
    ) -> Self {
        Self {
            title: title.into(),
            icon,
            content: content.into_any_element(),
            fill: false,
        }
    }

    #[must_use]
    pub(crate) fn fill(mut self) -> Self {
        self.fill = true;
        self
    }
}

impl RenderOnce for PanelCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pal = theme::palette(cx);
        div()
            .w_full()
            .when(self.fill, gpui::Styled::h_full)
            .max_w_full()
            .min_w_0()
            .rounded(pal.card_radius)
            .border_1()
            .border_color(pal.border)
            .bg(pal.panel)
            .p(theme::CARD_PAD.rems())
            .child(
                v_flex()
                    .gap(theme::CARD_GAP.rems())
                    .when(!self.title.is_empty(), |this| {
                        this.child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .text_color(pal.text_primary)
                                .child(self.icon.size_4().text_color(pal.text_muted))
                                .child(div().text_subheading().child(self.title)),
                        )
                    })
                    .child(self.content),
            )
    }
}

type ClickHandler = std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A full-width row in an action menu.
#[derive(IntoElement)]
pub(crate) struct MenuRow {
    base: BaseButton,
    role: Role,
    selected: bool,
    children: Vec<AnyElement>,
    on_click: Option<ClickHandler>,
}

impl MenuRow {
    /// `label` is the row's accessible name. Children are visual only, so a
    /// row whose content is an icon, a value or a step summary still says
    /// what choosing it does.
    pub(crate) fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            base: BaseButton::new(id).accessibility_label(label),
            role: Role::Button,
            selected: false,
            children: Vec::new(),
            on_click: None,
        }
    }

    /// Set the accessibility role for this row's semantic button.
    #[must_use]
    pub(crate) fn role(mut self, role: Role) -> Self {
        self.role = role;
        self.base = self.base.role(role);
        self
    }

    /// Track `handle` instead of the row's own keyed focus, so its list can
    /// move focus onto this row (the current choice, when a panel opens).
    #[must_use]
    pub(crate) fn track_focus(mut self, handle: &gpui::FocusHandle) -> Self {
        self.base = self.base.track_focus(handle);
        self
    }

    #[must_use]
    pub(crate) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(std::rc::Rc::new(handler));
        self
    }
}

impl Selectable for MenuRow {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl ParentElement for MenuRow {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl InteractiveElement for MenuRow {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for MenuRow {}

impl RenderOnce for MenuRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pal = theme::palette(cx);
        let selected = self.selected;
        // A choice among rows says whether it is the current one. Windows
        // reports that as "checked" for radio roles and ignores "selected"
        // on menu items and buttons, so a choice row carries it as toggled.
        let base = if matches!(self.role, Role::MenuItemRadio | Role::RadioButton) {
            self.base.aria_toggled(if selected {
                gpui::accesskit::Toggled::True
            } else {
                gpui::accesskit::Toggled::False
            })
        } else {
            self.base.aria_selected(selected)
        };
        base.selected(selected)
            // Rows form a list: Up and Down walk it as Shift+Tab and Tab do.
            .on_key_down(|event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.modified() {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "down" => window.focus_next(cx),
                    "up" => window.focus_prev(cx),
                    _ => return,
                }
                cx.stop_propagation();
            })
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded(pal.control_radius)
            .text_body()
            .text_color(pal.text_primary)
            .selected_fill(selected)
            .hover(move |style| {
                style.bg(if selected {
                    theme::accent_tint_hover()
                } else {
                    pal.control_hover
                })
            })
            .focus_visible(move |style| {
                style.bg(if selected {
                    theme::accent_tint_hover()
                } else {
                    pal.control_hover
                })
            })
            .children(self.children)
            .when_some(self.on_click, |row, handler| {
                row.on_click(move |event, window, cx| handler(event, window, cx))
            })
    }
}

/// A selectable camera-profile chip.
#[derive(IntoElement)]
pub(crate) struct ProfileTab {
    base: BaseButton,
    label: SharedString,
    icon: Option<Icon>,
    selected: bool,
    children: Vec<AnyElement>,
    on_click: Option<ClickHandler>,
    delete: Option<(ElementId, ClickHandler)>,
}

impl ProfileTab {
    pub(crate) fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            base: BaseButton::new(id).role(Role::Tab),
            label: label.into(),
            icon: None,
            selected: false,
            children: Vec::new(),
            on_click: None,
            delete: None,
        }
    }

    /// A mark before the label, for a tab that is an action rather than a
    /// profile (the one that creates a new profile).
    #[must_use]
    pub(crate) fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    #[must_use]
    pub(crate) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(std::rc::Rc::new(handler));
        self
    }

    #[must_use]
    pub(crate) fn on_delete(
        mut self,
        id: impl Into<ElementId>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.delete = Some((id.into(), std::rc::Rc::new(handler)));
        self
    }
}

impl Selectable for ProfileTab {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl ParentElement for ProfileTab {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl InteractiveElement for ProfileTab {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for ProfileTab {}

impl RenderOnce for ProfileTab {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pal = theme::palette(cx);
        let accent = rgb(ACCENT_BLUE);
        let has_delete = self.delete.is_some();
        let label = self.label;
        self.base
            .selected(self.selected)
            .aria_selected(self.selected)
            .accessibility_label(label.clone())
            .when(!has_delete, gpui::Styled::px_2)
            .when(has_delete, |tab| tab.pl_2().pr_1())
            .flex()
            .py_0p5()
            .gap_1()
            .items_center()
            .rounded_full()
            .border_1()
            .border_color(if self.selected {
                accent.into()
            } else {
                pal.border
            })
            .text_caption()
            .text_color(if self.selected {
                pal.text_primary
            } else {
                pal.text_muted
            })
            .bg(if self.selected {
                theme::accent_tint()
            } else {
                pal.control
            })
            .hover(move |style| {
                style.bg(if self.selected {
                    theme::accent_tint_hover()
                } else {
                    pal.control_hover
                })
            })
            .focus_visible(move |style| {
                style
                    .bg(if self.selected {
                        theme::accent_tint_hover()
                    } else {
                        pal.control_hover
                    })
                    .border_color(accent)
            })
            .when_some(self.icon, |tab, icon| tab.child(icon.size_3()))
            .child(label.clone())
            .children(self.children)
            .when_some(self.on_click, |tab, handler| {
                tab.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .when_some(self.delete, |tab, (id, handler)| {
                tab.child(
                    BaseButton::new(id)
                        .accessibility_label(
                            tr!("common.remove_named_item", name => label.to_string()),
                        )
                        .px_0p5()
                        .rounded_full()
                        .text_color(pal.text_muted)
                        .hover(|style| style.text_color(pal.text_primary))
                        .focus_visible(|style| style.text_color(pal.text_primary))
                        .child(Icon::new(IconName::Close).size_3())
                        .on_click(move |event, window, cx| {
                            cx.stop_propagation();
                            handler(event, window, cx);
                        }),
                )
            })
    }
}

/// A selected-state container for one DPI preset and its actions.
#[derive(IntoElement)]
pub(crate) struct PresetChip {
    base: Stateful<gpui::Div>,
    selected: bool,
    children: Vec<AnyElement>,
}

impl PresetChip {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        Self {
            base: h_flex().id(id),
            selected: false,
            children: Vec::new(),
        }
    }
}

impl Selectable for PresetChip {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl ParentElement for PresetChip {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for PresetChip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pal = theme::palette(cx);
        let selected = self.selected;
        self.base
            .h(px(28.))
            .px_2()
            .gap_2()
            .items_center()
            .rounded(pal.control_radius)
            .selected_border(selected, pal)
            .bg(pal.control)
            .selected_fill(selected)
            .hover(move |style| {
                style.bg(if selected {
                    theme::accent_tint_hover()
                } else {
                    pal.control_hover
                })
            })
            .children(self.children)
    }
}

mod control;

pub(crate) use control::{ControlCallout, ControlTarget};

#[cfg(test)]
mod tests;
