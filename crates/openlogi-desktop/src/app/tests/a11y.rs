//! The main window as a screen reader reads it: roles, names and focus on the
//! accessibility-enabled headless platform ([`A11yTestApp`]).

use gpui::{AppContext as _, Role};
use openlogi_core::config::Config;
use openlogi_core::device::{
    BatteryInfo, BatteryLevel, BatteryStatus, Capabilities, DeviceInventory, DeviceKind,
    PairedDevice, ReceiverInfo, StandaloneDevice,
};
use openlogi_ipc::{AgentStatus, InventoryHealth, PROTOCOL_VERSION};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::services::assets::AssetResolver;
use crate::services::ipc::Command;
use crate::state::{AgentLink, AppState, Sources};
use crate::ui::a11y_test::{A11yNode, A11yTestApp};

/// The production main window over a fixed inventory and a ready agent.
pub(crate) struct MainWindow {
    pub app: A11yTestApp,
    /// Held so writes the window sends to the "agent" have somewhere to go.
    _commands: UnboundedReceiver<Command>,
}

impl MainWindow {
    pub fn open(inventories: &[DeviceInventory]) -> Self {
        Self::open_with(inventories, &[])
    }

    pub fn open_with(inventories: &[DeviceInventory], standalone: &[StandaloneDevice]) -> Self {
        Self::open_root(
            inventories,
            standalone,
            crate::windows::main_window::build_root,
        )
    }

    /// Any production window over the same state: `build` is the window's
    /// real root builder.
    pub fn open_root<V: gpui::Render + 'static>(
        inventories: &[DeviceInventory],
        standalone: &[StandaloneDevice],
        build: impl FnOnce(&mut gpui::Window, &mut gpui::App) -> gpui::Entity<V>,
    ) -> Self {
        let app = A11yTestApp::new();
        let (sender, commands) = tokio::sync::mpsc::unbounded_channel();
        app.update(|cx| {
            gpui_component::init(cx);
            crate::ui::theme::register_builtin_themes(cx);
            crate::app::menu::install(cx);
            crate::platform::installation::install(cx);
            let resolver = AssetResolver::new();
            let mut state = AppState::new(Sources {
                inventories,
                standalone,
                ..Sources::in_memory(Config::ephemeral(), &resolver, sender)
            });
            let _ = state.set_agent_link(AgentLink::Ready(ready_status()));
            let state = cx.new(|_| state);
            AppState::set_global(state, cx);
        });
        app.open_window(build);
        Self {
            app,
            _commands: commands,
        }
    }

    /// Open the named device from its Home card with the keyboard, failing
    /// unless its page actually opened.
    #[track_caller]
    pub fn open_device(&self, name: &str) {
        self.focus(Role::Button, name);
        self.app.press("enter");
        let tree = self.app.snapshot();
        assert!(
            tree.nodes().iter().any(|node| node.role == Role::Tab),
            "Enter on the {name} card did not open its page:\n{tree:?}"
        );
    }

    /// Move focus to the control named `name`, as a screen reader's focus
    /// command does, and return it.
    #[track_caller]
    pub fn focus(&self, role: Role, name: &str) -> A11yNode {
        let node = self.app.snapshot().expect(role, name).clone();
        self.app.action(&node, gpui::accesskit::Action::Focus);
        node
    }

    #[track_caller]
    pub fn focused(&self) -> A11yNode {
        let tree = self.app.snapshot();
        tree.focused()
            .filter(|node| node.role != Role::Window)
            .cloned()
            .unwrap_or_else(|| panic!("focus is on no control:\n{tree:?}"))
    }
}

fn ready_status() -> AgentStatus {
    AgentStatus {
        accessibility_granted: true,
        hook_installed: true,
        launch_at_login: false,
        inventory: InventoryHealth::Ready,
        protocol_version: PROTOCOL_VERSION,
        agent_version: "a11y-test".to_owned(),
        input_monitoring_granted: true,
        hid_open_failures: false,
    }
}

