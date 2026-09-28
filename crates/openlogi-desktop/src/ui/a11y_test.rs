//! A headless GPUI platform that builds the AccessKit tree, for tests.
//!
//! GPUI's `TestAppContext` runs on a private test platform whose windows drop
//! the accessibility-adapter callbacks, so the tree a screen reader reads is
//! never built there (gpui-pre 0.3.x). [`A11yTestApp`] runs the real app code on
//! the smallest platform that keeps them: its window reports the adapter as
//! active, forwards every `TreeUpdate` into `accesskit_consumer` — the crate the
//! Windows, macOS and Unix adapters themselves resolve names with — and can send
//! adapter actions (Focus, Click) the way a screen reader does.
//!
//! Tests assert on [`A11yNode`]s: the role, the name a screen reader speaks, the
//! description and state, and where focus is. Everything else (layout, pixels)
//! stays with the ordinary GPUI tests.

use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
};

use accesskit_consumer::{FilterResult, Node, Tree, TreeChangeHandler, common_filter};
use anyhow::Result;
use futures_channel::oneshot;
use gpui::{
    A11yCallbacks, Action, AnyWindowHandle, App, Application, ApplicationHandle, AtlasKey,
    AtlasTextureId, AtlasTextureKind, AtlasTile, BackgroundExecutor, Bounds, Capslock,
    ClipboardItem, CursorStyle, DevicePixels, DispatchEventResult, DummyKeyboardMapper,
    ForegroundExecutor, GpuSpecs, KeyUpEvent, Keymap, Keystroke, Menu, MenuItem, Modifiers,
    NoopTextSystem, OwnedMenu, PathPromptOptions, Pixels, Platform, PlatformAtlas, PlatformDisplay,
    PlatformInput, PlatformInputHandler, PlatformKeyboardLayout, PlatformKeyboardMapper,
    PlatformTextSystem, PlatformWindow, Point, PromptButton, PromptLevel, Render,
    RequestFrameOptions, Scene, Size, Task, TestDispatcher, ThermalState, TileId, Window,
    WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowHandle,
    WindowOptions, WindowParams, accesskit, px, size,
};

/// What a screen reader knows about one node of the accessibility tree.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct A11yNode {
    pub id: accesskit::NodeId,
    pub role: accesskit::Role,
    /// The accessible name, resolved the way the platform adapters do.
    pub name: Option<String>,
    pub description: Option<String>,
    pub value: Option<String>,
    pub selected: Option<bool>,
    pub toggled: Option<accesskit::Toggled>,
    pub expanded: Option<bool>,
    pub level: Option<usize>,
    pub focusable: bool,
    pub focused: bool,
    /// Depth below the window root, for readable dumps.
    pub depth: usize,
    /// The nearest ancestor the adapters expose.
    pub parent: Option<accesskit::NodeId>,
}

impl A11yNode {
    fn from_consumer(node: &Node<'_>, depth: usize, parent: Option<accesskit::NodeId>) -> Self {
        let name = if node.label_comes_from_value() {
            node.value()
        } else {
            node.label()
        };
        Self {
            id: node.locate().0,
            role: node.role(),
            name,
            description: node.description(),
            value: (!node.label_comes_from_value())
                .then(|| node.value())
                .flatten(),
            selected: node.is_selected(),
            toggled: node.toggled(),
            expanded: node.data().is_expanded(),
            level: node.level(),
            focusable: node.is_focusable(&common_filter),
            focused: node.is_focused(),
            depth,
            parent,
        }
    }

    /// Whether this node is something a user operates, as opposed to reads.
    pub fn is_interactive(&self) -> bool {
        use accesskit::Role;
        matches!(
            self.role,
            Role::Button
                | Role::CheckBox
                | Role::ComboBox
                | Role::Link
                | Role::MenuItem
                | Role::MenuItemRadio
                | Role::RadioButton
                | Role::Slider
                | Role::SpinButton
                | Role::Switch
                | Role::Tab
                | Role::TextInput
                | Role::MultilineTextInput
        )
    }
}

