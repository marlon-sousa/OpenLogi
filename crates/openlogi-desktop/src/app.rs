use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    MouseButton, NavigationDirection, ParentElement, Render, Styled, Subscription, Window, div,
    prelude::FluentBuilder as _, rgb,
};
use gpui_base::Button as BaseButton;
use gpui_component::{
    Icon, IconName, TitleBar,
    button::{Button, ButtonVariants as _},
    v_flex,
};
use openlogi_core::device::{Capabilities, DeviceKind};
use openlogi_ipc::InventoryHealth;
use tracing::info;

use self::menu::{APP_KEY_CONTEXT, CloseWindow, Minimize, NavigateBack, Zoom};
use crate::features::action_ring::ActionRingPanel;
use crate::features::camera::controls::CameraControlsPanel;
use crate::features::camera::preview::CameraPreview;
use crate::features::keyboard::function_row::FunctionRowView;
use crate::features::lighting::keyboard_rgb::LightingPanel;
use crate::features::lighting::standalone::LightPanel;
use crate::features::mouse::view::MouseModelView;
use crate::features::pointer::dpi::DpiPanel;
use crate::features::pointer::smartshift::SmartShiftPanel;
use crate::features::profiles::{AppCatalogPicker, ProfileIconCache};
use crate::services::assets::user_cache_root;
use crate::state::{AgentLink, AppState, DeviceRecord, StateEvent, StateEvents};
use crate::ui::focus::{FocusTargets, keep_focus};
use crate::ui::theme::{self, ContentWidth, Typography as _};

pub(crate) mod deeplink;
mod detail;
mod home;
pub(crate) mod menu;
mod status;
mod widgets;

// The mouse diagram paints the same keyboard-lighting glow as the Home
// gallery card, so it reaches these through the crate-stable `crate::app::…`
// path rather than the internal `app::home` submodule.
pub(crate) use home::{glow_canvas, keyboard_glow};
/// Which screen the root view is showing.
///
/// GPUI has no router, so navigation is a tiny view-local enum that selects
/// which subtree [`AppView::render`] builds. It is deliberately *not* in
/// [`AppState`]: the route is pure UI presentation, whereas
/// [`AppState::current_record`] is functional (it drives the hook bindings,
/// DPI, and persisted selection). The detail route is keyed by the record's
/// user-facing identity rather than an index so a hot-plug that reorders
/// or drops the device list can't silently swap the user onto another device —
/// including a same-model camera that shares its settings key. Render validates
/// the key against the live selection and pops back to [`Route::Home`] when it
/// no longer matches.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Route {
    /// The device gallery.
    Home,
    /// A single device's settings, identified by its user-facing record key.
    Device { record_key: String },
}

/// The active section of the device-detail screen. Backs the detail `TabBar`;
/// reset to the device's first tab whenever a device is opened.
///
/// The tab *set* depends on the device kind — see [`DetailTab::tabs_for`]. A
/// mouse gets button-mapping + pointer tuning; a wired keyboard gets RGB
/// lighting; every device gets the info tab. Tailoring the tabs is what keeps a
/// keyboard from rendering a mouse silhouette and an irrelevant DPI panel
/// (issue #19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DetailTab {
    /// The mouse model with clickable button hotspots.
    Buttons,
    /// Cursor-centred eight-slot action launcher.
    ActionsRing,
    /// The keyboard function-row remapper with clickable F-key bubbles.
    Keys,
    /// Pointer tuning — DPI and presets.
    Pointer,
    /// RGB lighting — color, brightness, on/off.
    Lighting,
    /// Live webcam preview (UVC cameras only).
    Camera,
    /// Standalone light controls driven by a raw-HID device driver.
    Light,
    /// Device info and configuration.
    Device,
}