/// A K780 on a Unifying receiver: the keyboard from the live NVDA session.
pub(crate) fn k780() -> DeviceInventory {
    DeviceInventory {
        receiver: ReceiverInfo {
            name: "Unifying Receiver".to_owned(),
            vendor_id: 0x046d,
            product_id: 0xc52b,
            unique_id: Some("a11y-unifying".to_owned()),
        },
        paired: vec![PairedDevice {
            slot: 1,
            codename: Some("K780".to_owned()),
            wpid: Some(0x405b),
            kind: DeviceKind::Keyboard,
            online: true,
            battery: Some(BatteryInfo {
                percentage: 90,
                level: BatteryLevel::Full,
                status: BatteryStatus::Discharging,
            }),
            model_info: None,
            capabilities: Some(Capabilities {
                buttons: true,
                ..Capabilities::default()
            }),
        }],
    }
}

fn is_name(node: &A11yNode, role: Role, name: &str) -> bool {
    node.role == role && node.name.as_deref() == Some(name)
}

#[test]
fn opening_a_device_lands_focus_on_its_selected_section() {
    let window = MainWindow::open(&[k780()]);
    window.focus(Role::Button, "K780");
    let _ = window.app.take_focus_announcements();
    window.open_device("K780");

    // What the screen reader says as the page opens: the section tab, and
    // nothing on the way (no stop on the bare window).
    let announced = window.app.take_focus_announcements();
    assert!(
        !announced.is_empty()
            && announced
                .iter()
                .all(|node| is_name(node, Role::Tab, "Keys") && node.selected == Some(true)),
        "announced while opening: {announced:?}"
    );
    let focused = window.focused();
    assert!(
        is_name(&focused, Role::Tab, "Keys") && focused.selected == Some(true),
        "focus after opening the device: {focused:?}\n{:?}",
        window.app.snapshot()
    );
}

#[test]
fn tab_moves_through_a_device_page() {
    let window = MainWindow::open(&[k780()]);
    window.open_device("K780");
    let landing = window.focused();

    window.app.press("tab");
    assert_ne!(window.focused().id, landing.id, "Tab moved nowhere");
    window.app.press("shift-tab");
    assert_eq!(window.focused().id, landing.id, "Shift+Tab did not return");
}

#[test]
fn going_back_returns_focus_to_the_device_left() {
    for back in ["alt-left", "escape"] {
        let window = MainWindow::open(&[k780()]);
        window.open_device("K780");
        window.app.press(back);

        let tree = window.app.snapshot();
        assert!(
            tree.find(Role::Tab, "Keys").is_none(),
            "{back} did not leave the device page:\n{tree:?}"
        );
        let focused = window.focused();
        assert!(
            is_name(&focused, Role::Button, "K780"),
            "{back}: focus after going back: {focused:?}"
        );
    }
}

/// A second, always-online device so Home keeps a card after the K780 goes.
fn mx_keys() -> DeviceInventory {
    let mut inventory = k780();
    inventory.receiver.unique_id = Some("a11y-unifying-2".to_owned());
    let device = &mut inventory.paired[0];
    device.codename = Some("MX Keys".to_owned());
    device.wpid = Some(0x408a);
    inventory
}

#[test]
fn focus_survives_the_device_page_disappearing() {
    let mut offline = k780();
    offline.paired[0].online = false;
    let window = MainWindow::open(&[offline, mx_keys()]);
    window.open_device("K780");
    let record_key = window.app.update(|cx| {
        AppState::try_read(cx)
            .and_then(AppState::current_record)
            .map(crate::state::DeviceRecord::record_key)
            .expect("K780 is selected")
    });

    // Nobody declared this transition: the device is forgotten under the
    // open page, render pops back Home, and the focused tab is gone.
    window
        .app
        .update(|cx| AppState::apply(cx, |state| state.forget_device(&record_key)));
    window.app.settle();

    let focused = window.focused();
    assert!(
        is_name(&focused, Role::Button, "MX Keys"),
        "focus after the page vanished: {focused:?}\n{:?}",
        window.app.snapshot()
    );
    window.app.press("tab");
    assert_ne!(
        window.focused().id,
        focused.id,
        "Tab is dead after recovery"
    );
}