impl fmt::Debug for A11yNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:indent$}{:?}", "", self.role, indent = self.depth * 2)?;
        if let Some(name) = &self.name {
            write!(f, " {name:?}")?;
        }
        if let Some(description) = &self.description {
            write!(f, " desc={description:?}")?;
        }
        if let Some(value) = &self.value {
            write!(f, " value={value:?}")?;
        }
        if let Some(selected) = self.selected {
            write!(f, " selected={selected}")?;
        }
        if let Some(toggled) = self.toggled {
            write!(f, " toggled={toggled:?}")?;
        }
        if let Some(expanded) = self.expanded {
            write!(f, " expanded={expanded}")?;
        }
        if let Some(level) = self.level {
            write!(f, " level={level}")?;
        }
        if self.focused {
            write!(f, " [focused]")?;
        } else if self.focusable {
            write!(f, " [focusable]")?;
        }
        Ok(())
    }
}

/// The tree as a flat pre-order list, filtered the way the adapters filter it.
pub(crate) struct A11ySnapshot(pub Vec<A11yNode>);

impl A11ySnapshot {
    pub fn nodes(&self) -> &[A11yNode] {
        &self.0
    }

    pub fn focused(&self) -> Option<&A11yNode> {
        self.0.iter().find(|node| node.focused)
    }

    /// The first node with this role and name.
    pub fn find(&self, role: accesskit::Role, name: &str) -> Option<&A11yNode> {
        self.0
            .iter()
            .find(|node| node.role == role && node.name.as_deref() == Some(name))
    }

    /// Like [`Self::find`], panicking with the whole tree when it is absent.
    #[track_caller]
    pub fn expect(&self, role: accesskit::Role, name: &str) -> &A11yNode {
        self.find(role, name)
            .unwrap_or_else(|| panic!("no {role:?} named {name:?} in\n{self:?}"))
    }

    fn by_id(&self, id: accesskit::NodeId) -> Option<&A11yNode> {
        self.0.iter().find(|node| node.id == id)
    }

    /// Interactive nodes a screen reader cannot name.
    ///
    /// One shape is exempt, and only that exact shape: the stock slider
    /// nested directly in a named `ui::commit_slider::SliderControl`, which
    /// is the focusable, named slider. `gpui_base::Slider` has no label API
    /// (spec gap 4); drop the exemption when it gains one.
    pub fn unnamed_controls(&self) -> Vec<&A11yNode> {
        let unnamed = |node: &A11yNode| {
            node.name
                .as_deref()
                .is_none_or(|name| name.trim().is_empty())
        };
        self.0
            .iter()
            .filter(|node| node.is_interactive() && unnamed(node))
            .filter(|node| {
                let inside_named_slider = node.role == accesskit::Role::Slider
                    && !node.focusable
                    && node
                        .parent
                        .and_then(|parent| self.by_id(parent))
                        .is_some_and(|parent| {
                            parent.role == accesskit::Role::Slider && !unnamed(parent)
                        });
                !inside_named_slider
            })
            .collect()
    }
}

impl fmt::Debug for A11ySnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for node in &self.0 {
            writeln!(f, "{node:?}")?;
        }
        Ok(())
    }
}

/// A GPUI app on the accessibility-enabled headless platform.
pub(crate) struct A11yTestApp {
    app: ApplicationHandle,
    dispatcher: TestDispatcher,
    platform: Rc<HeadlessPlatform>,
    window: Cell<Option<AnyWindowHandle>>,
    /// Names are asserted in English; the locale is process-global.
    _locale: std::sync::MutexGuard<'static, ()>,
}

impl A11yTestApp {
    pub fn new() -> Self {
        let locale = crate::services::i18n::LOCALE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rust_i18n::set_locale("en");
        let dispatcher = TestDispatcher::new(0);
        let platform = HeadlessPlatform::new(&dispatcher);
        let app = Application::with_platform(platform.clone()).run_embedded(|_| {});
        Self {
            app,
            dispatcher,
            platform,
            window: Cell::new(None),
            _locale: locale,
        }
    }

    pub fn update<R>(&self, f: impl FnOnce(&mut App) -> R) -> R {
        self.app.update(f)
    }