impl DetailTab {
    /// The detail sections shown for `record`, in tab order. Always non-empty:
    /// every device gets at least the info tab.
    ///
    /// Each panel is gated on the device's actual [`Capabilities`] — the HID++
    /// features it announced — not on its [`DeviceKind`]. A panel shows iff the
    /// device can do that thing, so a misclassified device can't lose its
    /// panels (issue #127). Devices we never probed (offline at startup) have no
    /// measured capabilities; we presume a set from their kind so a sleeping
    /// mouse still shows its (host-side) button bindings.
    ///
    /// The Buttons panel renders a mouse-model silhouette with hotspots. It is
    /// only useful for pointer-type devices; keyboards get the Keys panel
    /// instead, even when they expose ReprogControls over HID++.
    fn tabs_for(record: &DeviceRecord) -> Vec<Self> {
        let caps = record
            .capabilities
            .unwrap_or_else(|| Capabilities::presumed_from_kind(record.kind));
        // Buttons panel is a mouse-model silhouette — only for pointer devices.
        // Keyboards get the Keys panel instead, even when they expose ReprogControls.
        let can_show_mouse_model = matches!(record.kind, DeviceKind::Mouse | DeviceKind::Trackball);
        let mut tabs = Vec::new();
        // A webcam is a UVC device with no HID++ capabilities; its detail screen
        // leads with the live preview, then the generic info tab.
        if matches!(record.kind, DeviceKind::Camera) {
            tabs.push(Self::Camera);
        }
        if caps.buttons && can_show_mouse_model {
            tabs.push(Self::Buttons);
        }
        if caps.haptic_panel || (caps.buttons && can_show_mouse_model) {
            tabs.push(Self::ActionsRing);
        }
        // Function-row remapper when the keyboard reports remappable buttons.
        if matches!(record.kind, DeviceKind::Keyboard) && caps.buttons {
            tabs.push(Self::Keys);
        }
        if caps.pointer {
            tabs.push(Self::Pointer);
        }
        if caps.lighting {
            tabs.push(Self::Lighting);
        }
        if record.light_capabilities.is_some() {
            tabs.push(Self::Light);
        }
        tabs.push(Self::Device);
        tabs
    }

    /// The first (default) tab for `record` — what a freshly opened device shows.
    fn default_for(record: &DeviceRecord) -> Self {
        Self::tabs_for(record)
            .first()
            .copied()
            .unwrap_or(Self::Device)
    }

    fn label(self) -> gpui::SharedString {
        match self {
            Self::Buttons => tr!("device.buttons"),
            Self::ActionsRing => tr!("action_ring.actions_ring"),
            Self::Keys => tr!("device.keys"),
            Self::Pointer => tr!("device.pointer"),
            Self::Lighting | Self::Light => tr!("device.lighting"),
            Self::Camera => tr!("camera.camera"),
            Self::Device => tr!("device.device"),
        }
    }
}

/// Root application view.
pub struct AppView {
    focus_handle: FocusHandle,
    route: Route,
    mouse_model: Entity<MouseModelView>,
    action_ring_panel: Entity<ActionRingPanel>,
    keyboard_model: Entity<FunctionRowView>,
    dpi_panel: Entity<DpiPanel>,
    smartshift_panel: Entity<SmartShiftPanel>,
    lighting_panel: Entity<LightingPanel>,
    camera_preview: Entity<CameraPreview>,
    camera_controls: Entity<CameraControlsPanel>,
    light_panel: Entity<LightPanel>,
    profile_icons: ProfileIconCache,
    app_catalog: Entity<AppCatalogPicker>,
    /// Redraw the profile picker after discovery, filtering, or expansion changes.
    _app_catalog_obs: Subscription,
    appearance_obs: Option<Subscription>,
    /// Invalidates the root only for semantic state changes its current route
    /// reads; feature entities subscribe to their own events directly.
    #[expect(dead_code, reason = "held to keep the AppState subscription alive")]
    state_obs: Subscription,
    /// Whether the last frame was the fail-closed configuration-error screen.
    /// A successful save must redraw that screen even though the error is gone.
    config_issue_visible: bool,
    accessibility_dismissed: bool,
    /// Which section of the device-detail screen is showing.
    active_tab: DetailTab,
    /// Landing targets: each Home device card by record key, and each section
    /// tab of the device page. See [`crate::ui::focus`].
    card_focus: FocusTargets<String>,
    section_focus: FocusTargets<DetailTab>,
    _keep_focus: Subscription,
}

impl Focusable for AppView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AppView {
    /// Construct the root view and its child entities.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        // `AppState` is installed as an entity by `main` (with the IPC command
        // sender) before any window opens, so there is no fallback state here.

        let state = AppState::global(cx);
        {
            let state = state.read(cx);
            if let Some(record) = state.current_record() {
                info!(
                    device_key = %record.config_key,
                    display = %record.display_name,
                    "initial device selected"
                );
            } else {
                info!(
                    root = ?user_cache_root(),
                    "no devices with HID++ model info — using synthetic silhouette"
                );
            }
        }