/// Spec open question 4: gpui-kit's dialog restores focus to the element
/// that had it when the dialog opened.
#[test]
fn closing_a_dialog_returns_focus_to_its_opener() {
    use gpui_component::WindowExt as _;

    let window = MainWindow::open(&[k780()]);
    let card = window.focus(Role::Button, "K780");
    window.app.update_window(|window, cx| {
        window.open_alert_dialog(cx, |alert, _, _| alert.title("Rename"));
    });
    let during = window.app.snapshot();
    assert!(
        during.focused().is_none_or(|node| node.id != card.id),
        "the dialog took focus"
    );
    window.app.press("escape");
    assert_eq!(window.focused().id, card.id, "{:?}", window.app.snapshot());
}

/// Tab from the landing tab until focus comes back around, collecting what a
/// screen reader announces at each stop.
fn tab_stops(window: &MainWindow) -> Vec<A11yNode> {
    let start = window.focused();
    let mut stops = Vec::new();
    for _ in 0..100 {
        window.app.press("tab");
        let node = window.focused();
        if node.id == start.id {
            return stops;
        }
        stops.push(node);
    }
    panic!("Tab never cycled back: {stops:#?}");
}

#[test]
fn the_section_tabs_are_one_tab_stop_that_the_arrows_move_through() {
    let window = MainWindow::open(&[k780()]);
    window.open_device("K780");

    // Only the selected section is a Tab stop: Tab leaves the list for the
    // page, and Shift+Tab from the page comes back to the selected section.
    let stops = tab_stops(&window);
    assert!(
        stops.iter().all(|node| node.role != Role::Tab),
        "Tab stopped on another section: {stops:#?}"
    );

    // The rail is vertical: Down selects the next section and takes focus
    // there, so the screen reader announces the new section as selected and
    // the page below it changes.
    let _ = window.app.take_focus_announcements();
    window.app.press("down");
    let focused = window.focused();
    assert!(
        is_name(&focused, Role::Tab, "Device") && focused.selected == Some(true),
        "focus after Down: {focused:?}\n{:?}",
        window.app.snapshot()
    );
    let announced = window.app.take_focus_announcements();
    assert!(
        announced
            .iter()
            .any(|node| is_name(node, Role::Tab, "Device") && node.selected == Some(true)),
        "announced after Down: {announced:?}"
    );
    assert!(
        window
            .app
            .snapshot()
            .find(Role::Button, "Bind Esc")
            .is_none(),
        "Down did not switch the page to Device"
    );

    // Past the last section, Down stays put; Up, Home and End move back.
    window.app.press("down");
    assert!(is_name(&window.focused(), Role::Tab, "Device"));
    window.app.press("up");
    assert!(is_name(&window.focused(), Role::Tab, "Keys"));
    window.app.press("end");
    assert!(is_name(&window.focused(), Role::Tab, "Device"));
    window.app.press("home");
    let focused = window.focused();
    assert!(
        is_name(&focused, Role::Tab, "Keys") && focused.selected == Some(true),
        "focus after Home: {focused:?}"
    );
    // Left and Right work too, for anyone who reads the rail as a row.
    window.app.press("right");
    assert!(is_name(&window.focused(), Role::Tab, "Device"));
    window.app.press("left");
    assert!(is_name(&window.focused(), Role::Tab, "Keys"));
}

