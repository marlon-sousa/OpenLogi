//! The primitives as a screen reader reads them, on the accessibility-enabled
//! headless platform.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Role, Styled as _,
    Window, accesskit, div, px,
};
use gpui_component::input::InputState;

use super::super::{ControlCallout, ControlTarget, control_input, control_switch, icon_button};
use crate::ui::a11y_test::{A11yNode, A11yTestApp};
use crate::ui::commit_slider::{CommitSlider, SliderRange};

fn open<V: Render + 'static>(
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
) -> A11yTestApp {
    let app = A11yTestApp::new();
    app.update(gpui_component::init);
    // Under the component Root, as every production window is: it owns Tab.
    app.open_window(|window, cx| {
        let view = cx.new(|cx| build(window, cx));
        cx.new(|cx| gpui_component::Root::new(view, window, cx))
    });
    app
}

fn focused(app: &A11yTestApp) -> A11yNode {
    app.snapshot()
        .focused()
        .cloned()
        .expect("focus is somewhere")
}

struct ControlPair {
    clicks: Rc<Cell<usize>>,
}

impl Render for ControlPair {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        let target_clicks = self.clicks.clone();
        div()
            .size_full()
            .child(
                ControlCallout::new("callout-f1", "F1")
                    .binding("Off")
                    .expanded(false)
                    .w(px(60.))
                    .h(px(30.))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            )
            .child(
                ControlTarget::new("target-f1", "F1")
                    .pointer_only(true)
                    .size(px(20.))
                    .on_click(move |_, _, _| target_clicks.set(target_clicks.get() + 1)),
            )
            .child(ControlTarget::new("target-middle", "Middle button").size(px(20.)))
    }
}

#[test]
fn a_control_is_one_named_tab_stop_whether_callout_or_target() {
    let clicks = Rc::new(Cell::new(0));
    let app = open({
        let clicks = clicks.clone();
        move |_, _| ControlPair { clicks }
    });

    let tree = app.snapshot();
    let f1: Vec<_> = tree
        .nodes()
        .iter()
        .filter(|node| node.name.as_deref() == Some("Bind F1"))
        .collect();
    assert_eq!(
        f1.len(),
        1,
        "the pointer-only target is not a second F1:\n{tree:?}"
    );
    assert_eq!(f1[0].role, Role::Button);
    assert_eq!(f1[0].description.as_deref(), Some("Off"));
    assert_eq!(f1[0].expanded, Some(false));
    tree.expect(Role::Button, "Bind Middle button");

    app.action(f1[0], accesskit::Action::Focus);
    assert_eq!(focused(&app).name.as_deref(), Some("Bind F1"));
    app.press("enter");
    app.press("space");
    assert_eq!(clicks.get(), 2, "Enter and Space activate the callout");
    app.press("tab");
    assert_eq!(focused(&app).name.as_deref(), Some("Bind Middle button"));
}

struct Brightness {
    slider: CommitSlider<u8>,
}

impl Render for Brightness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let value = self.slider.value(cx);
        div().w(px(200.)).child(
            self.slider
                .control("Brightness")
                .value_text(format!("{value}%")),
        )
    }
}

fn brightness(app: &A11yTestApp) -> A11yNode {
    app.snapshot()
        .nodes()
        .iter()
        .find(|node| node.role == Role::Slider && node.name.as_deref() == Some("Brightness"))
        .cloned()
        .expect("the named slider")
}

#[test]
fn a_slider_is_named_focusable_and_commits_each_keyboard_step() {
    let commits = Rc::new(Cell::new(0));
    let app = open({
        let commits = commits.clone();
        move |_, cx| Brightness {
            slider: CommitSlider::new(SliderRange::new(0, 100).step(5.), 50, cx, {
                let commits = commits.clone();
                move |_, _, _| commits.set(commits.get() + 1)
            }),
        }
    });

    let slider = brightness(&app);
    assert_eq!(slider.value.as_deref(), Some("50%"));
    assert!(slider.focusable, "Tab can reach the slider");

    app.action(&slider, accesskit::Action::Focus);
    assert_eq!(focused(&app).id, slider.id);
    app.press("right");
    assert_eq!(brightness(&app).value.as_deref(), Some("55%"));
    assert_eq!(commits.get(), 1, "a keyboard step commits once");
    app.press("home");
    assert_eq!(brightness(&app).value.as_deref(), Some("0%"));
    app.press("left");
    assert_eq!(commits.get(), 2, "a step past the end commits nothing");

    app.action(&brightness(&app), accesskit::Action::Increment);
    assert_eq!(brightness(&app).value.as_deref(), Some("5%"));
    assert_eq!(commits.get(), 3, "the adapter's Increment commits too");
}

struct FormControls {
    input: Entity<InputState>,
}

impl Render for FormControls {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(icon_button(
                "remove",
                gpui_component::IconName::Close,
                "Remove preset",
            ))
            .child(control_switch("lights", "Lighting"))
            .child(control_input(&self.input, "Device name"))
    }
}

#[test]
fn form_controls_are_named_by_their_label_not_their_placeholder() {
    let app = open(|window, cx| FormControls {
        input: cx.new(|cx| InputState::new(window, cx).placeholder("MX Keys")),
    });
    let tree = app.snapshot();
    tree.expect(Role::Button, "Remove preset");
    tree.expect(Role::Switch, "Lighting");
    let input = tree
        .nodes()
        .iter()
        .find(|node| node.role == Role::TextInput)
        .unwrap_or_else(|| panic!("no text input:\n{tree:?}"));
    assert_eq!(input.name.as_deref(), Some("Device name"), "{tree:?}");
    assert!(tree.unnamed_controls().is_empty(), "{tree:?}");
}