    /// Open the one window under test and turn its accessibility adapter on.
    pub fn open_window<V: Render + 'static>(
        &self,
        build: impl FnOnce(&mut Window, &mut App) -> gpui::Entity<V>,
    ) -> WindowHandle<V> {
        let handle = self.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Point::default(),
                        size(px(1100.), px(760.)),
                    ))),
                    ..WindowOptions::default()
                },
                build,
            )
            .expect("the headless platform opens windows")
        });
        self.window.set(Some(handle.into()));
        self.platform.window().activate_adapter();
        self.settle();
        handle
    }

    fn window_handle(&self) -> AnyWindowHandle {
        self.window.get().expect("open_window first")
    }

    /// Run queued work and draw until nothing is left to do.
    pub fn settle(&self) {
        for _ in 0..4 {
            self.dispatcher.run_until_parked();
            self.platform.window().tick_frame();
            self.dispatcher.run_until_parked();
            let handle = self.window_handle();
            self.update(|cx| {
                handle
                    .update(cx, |_, window, cx| {
                        window.draw(cx).clear(cx);
                    })
                    .expect("window is open");
            });
        }
        self.dispatcher.run_until_parked();
    }

    /// Run `f` against the window, then settle.
    pub fn update_window<R>(&self, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
        let handle = self.window_handle();
        let result = self.update(|cx| {
            handle
                .update(cx, |_, window, cx| f(window, cx))
                .expect("window is open")
        });
        self.settle();
        result
    }

    /// Press and release space-separated keystrokes.
    ///
    /// Both halves are sent, as a real platform does: GPUI activates a focused
    /// clickable on key *up*, so `Window::dispatch_keystroke` (down only, what
    /// `simulate_keystrokes` uses) never reaches a button's click handler.
    pub fn press(&self, keystrokes: &str) {
        for keystroke in keystrokes.split(' ') {
            let keystroke = Keystroke::parse(keystroke).expect("valid keystroke");
            let up = keystroke.clone();
            self.update_window(|window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            });
            self.update_window(|window, cx| {
                window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke: up }), cx);
            });
        }
    }

    /// Send an adapter action (what a screen reader does on "activate").
    pub fn action(&self, node: &A11yNode, action: accesskit::Action) {
        self.platform
            .window()
            .send_action(accesskit::ActionRequest {
                action,
                target_tree: accesskit::TreeId::ROOT,
                target_node: node.id,
                data: None,
            });
        self.settle();
    }

    pub fn snapshot(&self) -> A11ySnapshot {
        self.platform.window().snapshot()
    }

    /// Names announced on each focus move since the last call, oldest first.
    pub fn take_focus_announcements(&self) -> Vec<A11yNode> {
        std::mem::take(&mut *self.platform.window().focus_moves.borrow_mut())
    }
}

impl Drop for A11yTestApp {
    fn drop(&mut self) {
        self.app.update(App::shutdown);
    }
}

struct HeadlessPlatform {
    background: BackgroundExecutor,
    foreground: ForegroundExecutor,
    window: RefCell<Option<Rc<WindowShared>>>,
}

impl HeadlessPlatform {
    fn new(dispatcher: &TestDispatcher) -> Rc<Self> {
        let dispatcher = Arc::new(dispatcher.clone());
        Rc::new(Self {
            background: BackgroundExecutor::new(dispatcher.clone()),
            foreground: ForegroundExecutor::new(dispatcher),
            window: RefCell::new(None),
        })
    }

    fn window(&self) -> Rc<WindowShared> {
        self.window.borrow().clone().expect("a window is open")
    }
}

type FrameCallback = Box<dyn FnMut(RequestFrameOptions)>;

/// State the platform and its window share: the adapter side of one window.
#[derive(Default)]
struct WindowShared {
    callbacks: RefCell<Option<A11yCallbacks>>,
    /// GPUI's frame callback: runs `Window::on_next_frame` work and draws, as
    /// a real platform's frame tick does.
    frame: RefCell<Option<FrameCallback>>,
    tree: RefCell<Option<Tree>>,
    focus_moves: RefCell<Vec<A11yNode>>,
    active: Cell<bool>,
}

