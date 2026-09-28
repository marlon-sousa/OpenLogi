//! The "Add device" window — drives a wireless pairing session.
//!
//! Pairing runs in the **agent** (it owns device I/O, so it opens the receiver,
//! not the GUI). This window is a thin state machine that talks to the agent
//! over IPC:
//!
//! - The buttons send [`StartPairing`] / [`PairDevice`] / [`CancelPairing`]
//!   through the agent IPC client.
//! - [`PairingUi`] — the latest session state, taken from the agent's observed
//!   state by the runtime via [`apply_state`], or a refusal the agent never
//!   turned into a session via [`apply_undeliverable`]. The view observes it
//!   and repaints on change.
//!
//! Bolt is interactive (discover → pick → enter a passkey on the device);
//! Unifying just opens a lock and waits for the next device to link, so it
//! jumps straight from *searching* to *paired*.

use gpui::{
    App, Context, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyBinding,
    ParentElement as _, Render, RenderOnce, SharedString, Size, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, actions, div, prelude::FluentBuilder as _, px, svg,
};
use gpui_base::Button as BaseButton;
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use openlogi_core::hid::{Click, PasskeyMethod, ReceiverSelector};
use openlogi_ipc::{FoundDevice, PairingFailure, PairingPhase};

use crate::app::menu::{CloseWindow, Minimize, Zoom};
use crate::services::ipc::{CancelPairing, Command, PairDevice, StartPairing};
use crate::state::AppState;
use crate::ui::focus::keep_focus_on_root;
use crate::ui::theme::{self, Palette, Typography as _};
use crate::windows::{self, AuxWindow};

/// The pairing flow as the window renders it: the agent's [`PairingPhase`]
/// plus [`Self::Idle`] for no session.
#[derive(Clone, Default, PartialEq, Eq)]
pub enum PairingUi {
    /// No session in flight (initial, or after Done / dismissing a failure).
    #[default]
    Idle,
    /// Discovery (Bolt) or the pairing lock (Unifying) is open.
    Searching,
    /// Bolt: devices discovered so far, awaiting the user's pick.
    Found(Vec<FoundDevice>),
    /// A device was picked; waiting for the receiver's next step.
    Pairing,
    /// Bolt: the device asks the user to enter a passkey.
    Passkey(PasskeyMethod),
    /// A device paired into `slot`.
    Paired { slot: u8 },
    /// The session ended without pairing.
    Failed(PairingFailure),
}

impl Global for PairingUi {}

/// Open the Add Device window, starting a fresh search unless one is already
/// in flight (re-opening just focuses the existing window).
pub fn open(cx: &mut App) {
    if !pairing_in_flight(cx) {
        start_search(cx);
    }
    windows::open_or_focus(
        |reg| &mut reg.add_device,
        window_title(),
        Size::new(px(520.), px(460.)),
        AddDeviceView::new,
        cx,
    );
}

/// Whether a pairing session is running (searching, found, pairing or
/// awaiting a passkey), as opposed to idle or finished.
fn pairing_in_flight(cx: &App) -> bool {
    matches!(
        cx.try_global::<PairingUi>(),
        Some(
            PairingUi::Searching | PairingUi::Found(_) | PairingUi::Pairing | PairingUi::Passkey(_)
        )
    )
}

/// The window's native title — one definition for open and the live-language
/// retitle ([`windows::retitle_open`]), so the two cannot drift.
pub(crate) fn window_title() -> SharedString {
    tr!("pairing.add_device")
}