        let mouse_model = cx.new(|cx| MouseModelView::new(window, cx));
        let action_ring_panel = cx.new(ActionRingPanel::new);
        let keyboard_model = cx.new(FunctionRowView::new);
        let dpi_panel = cx.new(DpiPanel::new);
        let smartshift_panel = cx.new(SmartShiftPanel::new);
        let lighting_panel = cx.new(LightingPanel::new);
        let camera_preview = cx.new(CameraPreview::new);
        let camera_controls = cx.new(CameraControlsPanel::new);
        let light_panel = cx.new(LightPanel::new);
        let profile_icons = ProfileIconCache::default();
        let app_catalog = cx.new(|cx| AppCatalogPicker::new(profile_icons.clone(), window, cx));
        let app_catalog_obs = cx.observe(&app_catalog, |_, _, cx| cx.notify());
        let state_obs = cx.subscribe(&state, |view, _, event: &StateEvent, cx| {
            let is_current =
                |key| AppState::try_read(cx).is_some_and(|state| state.is_current_device(key));
            let on_home = matches!(view.route, Route::Home);
            let relevant = match event {
                StateEvent::AgentChanged
                | StateEvent::InventoryChanged
                | StateEvent::DeviceSelected(_) => true,
                StateEvent::ForegroundChanged => !on_home,
                StateEvent::BindingsChanged(key) => {
                    !on_home
                        && matches!(
                            view.active_tab,
                            DetailTab::Buttons | DetailTab::ActionsRing | DetailTab::Device
                        )
                        && is_current(key)
                }
                StateEvent::DpiChanged(key) => {
                    !on_home && view.active_tab == DetailTab::Device && is_current(key)
                }
                StateEvent::LightingChanged(key) => {
                    on_home || (view.active_tab == DetailTab::Light && is_current(key))
                }
                StateEvent::DeviceConfigChanged(key) => {
                    on_home
                        || (matches!(view.active_tab, DetailTab::Pointer | DetailTab::Device)
                            && is_current(key))
                }
                StateEvent::CameraChanged => on_home || view.active_tab == DetailTab::Light,
                // Child entities own these surfaces and subscribe directly. A
                // language switch already refreshes every window, and the root
                // caches no localized text.
                StateEvent::SmartShiftChanged(_)
                | StateEvent::CameraPermissionChanged
                | StateEvent::DiagnosticsChanged
                | StateEvent::LanguageChanged => false,
                // App-wide settings render in their own window. The root only
                // cares when a persistence/reload failure opens or closes its
                // fail-closed configuration-error screen.
                StateEvent::SettingsChanged => {
                    view.config_issue_visible
                        || AppState::try_read(cx)
                            .is_some_and(|state| state.config_issue().is_some())
                }
            };
            if relevant {
                cx.notify();
            }
        });
        let keep_focus_sub = keep_focus(window, cx, focus_handle.clone(), |this, _, cx| {
            this.landing_target(cx)
        });
        Self {
            focus_handle,
            route: Route::Home,
            mouse_model,
            action_ring_panel,
            keyboard_model,
            dpi_panel,
            smartshift_panel,
            lighting_panel,
            camera_preview,
            camera_controls,
            light_panel,
            profile_icons,
            app_catalog,
            _app_catalog_obs: app_catalog_obs,
            appearance_obs: None,
            state_obs,
            config_issue_visible: false,
            accessibility_dismissed: false,
            active_tab: DetailTab::Buttons,
            card_focus: FocusTargets::default(),
            section_focus: FocusTargets::default(),
            _keep_focus: keep_focus_sub,
        }
    }

    /// Keep the OS-appearance observer alive.
    pub fn set_appearance_obs(&mut self, sub: Subscription) {
        self.appearance_obs = Some(sub);
    }

    /// Switch screens. The one route transition: it moves keyboard focus to
    /// the new screen's landing target in the same step, because the element
    /// that held focus is not rendered on the new route (see
    /// [`crate::ui::focus`]).
    ///
    /// Opening a device makes it the functionally active device too (hook
    /// bindings, DPI, and the persisted selection follow
    /// [`AppState::select_device`]) and lands on its first section tab.
    /// Returning Home leaves the selection untouched — the route is purely
    /// presentational — and lands on the card of the device just left.
    fn navigate(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        let left = match &self.route {
            Route::Device { record_key } => Some(record_key.clone()),
            Route::Home => None,
        };
        if let Route::Device { record_key } = &route {
            AppState::apply(cx, |state| {
                state
                    .devices()
                    .iter()
                    .position(|record| record.record_key() == *record_key)
                    .map_or_else(StateEvents::none, |idx| state.select_device(idx))
            });
            AppState::load_current_device_reads(cx);
            // Land on the device's first relevant tab — Buttons for a mouse,
            // Lighting for a wired keyboard, Device for everything else.
            self.active_tab = AppState::try_global(cx)
                .map(|state| state.read(cx))
                .and_then(AppState::current_record)
                .map_or(DetailTab::Device, DetailTab::default_for);
        }
        self.route = route;
        let target = match (&self.route, left) {
            (Route::Home, Some(left)) if Self::device_listed(&left, cx) => {
                Some(self.card_focus.handle(&left, cx))
            }
            _ => self.landing_target(cx),
        };
        target
            .unwrap_or_else(|| self.focus_handle.clone())
            .focus(window, cx);
        cx.notify();
    }

    /// Where focus belongs on the current route when nothing more specific
    /// was declared: the device page's selected section tab, or the active
    /// device's Home card. `None` only when neither is rendered.
    fn landing_target(&mut self, cx: &mut Context<Self>) -> Option<gpui::FocusHandle> {
        match &self.route {
            Route::Device { .. } => {
                let tab = self.visible_tab(cx)?;
                Some(self.section_focus.handle(&tab, cx))
            }
            Route::Home => {
                let key = AppState::try_read(cx)
                    .and_then(|state| state.current_record().or_else(|| state.devices().first()))
                    .map(DeviceRecord::record_key)?;
                Some(self.card_focus.handle(&key, cx))
            }
        }
    }

    /// The section the device page shows: the stored tab while it belongs to
    /// the live device, else that device's first tab. `None` when the route's
    /// device is gone, which render turns back into Home.
    fn visible_tab(&self, cx: &App) -> Option<DetailTab> {
        let Route::Device { record_key } = &self.route else {
            return None;
        };
        let record = AppState::try_read(cx)
            .and_then(AppState::current_record)
            .filter(|record| record.record_key() == *record_key)?;
        let tabs = DetailTab::tabs_for(record);
        Some(if tabs.contains(&self.active_tab) {
            self.active_tab
        } else {
            tabs.first().copied().unwrap_or(DetailTab::Device)
        })
    }

    fn device_listed(record_key: &str, cx: &App) -> bool {
        AppState::try_read(cx).is_some_and(|state| {
            state
                .devices()
                .iter()
                .any(|record| record.record_key() == record_key)
        })
    }

    /// Open `record_key`'s device page.
    fn open_device(&mut self, record_key: String, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(Route::Device { record_key }, window, cx);
    }

    /// Return to the gallery from a device page; a no-op on Home.
    fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.route, Route::Home) {
            self.navigate(Route::Home, window, cx);
        }
    }

    /// Attach the window-level back-navigation listeners to `root`: a mouse
    /// configurator should honor the hardware it configures. Two routes reach
    /// us — the native navigate button (its default binding never diverts, so
    /// the OS still sees it), and the contextual [`NavigateBack`] action, bound
    /// to Alt+Left (what a rebound BrowserBack action injects on Linux and what
    /// keyboard users expect) and Escape.
    fn with_back_navigation(root: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        root.on_mouse_down(
            MouseButton::Navigate(NavigationDirection::Back),
            cx.listener(|this, _, window, cx| this.go_back(window, cx)),
        )
        .on_action(cx.listener(|this, _: &NavigateBack, window, cx| {
            if matches!(this.route, Route::Home) {
                // Escape on Home is nobody's; let an outer handler have it.
                cx.propagate();
            } else {
                this.go_back(window, cx);
            }
        }))
    }

    fn accessibility_gate(cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette(cx);
        v_flex()
            .size_full()
            .bg(pal.page)
            .text_color(pal.text_primary)
            .items_center()
            .justify_center()
            .gap_4()
            .p_8()
            .child(
                Icon::new(IconName::TriangleAlert)
                    .size_8()
                    .text_color(rgb(theme::STATUS_CONNECTING)),
            )
            .child(
                div()
                    .text_title()
                    .child(tr!("permissions.accessibility_permission_required")),
            )
            .child(
                div()
                    .max_w(ContentWidth::Narrow.rems())
                    .text_body()
                    .text_color(pal.text_muted)
                    .child(tr!("permissions.accessibility_permission_description")),
            )
            .child(
                div()
                    .max_w(ContentWidth::Narrow.rems())
                    .text_body()
                    .text_color(pal.text_muted)
                    .child(tr!("permissions.accessibility_agent_instructions")),
            )
            .child(
                Button::new("open-accessibility")
                    .primary()
                    .icon(IconName::Settings)
                    .label(tr!("permissions.open_system_settings_to_grant_access"))
                    .on_click(|_, _, cx| request_accessibility(cx)),
            )
            .child(
                div()
                    .text_caption()
                    .text_color(pal.text_muted)
                    .child(tr!("permissions.permission_applies_without_restart")),
            )
            .child(
                BaseButton::new("skip-accessibility")
                    .accessibility_label(tr!("permissions.not_now_use_dpi_and_other_features_only"))
                    .text_caption()
                    .text_color(pal.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(pal.text_primary))
                    .focus_visible(|s| s.text_color(pal.text_primary))
                    .child(tr!("permissions.not_now_use_dpi_and_other_features_only"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.accessibility_dismissed = true;
                        cx.notify();
                    })),
            )
    }
}