impl WindowShared {
    /// One platform frame. `require_presentation` bypasses the inactive-window
    /// throttle, so every tick runs the pending next-frame callbacks.
    fn tick_frame(&self) {
        let Some(mut frame) = self.frame.borrow_mut().take() else {
            return;
        };
        frame(RequestFrameOptions {
            require_presentation: true,
            force_render: false,
        });
        *self.frame.borrow_mut() = Some(frame);
    }

    fn activate_adapter(&self) {
        let initial = {
            let callbacks = self.callbacks.borrow();
            let callbacks = callbacks.as_ref().expect("GPUI initialized the adapter");
            (callbacks.activation)().expect("activation returns the initial tree")
        };
        *self.tree.borrow_mut() = Some(Tree::new(initial, true));
        self.active.set(true);
    }

    fn send_action(&self, request: accesskit::ActionRequest) {
        let callbacks = self.callbacks.borrow();
        let callbacks = callbacks.as_ref().expect("GPUI initialized the adapter");
        (callbacks.action)(request);
    }

    fn apply(&self, update: accesskit::TreeUpdate) {
        let mut tree = self.tree.borrow_mut();
        let tree = tree.as_mut().expect("updates arrive only after activation");
        let mut recorder = FocusRecorder(Vec::new());
        tree.update_and_process_changes(update, &mut recorder);
        self.focus_moves.borrow_mut().extend(recorder.0);
    }

    fn snapshot(&self) -> A11ySnapshot {
        let tree = self.tree.borrow();
        let tree = tree.as_ref().expect("adapter is active");
        let mut nodes = Vec::new();
        collect(&tree.state().root(), 0, None, &mut nodes);
        A11ySnapshot(nodes)
    }
}

fn collect(
    node: &Node<'_>,
    depth: usize,
    parent: Option<accesskit::NodeId>,
    out: &mut Vec<A11yNode>,
) {
    let this = A11yNode::from_consumer(node, depth, parent);
    let id = this.id;
    out.push(this);
    for child in node.filtered_children(filter) {
        collect(&child, depth + 1, Some(id), out);
    }
}

fn filter(node: &Node<'_>) -> FilterResult {
    common_filter(node)
}

struct FocusRecorder(Vec<A11yNode>);

impl TreeChangeHandler for FocusRecorder {
    fn node_added(&mut self, _: &Node<'_>) {}
    fn node_updated(&mut self, _: &Node<'_>, _: &Node<'_>) {}
    fn focus_moved(&mut self, _: Option<&Node<'_>>, new: Option<&Node<'_>>) {
        if let Some(new) = new {
            self.0.push(A11yNode::from_consumer(new, 0, None));
        }
    }
    fn node_removed(&mut self, _: &Node<'_>) {}
}

struct HeadlessWindow {
    shared: Rc<WindowShared>,
    bounds: Bounds<Pixels>,
    input_handler: Option<PlatformInputHandler>,
    atlas: Arc<HeadlessAtlas>,
}

impl raw_window_handle::HasWindowHandle for HeadlessWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::NotSupported)
    }
}

impl raw_window_handle::HasDisplayHandle for HeadlessWindow {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::NotSupported)
    }
}