/// Show the agent's pairing session. `None` is no session — including after a
/// cancel, and after an agent restart, which is why a window left mid-flow no
/// longer needs a terminal event synthesized on its behalf.
///
/// The accumulation this used to do (collecting discovered devices out of an
/// event stream) belongs to the agent, which is the side that knows what it has
/// discovered; nothing is folded here any more.
pub fn apply_state(cx: &mut App, phase: Option<PairingPhase>) {
    let next = match phase {
        None => PairingUi::Idle,
        Some(PairingPhase::Searching) => PairingUi::Searching,
        Some(PairingPhase::Found(devices)) => PairingUi::Found(devices),
        Some(PairingPhase::Pairing) => PairingUi::Pairing,
        Some(PairingPhase::Passkey(method)) => PairingUi::Passkey(method),
        Some(PairingPhase::Paired { slot }) => PairingUi::Paired { slot },
        Some(PairingPhase::Failed(failure)) => PairingUi::Failed(failure),
    };
    if cx.try_global::<PairingUi>() == Some(&next) {
        return;
    }
    cx.set_global(next);
}

/// Report a pairing command the client could not deliver. No session will ever
/// appear to explain the silence, so the window has to be told directly.
pub fn apply_undeliverable(cx: &mut App, failure: PairingFailure) {
    cx.set_global(PairingUi::Failed(failure));
}

fn pairing_failure_text(failure: &PairingFailure) -> String {
    match failure {
        PairingFailure::Hid { message } => {
            tr!("pairing.hid_transport_error", message => message.clone()).to_string()
        }
        PairingFailure::ReceiverNotFound => tr!("pairing.pairing_receiver_not_found").to_string(),
        PairingFailure::Register { message } => {
            tr!("pairing.receiver_register_error", message => message.clone()).to_string()
        }
        PairingFailure::Timeout => tr!("pairing.pairing_timed_out").to_string(),
        PairingFailure::Device { code } => tr!(
            "pairing.receiver_pairing_error",
            code => format!("0x{code:02x}"),
        )
        .to_string(),
        PairingFailure::Cancelled => tr!("pairing.pairing_was_cancelled").to_string(),
        PairingFailure::ReceiverBusy => {
            tr!("pairing.the_receiver_is_busy_try_pairing_again").to_string()
        }
        PairingFailure::WatcherUnavailable => tr!("pairing.pairing_agent_not_ready").to_string(),
        PairingFailure::AgentRestarted => tr!("agent.agent_restarted_during_pairing").to_string(),
        PairingFailure::ReceiverAccessUnavailable => {
            tr!("pairing.pairing_receiver_access_unrecorded").to_string()
        }
        PairingFailure::AlreadyActive => {
            tr!("pairing.a_pairing_session_is_already_active").to_string()
        }
        PairingFailure::UnknownDevice => {
            tr!("pairing.pairing_device_no_longer_available").to_string()
        }
        PairingFailure::NoActiveSession => tr!("pairing.no_pairing_session_is_active").to_string(),
    }
}

fn send(cx: &App, command: impl Into<Command>) {
    if let Some(state) = AppState::try_global(cx) {
        let _ = state.read(cx).ipc_sender().send(command.into());
    }
}

fn start_search(cx: &mut App) {
    send(
        cx,
        StartPairing {
            selector: ReceiverSelector::First,
        },
    );
}

/// Standalone Add Device window root view.
pub struct AddDeviceView {
    focus_handle: FocusHandle,
    /// Returns focus to the root when the focused control disappears.
    _keep_focus: Subscription,
    appearance_obs: Option<Subscription>,
    #[expect(dead_code, reason = "held to keep the PairingUi observer alive")]
    state_obs: Subscription,
}

impl AddDeviceView {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        let keep_focus = keep_focus_on_root(window, cx, focus_handle.clone());
        let mut phase = cx.try_global::<PairingUi>().map(std::mem::discriminant);
        let state_obs = cx.observe_global_in::<PairingUi>(window, move |_, window, cx| {
            cx.notify();
            // A new step replaces the controls; land on its first one. Updates
            // within a step (another device found) leave focus where it is.
            let next = cx.try_global::<PairingUi>().map(std::mem::discriminant);
            if next != phase {
                phase = next;
                land_on_first_control(window, cx);
            }
        });
        land_on_first_control(window, cx);
        Self {
            focus_handle,
            _keep_focus: keep_focus,
            appearance_obs: None,
            state_obs,
        }
    }
}