fn request_accessibility(cx: &mut App) {
    use openlogi_permissions::{self as permissions, Permission};
    // Ask the *agent* to fire the prompt (it owns the hook, so the system dialog
    // must name and authorize openlogi-agent — prompting in the GUI would grant
    // the wrong binary), then open the System Settings pane so the user can flip
    // the switch. Shared by the gate button, the footer, and the Settings window.
    if let Some(state) = AppState::try_global(cx) {
        state.read(cx).request_accessibility_prompt();
    }
    permissions::open_pane(Permission::Accessibility);
}

/// Client-side main-window titlebar: window controls (minimize / maximize /
/// close on Linux + Windows), the drag region, and the app name centred.
/// Replaces the native titlebar so Linux — where the compositor declines
/// server-side decorations and gpui falls back to client-side ones it doesn't
/// paint — still gets a titlebar and window controls. On macOS the widget
/// reserves the traffic-light space.
fn app_title_bar(cx: &App) -> impl IntoElement {
    let pal = theme::palette(cx);
    TitleBar::new().child(
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_body()
            .text_color(pal.text_muted)
            .child("OpenLogi"),
    )
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.render_content(window, cx);
        // Root owns dialog state but the application must mount its layer.
        // Keep it present across connecting, permission, and error screens too.
        div()
            .relative()
            .size_full()
            .child(content)
            .children(gpui_component::Root::render_dialog_layer(window, cx))
    }
}