#[test]
fn every_function_key_is_one_named_tab_stop_in_order() {
    let window = MainWindow::open(&[k780()]);
    window.open_device("K780");

    let names: Vec<String> = tab_stops(&window)
        .into_iter()
        .filter_map(|node| node.name)
        .filter(|name| name.starts_with("Bind "))
        .collect();
    let expected: Vec<String> = std::iter::once("Esc".to_owned())
        .chain((1..=19).map(|n| format!("F{n}")))
        .map(|key| format!("Bind {key}"))
        .collect();
    assert_eq!(names, expected, "one stop per key, left to right");

    let tree = window.app.snapshot();
    let f1 = tree.expect(Role::Button, "Bind F1");
    assert_eq!(f1.description.as_deref(), Some("Off"), "{f1:?}");
    assert!(
        tree.unnamed_controls().is_empty(),
        "unnamed controls on the Keys page:\n{tree:?}"
    );
}

#[test]
fn a_key_panel_opens_from_the_keyboard_and_escape_returns_to_its_key() {
    let window = MainWindow::open(&[k780()]);
    window.open_device("K780");
    let key = window.focus(Role::Button, "Bind F1");

    window.app.press("enter");
    let tree = window.app.snapshot();
    let heading = tree.expect(Role::Heading, "Bind F1");
    assert_eq!(heading.level, Some(2));
    let key_now = tree.expect(Role::Button, "Bind F1");
    assert_eq!(
        (key_now.selected, key_now.expanded),
        (Some(true), Some(true))
    );

    window.app.press("escape");
    let tree = window.app.snapshot();
    assert!(
        tree.find(Role::Heading, "Bind F1").is_none(),
        "panel still open:\n{tree:?}"
    );
    assert!(
        tree.find(Role::Tab, "Keys").is_some(),
        "Escape left the device page"
    );
    assert_eq!(
        window.focused().id,
        key.id,
        "focus did not return to the key"
    );
    assert_eq!(window.focused().expanded, Some(false));

    window.app.press("escape");
    assert!(
        window.app.snapshot().find(Role::Tab, "Keys").is_none(),
        "a second Escape goes back"
    );
}

#[test]
fn a_key_panel_opens_onto_its_current_choice() {
    use gpui::accesskit::Toggled;

    let window = MainWindow::open(&[k780()]);
    window.open_device("K780");
    window.focus(Role::Button, "Bind F2");

    // Nothing is bound yet: focus goes into the panel, onto its first
    // choice, and every choice reads as a radio item that is not checked
    // (the state Windows reports; a menu item's "selected" never reaches it).
    window.app.press("space");
    let focused = window.focused();
    assert!(
        is_name(&focused, Role::MenuItemRadio, "Left Click")
            && focused.toggled == Some(Toggled::False),
        "focus after opening the panel: {focused:?}\n{:?}",
        window.app.snapshot()
    );

    // The arrows walk the choices.
    window.app.press("down");
    assert!(
        is_name(&window.focused(), Role::MenuItemRadio, "Right Click"),
        "focus after Down: {:?}",
        window.focused()
    );
    window.app.press("up");
    assert!(is_name(
        &window.focused(),
        Role::MenuItemRadio,
        "Left Click"
    ));

    // Choosing one checks it, and the panel reopens onto it.
    window.focus(Role::MenuItemRadio, "Copy");
    window.app.press("enter");
    let copy = window
        .app
        .snapshot()
        .expect(Role::MenuItemRadio, "Copy")
        .clone();
    assert_eq!(copy.toggled, Some(Toggled::True), "{copy:?}");
    window.app.press("escape");
    assert!(is_name(&window.focused(), Role::Button, "Bind F2"));
    window.app.press("space");
    let focused = window.focused();
    assert!(
        is_name(&focused, Role::MenuItemRadio, "Copy") && focused.toggled == Some(Toggled::True),
        "focus after reopening the panel: {focused:?}"
    );
}