/// Move focus to the step's first control once it is drawn: the one that
/// carries the step's status as its description, so a screen reader reads
/// what is happening together with what to do about it.
fn land_on_first_control(window: &mut Window, cx: &mut Context<AddDeviceView>) {
    // From the root, so landing is idempotent: steps can follow each other
    // within one frame, queueing two landings, and a control both steps show
    // (Cancel) must not be the starting point of the second.
    // Two frames: GPUI runs next-frame work before drawing that frame, and
    // `focus_next` walks the last drawn frame's tab order, so the first frame
    // draws the new step and the second lands on it.
    cx.on_next_frame(window, |_, window, cx| {
        cx.on_next_frame(window, |view, window, cx| {
            view.focus_handle.focus(window, cx);
            window.focus_next(cx);
        });
    });
}

actions!(
    pairing,
    [
        /// Cancel any pairing in flight and close the Add Device window.
        DismissAddDevice
    ]
);

const ADD_DEVICE_CONTEXT: &str = "AddDevice";

/// The Add Device window's bindings; installed with the app's others.
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        DismissAddDevice,
        Some(ADD_DEVICE_CONTEXT),
    )]);
}

impl AuxWindow for AddDeviceView {
    fn set_appearance_obs(&mut self, sub: Subscription) {
        self.appearance_obs = Some(sub);
    }
}

impl Render for AddDeviceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        theme::apply_ui_scale(window, cx);
        let pal = theme::palette(cx);
        let state = cx.try_global::<PairingUi>().cloned().unwrap_or_default();

        v_flex()
            .size_full()
            .bg(pal.page)
            .text_color(pal.text_primary)
            .track_focus(&self.focus_handle)
            .key_context(ADD_DEVICE_CONTEXT)
            .on_action(|_: &DismissAddDevice, window, cx| {
                if pairing_in_flight(cx) {
                    send(cx, CancelPairing);
                }
                window.remove_window();
            })
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window())
            // Linux only: a client-side titlebar at the top of the window; the
            // padded content sits in the flex-column below it. macOS / Windows
            // keep their native titlebar.
            .when(cfg!(target_os = "linux"), |this| {
                this.child(windows::aux_title_bar(tr!("pairing.add_device"), cx))
            })
            .child(
                v_flex()
                    .flex_1()
                    .w_full()
                    .p_6()
                    .gap_5()
                    .child(
                        div()
                            .text_heading()
                            .child(tr!("pairing.add_device")),
                    )
                    .child(AddDeviceBody { state }),
            )
    }
}

/// The state-dependent body owns theme resolution for the complete pairing flow.
#[derive(IntoElement)]
struct AddDeviceBody {
    state: PairingUi,
}

impl RenderOnce for AddDeviceBody {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pal = theme::palette(cx);
        pairing_body(self.state, pal)
    }
}