impl AppView {
    #[expect(
        clippy::too_many_lines,
        reason = "root view assembles every screen branch inline"
    )]
    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        theme::apply_ui_scale(window, cx);
        let pal = theme::palette(cx);

        // Every frame — including the pre-connection and error frames — hangs
        // off this root, so the window actions (⌘W / ⌘M / zoom) work from the
        // first frame on, not only once the full UI is up.
        let root = v_flex()
            .size_full()
            .bg(pal.page)
            .text_color(pal.text_primary)
            .tab_group()
            .track_focus(&self.focus_handle)
            .key_context(APP_KEY_CONTEXT)
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window())
            // Linux only: a client-side titlebar (window controls + drag region)
            // as the first row of every frame — including the pre-connection and
            // error frames — so the chrome is present from the first frame on.
            // macOS / Windows keep their native titlebar.
            .when(cfg!(target_os = "linux"), |this| {
                this.child(app_title_bar(cx))
            });
        let root = Self::with_back_navigation(root, cx);

        let config_issue = AppState::try_global(cx)
            .map(|state| state.read(cx))
            .and_then(AppState::config_issue)
            .map(gpui::SharedString::from);
        self.config_issue_visible = config_issue.is_some();
        if let Some(issue) = config_issue {
            window.set_window_title("OpenLogi");
            return root
                .child(status::config_issue_body(issue, cx))
                .into_any_element();
        }

        // The agent is the source of truth for both the permission state and
        // the device list; `AgentLink` is everything the GUI knows about it.
        // Until the first snapshot lands, hold a neutral connecting frame:
        // rendering the permission gate (and then the empty state) off
        // assumed-denied defaults flashed both screens at every already-set-up
        // user on launch. A missing global reads the same way — "nothing is
        // known yet".
        let link = AppState::try_global(cx)
            .map(|state| state.read(cx))
            .map_or(AgentLink::Connecting, |s| s.agent_link().clone());
        let status = match link {
            AgentLink::Connecting => {
                window.set_window_title("OpenLogi");
                return root.child(status::connecting_body(cx)).into_any_element();
            }
            AgentLink::Unreachable => {
                window.set_window_title("OpenLogi");
                return root.child(status::unreachable_body(cx)).into_any_element();
            }
            AgentLink::OutdatedGui => {
                window.set_window_title("OpenLogi");
                return root.child(status::outdated_gui_body(cx)).into_any_element();
            }
            AgentLink::Ready(status) => status,
        };

        let granted = status.accessibility_granted;
        if !granted && !self.accessibility_dismissed {
            window.set_window_title("OpenLogi");
            return root.child(Self::accessibility_gate(cx)).into_any_element();
        }

        let has_device = AppState::try_global(cx)
            .map(|state| state.read(cx))
            .is_some_and(|s| !s.devices().is_empty());

        // Resolve the route. A detail route lives only while its device is
        // still the live selection; if a hot-plug dropped or reordered it (or
        // the selection fell back to another device) pop quietly back to the
        // gallery rather than render a different device under the same screen.
        let show_device = match &self.route {
            Route::Home => false,
            Route::Device { record_key } => AppState::try_global(cx)
                .map(|state| state.read(cx))
                .and_then(AppState::current_record)
                .is_some_and(|record| record.record_key() == *record_key),
        };
        if !show_device {
            self.route = Route::Home;
        }

        window.set_window_title(&widgets::main_window_title(show_device, cx));

        let (header_el, content_el) = if show_device {
            // Resolve the active section once for both the navigation rail and
            // its workspace — and, through the same resolver, for the focus
            // landing target.
            let record = AppState::try_read(cx)
                .and_then(AppState::current_record)
                .cloned();
            let tabs = record
                .as_ref()
                .map_or_else(|| vec![DetailTab::Device], DetailTab::tabs_for);
            let active = self.visible_tab(cx).unwrap_or(DetailTab::Device);
            // Run the camera only while its live-preview tab is the one on screen;
            // any other tab, device, or Home tears the session down (LED off).
            // Use capture_id (OS open id), not config_key — the latter prefers
            // the port-stable USB serial and is not a valid AVFoundation id.
            let camera_target = if active == DetailTab::Camera {
                record
                    .as_ref()
                    .filter(|r| matches!(r.kind, DeviceKind::Camera))
                    .and_then(|r| r.capture_id.clone())
            } else {
                None
            };
            self.camera_preview
                .update(cx, |preview, cx| preview.set_target(camera_target, cx));
            (
                detail::detail_header(record.as_ref(), cx).into_any_element(),
                detail::detail_content(
                    &detail::DetailPanels {
                        mouse_model: &self.mouse_model,
                        action_ring: &self.action_ring_panel,
                        keyboard_model: &self.keyboard_model,
                        dpi_panel: &self.dpi_panel,
                        smartshift_panel: &self.smartshift_panel,
                        lighting_panel: &self.lighting_panel,
                        camera_preview: &self.camera_preview,
                        camera_controls: &self.camera_controls,
                        light_panel: &self.light_panel,
                    },
                    &mut self.section_focus,
                    &self.profile_icons,
                    &self.app_catalog,
                    &tabs,
                    active,
                    cx,
                )
                .into_any_element(),
            )
        } else {
            self.camera_preview
                .update(cx, |preview, cx| preview.set_target(None, cx));
            (
                home::home_header(cx).into_any_element(),
                if has_device {
                    home::device_gallery(&mut self.card_focus, cx).into_any_element()
                } else {
                    match status.inventory {
                        InventoryHealth::Scanning => home::device_scanning_state(cx),
                        InventoryHealth::Unavailable => home::scanning_unavailable_state(cx),
                        InventoryHealth::Ready => home::device_empty_state(cx),
                    }
                    .into_any_element()
                },
            )
        };

        root.child(header_el)
            .child(content_el)
            .when(!granted, |this| this.child(status::attention_footer(cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