#[test]
fn a_mouse_button_opens_its_inspector_and_escape_steps_back_out() {
    use gpui::accesskit::Toggled;

    let window = every_fixture_device();
    window.open_device("Synthetic Performance Mouse");

    // Opening a button takes focus to what it does now.
    window.focus(Role::Button, "Bind Middle Click");
    window.app.press("space");
    let focused = window.focused();
    assert!(
        is_name(&focused, Role::Button, "Current action: Middle Click")
            && focused.expanded == Some(false),
        "focus after opening Middle Click: {focused:?}\n{:?}",
        window.app.snapshot()
    );

    // Opening the picker lands on the current action, checked.
    window.app.press("space");
    let focused = window.focused();
    assert!(
        is_name(&focused, Role::MenuItemRadio, "Middle Click")
            && focused.toggled == Some(Toggled::True),
        "focus after opening the picker: {focused:?}\n{:?}",
        window.app.snapshot()
    );

    // Escape closes the picker back onto its card, then the inspector back
    // onto the button, and only then leaves the page.
    window.app.press("escape");
    assert!(
        is_name(
            &window.focused(),
            Role::Button,
            "Current action: Middle Click"
        ),
        "focus after the first Escape: {:?}",
        window.focused()
    );
    window.app.press("escape");
    assert!(
        is_name(&window.focused(), Role::Button, "Bind Middle Click"),
        "focus after the second Escape: {:?}",
        window.focused()
    );
    assert!(
        window.app.snapshot().find(Role::Tab, "Buttons").is_some(),
        "Escape left the device page"
    );

    // A gesture button lands on its selected direction, a checked choice.
    window.focus(Role::Button, "Bind Gesture Button");
    window.app.press("space");
    let focused = window.focused();
    assert!(
        focused.role == Role::MenuItemRadio
            && focused
                .name
                .as_deref()
                .is_some_and(|n| n.starts_with("Click: "))
            && focused.toggled == Some(Toggled::True),
        "focus after opening the gesture button: {focused:?}\n{:?}",
        window.app.snapshot()
    );
}

/// The node's ancestors, nearest first, as `(role, name)`.
fn ancestors(window: &MainWindow, node: &A11yNode) -> Vec<(Role, Option<String>)> {
    let tree = window.app.snapshot();
    let mut out = Vec::new();
    let mut parent = node.parent;
    while let Some(id) = parent {
        let Some(next) = tree.nodes().iter().find(|n| n.id == id) else {
            break;
        };
        out.push((next.role, next.name.clone()));
        parent = next.parent;
    }
    out
}

fn is_group(ancestor: &(Role, Option<String>), name: &str) -> bool {
    ancestor.0 == Role::Group && ancestor.1.as_deref() == Some(name)
}

#[test]
fn a_key_panel_names_its_key_and_the_category_of_each_choice() {
    let window = MainWindow::open(&[k780()]);
    window.open_device("K780");
    window.focus(Role::Button, "Bind F2");
    window.app.press("space");
    let focused = window.focused();
    let groups = ancestors(&window, &focused);
    assert!(
        groups.iter().any(|a| is_group(a, "Mouse"))
            && groups.iter().any(|a| is_group(a, "Bind F2")),
        "{focused:?} sits in {groups:?}"
    );
}

#[test]
fn single_choices_read_as_checked_radio_buttons_in_a_named_group() {
    use gpui::accesskit::Toggled;

    let window = every_fixture_device();
    window.open_device("Synthetic Performance Mouse");

    // The Actions Ring: the slot being edited, its icon and its action.
    click(&window, Role::Tab, "Actions Ring");
    let tree = window.app.snapshot();
    let slots: Vec<_> = tree
        .nodes()
        .iter()
        .filter(|n| {
            n.role == Role::RadioButton && n.name.as_deref().is_some_and(|s| s.starts_with("Top"))
        })
        .cloned()
        .collect();
    assert!(
        slots.iter().any(|n| n.toggled == Some(Toggled::True)),
        "no checked ring slot: {slots:?}\n{tree:?}"
    );
    let default_icon = tree.expect(Role::RadioButton, "Use action icon").clone();
    let editor = ancestors(&window, &default_icon);
    assert!(
        editor.iter().any(|a| is_group(a, "Icon")) && editor.iter().any(|a| is_group(a, "Top")),
        "the icon choices sit in {editor:?}"
    );
    assert!(
        tree.nodes()
            .iter()
            .any(|n| n.role == Role::MenuItemRadio && n.toggled == Some(Toggled::True)),
        "the slot's current action is not checked:\n{tree:?}"
    );

    // Keyboard lighting colours. One test app at a time: each holds the
    // locale lock.
    drop(window);
    let window = every_fixture_device();
    window.open_device("Synthetic RGB Keyboard");
    click(&window, Role::Tab, "Lighting");
    let tree = window.app.snapshot();
    let red = tree.expect(Role::RadioButton, "Red").clone();
    assert!(
        is_group(&ancestors(&window, &red)[0], "Color"),
        "Red is not in the Color group:\n{tree:?}"
    );
}