impl PlatformWindow for HeadlessWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }
    fn is_maximized(&self) -> bool {
        false
    }
    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Windowed(self.bounds)
    }
    fn content_size(&self) -> Size<Pixels> {
        self.bounds.size
    }
    fn resize(&mut self, size: Size<Pixels>) {
        self.bounds.size = size;
    }
    fn scale_factor(&self) -> f32 {
        1.0
    }
    fn appearance(&self) -> WindowAppearance {
        WindowAppearance::Light
    }
    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        None
    }
    fn mouse_position(&self) -> Point<Pixels> {
        Point::default()
    }
    fn modifiers(&self) -> Modifiers {
        Modifiers::default()
    }
    fn capslock(&self) -> Capslock {
        Capslock::default()
    }
    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.input_handler = Some(input_handler);
    }
    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.input_handler.take()
    }
    fn prompt(
        &self,
        _level: PromptLevel,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        None
    }
    fn activate(&self) {}
    fn is_active(&self) -> bool {
        true
    }
    fn is_hovered(&self) -> bool {
        false
    }
    fn background_appearance(&self) -> WindowBackgroundAppearance {
        WindowBackgroundAppearance::Opaque
    }
    fn set_title(&mut self, _title: &str) {}
    fn set_background_appearance(&self, _background_appearance: WindowBackgroundAppearance) {}
    fn minimize(&self) {}
    fn zoom(&self) {}
    fn toggle_fullscreen(&self) {}
    fn is_fullscreen(&self) -> bool {
        false
    }
    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        *self.shared.frame.borrow_mut() = Some(callback);
    }
    fn on_input(&self, _callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {}
    fn on_active_status_change(&self, _callback: Box<dyn FnMut(bool)>) {}
    fn on_hover_status_change(&self, _callback: Box<dyn FnMut(bool)>) {}
    fn on_resize(&self, _callback: Box<dyn FnMut(Size<Pixels>, f32)>) {}
    fn on_moved(&self, _callback: Box<dyn FnMut()>) {}
    fn on_should_close(&self, _callback: Box<dyn FnMut() -> bool>) {}
    fn on_hit_test_window_control(&self, _callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
    }
    fn on_close(&self, _callback: Box<dyn FnOnce()>) {}
    fn on_appearance_changed(&self, _callback: Box<dyn FnMut()>) {}
    fn draw(&self, _scene: &Scene) {}
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }
    fn is_subpixel_rendering_supported(&self) -> bool {
        false
    }
    #[cfg(target_os = "windows")]
    fn get_raw_handle(&self) -> windows::Win32::Foundation::HWND {
        windows::Win32::Foundation::HWND::default()
    }
    fn gpu_specs(&self) -> Option<GpuSpecs> {
        None
    }
    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}
    fn a11y_init(&self, callbacks: A11yCallbacks) {
        *self.shared.callbacks.borrow_mut() = Some(callbacks);
    }
    fn a11y_tree_update(&self, tree_update: accesskit::TreeUpdate) {
        if self.shared.active.get() {
            self.shared.apply(tree_update);
        }
    }
}

/// Hands out fake tiles; nothing is ever rasterized.
#[derive(Default)]
struct HeadlessAtlas(Mutex<AtlasState>);

#[derive(Default)]
struct AtlasState {
    next_id: u32,
    tiles: HashMap<AtlasKey, AtlasTile>,
}

impl HeadlessAtlas {
    fn lock(&self) -> std::sync::MutexGuard<'_, AtlasState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl PlatformAtlas for HeadlessAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        if let Some(tile) = self.lock().tiles.get(key) {
            return Ok(Some(*tile));
        }
        let Some((size, _)) = build()? else {
            return Ok(None);
        };
        let mut state = self.lock();
        state.next_id += 1;
        let tile = AtlasTile {
            texture_id: AtlasTextureId {
                index: state.next_id,
                kind: AtlasTextureKind::Monochrome,
            },
            tile_id: TileId(state.next_id),
            padding: 0,
            bounds: Bounds::new(Point::default(), size),
        };
        state.tiles.insert(key.clone(), tile);
        Ok(Some(tile))
    }
    fn remove(&self, key: &AtlasKey) {
        self.lock().tiles.remove(key);
    }
    fn contains(&self, key: &AtlasKey) -> bool {
        self.lock().tiles.contains_key(key)
    }
}

struct HeadlessKeyboardLayout;

impl PlatformKeyboardLayout for HeadlessKeyboardLayout {
    fn id(&self) -> &'static str {
        "openlogi.a11y-test"
    }
    fn name(&self) -> &'static str {
        "openlogi.a11y-test"
    }
}