fn pairing_body(state: PairingUi, pal: Palette) -> impl IntoElement {
    let mut col = v_flex().w_full().flex_1().gap_4();
    match state {
        PairingUi::Idle => {
            col = col
                .child(hint(tr!("pairing.pairing_mode_description"), pal))
                .child(
                    action_button("ad-search", tr!("pairing.search_for_devices"), true)
                        .accessibility_label(named_with_status(
                            tr!("pairing.search_for_devices"),
                            &tr!("pairing.pairing_mode_description"),
                        ))
                        .on_click(|_, _, cx| start_search(cx)),
                );
        }
        PairingUi::Searching => {
            col = col
                .child(status_line(tr!("pairing.searching_for_devices")))
                .child(hint(tr!("pairing.pairing_search_hint"), pal))
                .child(cancel_button(&status_text(&[
                    tr!("pairing.searching_for_devices"),
                    tr!("pairing.pairing_search_hint"),
                ])));
        }
        PairingUi::Found(devices) => col = found_step(col, &devices, pal),
        PairingUi::Pairing => {
            col = col
                .child(status_line(tr!("pairing.pairing")))
                .child(hint(
                    tr!("pairing.follow_the_instructions_on_your_device"),
                    pal,
                ))
                .child(cancel_button(&status_text(&[
                    tr!("pairing.pairing"),
                    tr!("pairing.follow_the_instructions_on_your_device"),
                ])));
        }
        PairingUi::Passkey(method) => {
            col = col
                .child(passkey_panel(&method, pal))
                .child(cancel_button(&passkey_status(&method)));
        }
        PairingUi::Paired { slot } => {
            col = col
                .child(
                    div()
                        .text_color(pal.text_primary)
                        .font_weight(FontWeight::MEDIUM)
                        .child(tr!("pairing.device_paired")),
                )
                .child(hint(
                    tr!("pairing.paired_receiver_slot", slot => slot.to_string()),
                    pal,
                ))
                .child(
                    action_button("ad-done", tr!("common.done"), false)
                        .accessibility_label(named_with_status(
                            tr!("common.done"),
                            &status_text(&[
                                tr!("pairing.device_paired"),
                                tr!("pairing.paired_receiver_slot", slot => slot.to_string()),
                            ]),
                        ))
                        .on_click(|_, _, cx| send(cx, CancelPairing)),
                );
        }
        PairingUi::Failed(failure) => {
            col = col
                .child(
                    div()
                        .text_color(pal.text_primary)
                        .font_weight(FontWeight::MEDIUM)
                        .child(tr!("pairing.pairing_failed")),
                )
                .child(hint(pairing_failure_text(&failure), pal))
                .when(
                    matches!(failure, PairingFailure::ReceiverNotFound),
                    |this| this.child(hint(tr!("device.device_connection_help"), pal)),
                )
                .child(
                    action_button("ad-retry", tr!("common.try_again"), true)
                        .accessibility_label(named_with_status(
                            tr!("common.try_again"),
                            &status_text(&[
                                tr!("pairing.pairing_failed"),
                                pairing_failure_text(&failure).into(),
                            ]),
                        ))
                        .on_click(|_, _, cx| start_search(cx)),
                );
        }
    }
    col
}

/// Bolt discovery with results so far: the devices to pick from, or a note
/// that none has turned up yet.
fn found_step(col: gpui::Div, devices: &[FoundDevice], pal: Palette) -> gpui::Div {
    let col = col.child(status_line(tr!("pairing.searching_for_devices")));
    if devices.is_empty() {
        return col
            .child(hint(tr!("pairing.no_devices_found_yet"), pal))
            .child(cancel_button(&status_text(&[
                tr!("pairing.searching_for_devices"),
                tr!("pairing.no_devices_found_yet"),
            ])));
    }
    let status = status_text(&[
        tr!("pairing.searching_for_devices"),
        tr!("pairing.select_a_device_to_pair"),
    ]);
    col.child(hint(tr!("pairing.select_a_device_to_pair"), pal))
        .children(
            devices.iter().enumerate().map(|(index, device)| {
                device_row(device, (index == 0).then(|| status.clone()), pal)
            }),
        )
        .child(cancel_button(&SharedString::default()))
}

/// A discovered-device row; clicking it pairs with that device.
fn device_row(
    device: &FoundDevice,
    status: Option<SharedString>,
    pal: Palette,
) -> impl IntoElement {
    let address = device.address;
    let address_id = u64::from_be_bytes([
        0, 0, address[0], address[1], address[2], address[3], address[4], address[5],
    ]);
    let name = SharedString::from(device.name.clone());
    BaseButton::new(("found-device", address_id))
        .accessibility_label(name.clone())
        .when_some(status, gpui::StatefulInteractiveElement::aria_description)
        .w_full()
        .flex()
        .items_center()
        .justify_start()
        .px_4()
        .py_3()
        .rounded(pal.control_radius)
        .border_1()
        .border_color(pal.border)
        .cursor_pointer()
        .bg(pal.control)
        .hover(|s| s.bg(pal.control_hover))
        .focus_visible(|s| s.bg(pal.control_hover))
        .child(div().text_body().child(name))
        .on_click(move |_, _, cx| send(cx, PairDevice { address }))
}