#[test]
fn add_device_lands_on_each_step_with_its_status_and_escape_closes_it() {
    use crate::windows::add_device::{AddDeviceView, PairingUi};
    use crate::windows::aux_root;

    let window = MainWindow::open_root(&[k780()], &[], aux_root(AddDeviceView::new));
    window.app.update(|cx| cx.set_global(PairingUi::Searching));
    window.app.settle();
    let focused = window.focused();
    assert!(
        focused.role == Role::Button
            && focused
                .name
                .as_deref()
                .is_some_and(|n| n.starts_with("Cancel. Searching for devices")),
        "focus while searching: {focused:?}\n{:?}",
        window.app.snapshot()
    );

    // Steps can follow each other within one frame (a search that finds a
    // device at once): landing twice must still land on the first control.
    window.app.update(|cx| cx.set_global(PairingUi::Idle));
    window.app.update(|cx| cx.set_global(PairingUi::Searching));
    // Devices turn up while Cancel (which both steps show) has focus: focus
    // still lands on the new step's first control, the first device.
    window.app.update(|cx| {
        cx.set_global(PairingUi::Found(vec![openlogi_ipc::FoundDevice {
            address: [1, 2, 3, 4, 5, 6],
            name: "ERGO K860".to_owned(),
        }]));
    });
    window.app.settle();
    let focused = window.focused();
    assert!(
        is_name(&focused, Role::Button, "ERGO K860")
            && focused
                .description
                .as_deref()
                .is_some_and(|d| d.starts_with("Searching for devices")),
        "focus once a device was found: {focused:?}"
    );

    // The step changes under the focused Cancel: focus lands on the new
    // step's first control instead of falling to the window.
    window.app.update(|cx| cx.set_global(PairingUi::Idle));
    window.app.settle();
    let focused = window.focused();
    assert!(
        focused.role == Role::Button
            && focused
                .name
                .as_deref()
                .is_some_and(|n| n.starts_with("Search for devices. ")),
        "focus after the search ended: {focused:?}"
    );
}

/// Every device the canonical synthetic fixture describes, plus the K780.
fn every_fixture_device() -> MainWindow {
    use openlogi_fixture::{CANONICAL_DEVICE_PROFILE_JSON, DeviceProfile};
    let profile: DeviceProfile =
        serde_json::from_str(CANONICAL_DEVICE_PROFILE_JSON).expect("canonical profile parses");
    let mut inventories = profile.inventories.clone();
    inventories.push(k780());
    MainWindow::open_with(&inventories, &profile.standalone)
}

/// Activate the control, as a screen reader's default action does.
#[track_caller]
fn click(window: &MainWindow, role: Role, name: &str) {
    window.focus(role, name);
    window.app.press("enter");
}