impl Platform for HeadlessPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.background.clone()
    }
    fn foreground_executor(&self) -> ForegroundExecutor {
        self.foreground.clone()
    }
    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        Arc::new(NoopTextSystem)
    }
    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        on_finish_launching();
    }
    fn quit(&self) {}
    fn restart(&self, _binary_path: Option<PathBuf>, _arguments: Vec<OsString>) {}
    fn activate(&self, _ignoring_other_apps: bool) {}
    fn hide(&self) {}
    fn hide_other_apps(&self) {}
    fn unhide_other_apps(&self) {}
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        Vec::new()
    }
    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        None
    }
    fn active_window(&self) -> Option<AnyWindowHandle> {
        None
    }
    fn open_window(
        &self,
        _handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let shared = Rc::new(WindowShared::default());
        *self.window.borrow_mut() = Some(shared.clone());
        Ok(Box::new(HeadlessWindow {
            shared,
            bounds: options.bounds,
            input_handler: None,
            atlas: Arc::default(),
        }))
    }
    fn window_appearance(&self) -> WindowAppearance {
        WindowAppearance::Light
    }
    fn open_url(&self, _url: &str) {}
    fn on_open_urls(&self, _callback: Box<dyn FnMut(Vec<String>)>) {}
    fn register_url_scheme(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }
    fn prompt_for_paths(
        &self,
        _options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        oneshot::channel().1
    }
    fn prompt_for_new_path(
        &self,
        _directory: &Path,
        _suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        oneshot::channel().1
    }
    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }
    fn reveal_path(&self, _path: &Path) {}
    fn open_with_system(&self, _path: &Path) {}
    fn on_quit(&self, _callback: Box<dyn FnMut() -> bool>) {}
    fn on_reopen(&self, _callback: Box<dyn FnMut()>) {}
    fn on_system_wake(&self, _callback: Box<dyn FnMut()>) {}
    fn set_menus(&self, _menus: Vec<Menu>, _keymap: &Keymap) {}
    fn get_menus(&self) -> Option<Vec<OwnedMenu>> {
        None
    }
    fn set_dock_menu(&self, _menu: Vec<MenuItem>, _keymap: &Keymap) {}
    fn on_app_menu_action(&self, _callback: Box<dyn FnMut(&dyn Action)>) {}
    fn on_will_open_app_menu(&self, _callback: Box<dyn FnMut()>) {}
    fn on_validate_app_menu_command(&self, _callback: Box<dyn FnMut(&dyn Action) -> bool>) {}
    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }
    fn on_thermal_state_change(&self, _callback: Box<dyn FnMut()>) {}
    fn app_path(&self) -> Result<PathBuf> {
        anyhow::bail!("headless test platform has no app path")
    }
    fn path_for_auxiliary_executable(&self, _name: &str) -> Result<PathBuf> {
        anyhow::bail!("headless test platform has no auxiliary executables")
    }
    fn set_cursor_style(&self, _style: CursorStyle) {}
    fn hide_cursor_until_mouse_moves(&self) {}
    fn is_cursor_visible(&self) -> bool {
        true
    }
    fn should_auto_hide_scrollbars(&self) -> bool {
        false
    }
    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        None
    }
    fn write_to_clipboard(&self, _item: ClipboardItem) {}
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn read_from_primary(&self) -> Option<ClipboardItem> {
        None
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn write_to_primary(&self, _item: ClipboardItem) {}
    #[cfg(target_os = "macos")]
    fn read_from_find_pasteboard(&self) -> Option<ClipboardItem> {
        None
    }
    #[cfg(target_os = "macos")]
    fn write_to_find_pasteboard(&self, _item: ClipboardItem) {}
    fn write_credentials(&self, _url: &str, _username: &str, _password: &[u8]) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }
    fn read_credentials(&self, _url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Ok(None))
    }
    fn delete_credentials(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }
    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(HeadlessKeyboardLayout)
    }
    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }
    fn on_keyboard_layout_change(&self, _callback: Box<dyn FnMut()>) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, Context, IntoElement, ParentElement as _, Styled as _, div};
    use gpui_base::Button as BaseButton;

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(BaseButton::new("named").accessibility_label("Save"))
                .child(BaseButton::new("unnamed"))
        }
    }

    #[test]
    fn builds_the_tree_a_screen_reader_reads() {
        let app = A11yTestApp::new();
        app.open_window(|_, cx| cx.new(|_| Probe));
        let tree = app.snapshot();
        eprintln!("{tree:?}");
        tree.expect(accesskit::Role::Button, "Save");
        assert_eq!(tree.unnamed_controls().len(), 1, "{tree:?}");
    }
}