/// The passkey-entry instructions panel.
fn passkey_panel(method: &PasskeyMethod, pal: Palette) -> impl IntoElement {
    let mut col = v_flex().w_full().gap_3();
    match method {
        PasskeyMethod::Keyboard(digits) => {
            col = col
                .child(status_line(tr!(
                    "pairing.keyboard_pairing_passkey_instructions"
                )))
                .child(div().text_title().child(SharedString::from(digits.clone())));
        }
        PasskeyMethod::Pointer { clicks, .. } => {
            col = col
                .child(status_line(tr!(
                    "pairing.mouse_pairing_passkey_instructions"
                )))
                .child(
                    h_flex()
                        .id("passkey-sequence")
                        // The icons carry no text of their own, so the order is
                        // spelled out once here rather than left to assistive
                        // tech as a row of unlabelled images.
                        .aria_label(spoken_click_sequence(clicks))
                        .gap_2()
                        .children(clicks.iter().enumerate().map(|(step, click)| {
                            v_flex()
                                .items_center()
                                .gap_0p5()
                                .child(svg().path(click_icon(*click)).size_6().flex_none())
                                .child(
                                    div()
                                        .text_caption()
                                        .text_color(pal.text_muted)
                                        .child((step + 1).to_string()),
                                )
                        })),
                );
        }
    }
    col
}

/// The mouse body with the button this step wants filled in.
fn click_icon(click: Click) -> &'static str {
    match click {
        Click::Left => "action-icons/mouse-left.svg",
        Click::Right => "action-icons/mouse-right.svg",
    }
}

/// The click sequence as an ordered sentence, for the accessibility tree.
fn spoken_click_sequence(clicks: &[Click]) -> String {
    clicks
        .iter()
        .enumerate()
        .map(|(step, click)| {
            let label = match click {
                Click::Left => tr!("actions.left_click"),
                Click::Right => tr!("actions.right_click"),
            };
            format!("{}. {label}", step + 1)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn status_line(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .text_body()
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

fn hint(text: impl Into<SharedString>, pal: Palette) -> impl IntoElement {
    div()
        .text_caption()
        .text_color(pal.text_muted)
        .child(text.into())
}

/// A styled button. `primary` paints it accent-filled; otherwise it's the
/// neutral default. The caller attaches `.on_click`.
fn action_button(id: &'static str, label: impl Into<SharedString>, primary: bool) -> Button {
    let button = Button::new(id).label(label);
    if primary { button.primary() } else { button }
}

/// Cancel, describing the step it cancels when it is the step's first
/// control (see [`land_on_first_control`]); an empty `status` adds nothing.
/// A gpui-kit button's accessible name with the step's status after it: that
/// button exposes no accessible description, so the status rides in the name
/// (an empty `status` leaves the plain name).
fn named_with_status(name: SharedString, status: &SharedString) -> SharedString {
    if status.is_empty() {
        name
    } else {
        format!("{name}. {status}").into()
    }
}

fn cancel_button(status: &SharedString) -> impl IntoElement {
    action_button("ad-cancel", tr!("common.cancel"), false)
        .accessibility_label(named_with_status(tr!("common.cancel"), status))
        .on_click(|_, _, cx| send(cx, CancelPairing))
}

/// A step's status lines as one sentence for a control's description.
fn status_text(lines: &[SharedString]) -> SharedString {
    lines
        .iter()
        .map(SharedString::as_ref)
        .collect::<Vec<_>>()
        .join(" ")
        .into()
}

/// The passkey step's instructions and the code itself, spoken.
fn passkey_status(method: &PasskeyMethod) -> SharedString {
    match method {
        PasskeyMethod::Keyboard(digits) => status_text(&[
            tr!("pairing.keyboard_pairing_passkey_instructions"),
            digits.clone().into(),
        ]),
        PasskeyMethod::Pointer { clicks, .. } => status_text(&[
            tr!("pairing.mouse_pairing_passkey_instructions"),
            spoken_click_sequence(clicks).into(),
        ]),
    }
}