/// The in-process counterpart of the spec's UIA smoke check: Home in every
/// layout and every section of every device, with no control a screen reader
/// cannot name.
#[test]
fn no_screen_has_an_unnamed_control() {
    use std::fmt::Write as _;

    let window = every_fixture_device();
    let mut report = String::new();
    let mut check = |screen: &str, window: &MainWindow| {
        let tree = window.app.snapshot();
        for node in tree.unnamed_controls() {
            let _ = writeln!(report, "{screen}: {node:?}");
        }
    };

    for layout in ["List", "Carousel", "Grid"] {
        click(&window, Role::Button, layout);
        check(&format!("Home ({layout})"), &window);
    }

    let devices: Vec<String> = window
        .app
        .update(|cx| AppState::try_read(cx).map(|state| state.devices().to_vec()))
        .unwrap_or_default()
        .into_iter()
        .map(|record| record.display_name)
        .collect();
    for device in &devices {
        window.open_device(device);
        let tree = window.app.snapshot();
        let rail = tree
            .nodes()
            .iter()
            .position(|node| node.role == Role::TabList)
            .expect("the device page has a section rail");
        let tabs: Vec<String> = tree.nodes()[rail + 1..]
            .iter()
            .take_while(|node| node.depth > tree.nodes()[rail].depth)
            .filter(|node| node.role == Role::Tab)
            .filter_map(|node| node.name.clone())
            .collect();
        for tab in &tabs {
            click(&window, Role::Tab, tab);
            check(&format!("{device} › {tab}"), &window);
        }
        window.app.press("escape");
    }

    assert!(report.is_empty(), "unnamed controls:\n{report}");
}

/// The auxiliary windows reachable without the Settings sidebar, which
/// gpui-kit does not expose to assistive technology yet.
#[test]
fn auxiliary_windows_name_every_control_they_own() {
    use crate::windows::add_device::AddDeviceView;
    use crate::windows::aux_root;
    use crate::windows::settings::{SettingsPage, SettingsView};
    use crate::windows::update_consent::UpdateConsentView;

    // One window at a time: each test app holds the process-wide locale lock.
    let settings = |page: SettingsPage| {
        Box::new(move || {
            MainWindow::open_root(
                &[k780()],
                &[],
                aux_root(move |window, cx| SettingsView::new(page, window, cx)),
            )
        }) as Box<dyn Fn() -> MainWindow>
    };
    let windows: Vec<Box<dyn Fn() -> MainWindow>> = vec![
        settings(SettingsPage::General),
        settings(SettingsPage::Updates),
        settings(SettingsPage::About),
        Box::new(|| MainWindow::open_root(&[k780()], &[], aux_root(AddDeviceView::new))),
        Box::new(|| MainWindow::open_root(&[k780()], &[], aux_root(UpdateConsentView::new))),
    ];

    for open in &windows {
        let window = open();
        let tree = window.app.snapshot();
        let nodes = tree.nodes();
        let unnamed: Vec<_> = tree
            .unnamed_controls()
            .into_iter()
            .filter(|node| {
                // gpui-kit's own SettingPage reset button, right after the
                // Settings search field; see the spec's upstream asks.
                let index = nodes.iter().position(|n| n.id == node.id).unwrap_or(0);
                !(index > 0 && nodes[index - 1].role == Role::TextInput)
            })
            .collect();
        assert!(
            unnamed.is_empty(),
            "unnamed controls: {unnamed:?}\n{tree:?}"
        );
    }
}

/// The first-run consent window: its two buttons ("Not now", "Enable") mean
/// nothing without the question, so the question must be readable too.
#[test]
fn the_update_consent_question_is_readable() {
    use crate::windows::aux_root;
    use crate::windows::update_consent::UpdateConsentView;

    let window = MainWindow::open_root(&[k780()], &[], aux_root(UpdateConsentView::new));
    let tree = window.app.snapshot();
    tree.expect(Role::Heading, "Check for updates?");
    assert!(
        tree.nodes().iter().any(|node| node.role == Role::Label
            && node
                .name
                .as_deref()
                .is_some_and(|text| text.starts_with("OpenLogi can check GitHub"))),
        "the explanation is not in the tree:\n{tree:?}"
    );
}
