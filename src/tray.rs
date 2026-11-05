//! Linux StatusNotifierItem tray on the existing GLib main loop.
//!
//! The module exports both the KDE StatusNotifierItem and Canonical DBusMenu
//! interfaces. Watcher discovery and registration use GIO asynchronous calls;
//! no worker thread or polling source is created.

use crate::ui::Ui;
use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::{
        OnceLock,
        atomic::{AtomicU32, Ordering},
    },
};

const ITEM_ID: &str = "io.github.bilimusic.Linux";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_INTERFACE: &str = "org.kde.StatusNotifierWatcher";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";

const ROOT_ID: i32 = 0;
const SHOW_ID: i32 = 1;
const PLAY_PAUSE_ID: i32 = 2;
const PREVIOUS_ID: i32 = 3;
const NEXT_ID: i32 = 4;
const SEPARATOR_ID: i32 = 5;
const QUIT_ID: i32 = 6;
const MENU_IDS: [i32; 6] = [
    SHOW_ID,
    PLAY_PAUSE_ID,
    PREVIOUS_ID,
    NEXT_ID,
    SEPARATOR_ID,
    QUIT_ID,
];

type Pixmap = (i32, i32, Vec<u8>);
type Tooltip = (String, Vec<Pixmap>, String, String);

const XML: &str = r#"
<node>
  <interface name="org.kde.StatusNotifierItem">
    <method name="ContextMenu">
      <arg name="x" type="i" direction="in"/>
      <arg name="y" type="i" direction="in"/>
    </method>
    <method name="Activate">
      <arg name="x" type="i" direction="in"/>
      <arg name="y" type="i" direction="in"/>
    </method>
    <method name="SecondaryActivate">
      <arg name="x" type="i" direction="in"/>
      <arg name="y" type="i" direction="in"/>
    </method>
    <method name="Scroll">
      <arg name="delta" type="i" direction="in"/>
      <arg name="orientation" type="s" direction="in"/>
    </method>

    <signal name="NewTitle"/>
    <signal name="NewIcon"/>
    <signal name="NewAttentionIcon"/>
    <signal name="NewOverlayIcon"/>
    <signal name="NewToolTip"/>
    <signal name="NewStatus">
      <arg name="status" type="s"/>
    </signal>

    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="WindowId" type="i" access="read"/>
    <property name="IconName" type="s" access="read"/>
    <property name="IconPixmap" type="a(iiay)" access="read"/>
    <property name="OverlayIconName" type="s" access="read"/>
    <property name="OverlayIconPixmap" type="a(iiay)" access="read"/>
    <property name="AttentionIconName" type="s" access="read"/>
    <property name="AttentionIconPixmap" type="a(iiay)" access="read"/>
    <property name="AttentionMovieName" type="s" access="read"/>
    <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
    <property name="ItemIsMenu" type="b" access="read"/>
    <property name="Menu" type="o" access="read"/>
  </interface>

  <interface name="com.canonical.dbusmenu">
    <method name="GetLayout">
      <arg name="parentId" type="i" direction="in"/>
      <arg name="recursionDepth" type="i" direction="in"/>
      <arg name="propertyNames" type="as" direction="in"/>
      <arg name="revision" type="u" direction="out"/>
      <arg name="layout" type="(ia{sv}av)" direction="out"/>
    </method>
    <method name="GetGroupProperties">
      <arg name="ids" type="ai" direction="in"/>
      <arg name="propertyNames" type="as" direction="in"/>
      <arg name="properties" type="a(ia{sv})" direction="out"/>
    </method>
    <method name="GetProperty">
      <arg name="id" type="i" direction="in"/>
      <arg name="name" type="s" direction="in"/>
      <arg name="value" type="v" direction="out"/>
    </method>
    <method name="Event">
      <arg name="id" type="i" direction="in"/>
      <arg name="eventId" type="s" direction="in"/>
      <arg name="data" type="v" direction="in"/>
      <arg name="timestamp" type="u" direction="in"/>
    </method>
    <method name="EventGroup">
      <arg name="events" type="a(isvu)" direction="in"/>
      <arg name="idErrors" type="ai" direction="out"/>
    </method>
    <method name="AboutToShow">
      <arg name="id" type="i" direction="in"/>
      <arg name="needUpdate" type="b" direction="out"/>
    </method>
    <method name="AboutToShowGroup">
      <arg name="ids" type="ai" direction="in"/>
      <arg name="updatesNeeded" type="ai" direction="out"/>
      <arg name="idErrors" type="ai" direction="out"/>
    </method>

    <signal name="ItemsPropertiesUpdated">
      <arg name="updatedProps" type="a(ia{sv})"/>
      <arg name="removedProps" type="a(ias)"/>
    </signal>
    <signal name="LayoutUpdated">
      <arg name="revision" type="u"/>
      <arg name="parent" type="i"/>
    </signal>
    <signal name="ItemActivationRequested">
      <arg name="id" type="i"/>
      <arg name="timestamp" type="u"/>
    </signal>

    <property name="Version" type="u" access="read"/>
    <property name="TextDirection" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconThemePath" type="as" access="read"/>
  </interface>
</node>
"#;

static NEXT_INSTANCE: AtomicU32 = AtomicU32::new(1);
static ICON_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

/// A live StatusNotifierItem owned by the application's GLib main context.
pub struct Tray {
    connection: gio::DBusConnection,
    registrations: Vec<gio::RegistrationId>,
    owner: Option<gio::OwnerId>,
    watcher_unwatch: Option<Box<dyn FnOnce()>>,
    signal_subscriptions: Vec<gio::SignalSubscription>,
    state: Rc<TrayState>,
}

struct TrayState {
    alive: Cell<bool>,
    service_name: String,
    service_owned: Cell<bool>,
    watcher_owner: RefCell<Option<String>>,
    host_registered: Cell<bool>,
    registered: Cell<bool>,
    registration_inflight: Cell<bool>,
    revision: Cell<u32>,
    ui: std::rc::Weak<Ui>,
}

impl Tray {
    /// Export the tray interfaces and watch for a StatusNotifier host.
    pub fn new(ui: &Rc<Ui>) -> Result<Self, String> {
        let connection = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
            .map_err(|error| format!("无法连接会话 D-Bus：{error}"))?;
        let node = gio::DBusNodeInfo::for_xml(XML)
            .map_err(|error| format!("托盘 D-Bus 接口无效：{error}"))?;
        let item_info = node
            .lookup_interface(ITEM_INTERFACE)
            .ok_or_else(|| "缺少 StatusNotifierItem 接口".to_string())?;
        let menu_info = node
            .lookup_interface(MENU_INTERFACE)
            .ok_or_else(|| "缺少 DBusMenu 接口".to_string())?;

        let instance = NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed);
        let state = Rc::new(TrayState {
            alive: Cell::new(true),
            service_name: format!(
                "org.kde.StatusNotifierItem-{}-{instance}",
                std::process::id()
            ),
            service_owned: Cell::new(false),
            watcher_owner: RefCell::new(None),
            host_registered: Cell::new(false),
            registered: Cell::new(false),
            registration_inflight: Cell::new(false),
            revision: Cell::new(1),
            ui: Rc::downgrade(ui),
        });

        let item_method_ui = Rc::downgrade(ui);
        let item_property_ui = Rc::downgrade(ui);
        let item_registration = connection
            .register_object(ITEM_PATH, &item_info)
            .method_call(
                move |_connection, _sender, _path, _interface, method, _params, invocation| {
                    let Some(ui) = item_method_ui.upgrade() else {
                        invocation
                            .return_dbus_error("org.kde.StatusNotifierItem.Closed", "Buisc 已关闭");
                        return;
                    };
                    match method {
                        "Activate" => ui.show_window(),
                        "SecondaryActivate" => ui.toggle(),
                        "ContextMenu" | "Scroll" => {}
                        _ => {
                            invocation.return_dbus_error(
                                "org.freedesktop.DBus.Error.UnknownMethod",
                                "未知托盘方法",
                            );
                            return;
                        }
                    }
                    invocation.return_value(Some(&().to_variant()));
                },
            )
            .property(move |_connection, _sender, _path, _interface, property| {
                let ui = item_property_ui.upgrade();
                item_property(ui.as_deref(), property)
            })
            .build()
            .map_err(|error| format!("无法导出 StatusNotifierItem：{error}"))?;

        let menu_state = Rc::downgrade(&state);
        let menu_ui = Rc::downgrade(ui);
        let menu_registration = match connection
            .register_object(MENU_PATH, &menu_info)
            .method_call(
                move |_connection, _sender, _path, _interface, method, params, invocation| {
                    menu_method(&menu_state, &menu_ui, method, params, invocation);
                },
            )
            .property(|_connection, _sender, _path, _interface, property| {
                menu_interface_property(property)
            })
            .build()
        {
            Ok(registration) => registration,
            Err(error) => {
                let _ = connection.unregister_object(item_registration);
                state.alive.set(false);
                return Err(format!("无法导出 DBusMenu：{error}"));
            }
        };

        let mut signal_subscriptions = Vec::new();
        for member in [
            "StatusNotifierHostRegistered",
            "StatusNotifierHostUnregistered",
        ] {
            let signal_state = Rc::downgrade(&state);
            signal_subscriptions.push(connection.subscribe_to_signal(
                Some(WATCHER_NAME),
                Some(WATCHER_INTERFACE),
                Some(member),
                Some(WATCHER_PATH),
                None,
                gio::DBusSignalFlags::NONE,
                move |signal| {
                    let Some(state) = signal_state.upgrade() else {
                        return;
                    };
                    if !is_current_watcher(&state, signal.sender_name) {
                        return;
                    }
                    let host_registered = member == "StatusNotifierHostRegistered";
                    set_host_state(signal.connection, &state, host_registered);
                },
            ));
        }

        let properties_state = Rc::downgrade(&state);
        signal_subscriptions.push(connection.subscribe_to_signal(
            Some(WATCHER_NAME),
            Some(PROPERTIES_INTERFACE),
            Some("PropertiesChanged"),
            Some(WATCHER_PATH),
            Some(WATCHER_INTERFACE),
            gio::DBusSignalFlags::NONE,
            move |signal| {
                let Some(state) = properties_state.upgrade() else {
                    return;
                };
                if !is_current_watcher(&state, signal.sender_name) {
                    return;
                }
                let Some((_interface, changed, _invalidated)) =
                    signal
                        .parameters
                        .get::<(String, HashMap<String, glib::Variant>, Vec<String>)>()
                else {
                    return;
                };
                if let Some(host_registered) = changed
                    .get("IsStatusNotifierHostRegistered")
                    .and_then(glib::Variant::get::<bool>)
                {
                    set_host_state(signal.connection, &state, host_registered);
                }
            },
        ));

        let owner_acquired_state = Rc::downgrade(&state);
        let owner_lost_state = owner_acquired_state.clone();
        let owner = gio::bus_own_name_on_connection(
            &connection,
            &state.service_name,
            gio::BusNameOwnerFlags::NONE,
            move |connection, _name| {
                let Some(state) = owner_acquired_state.upgrade() else {
                    return;
                };
                state.service_owned.set(true);
                try_register_item(&connection, &state);
            },
            move |_connection, _name| {
                if let Some(state) = owner_lost_state.upgrade() {
                    state.service_owned.set(false);
                    state.registered.set(false);
                    state.registration_inflight.set(false);
                }
            },
        );

        let watcher_appeared_state = Rc::downgrade(&state);
        let watcher_vanished_state = watcher_appeared_state.clone();
        let watcher = gio::bus_watch_name_on_connection(
            &connection,
            WATCHER_NAME,
            gio::BusNameWatcherFlags::NONE,
            move |connection, _name, owner| {
                let Some(state) = watcher_appeared_state.upgrade() else {
                    return;
                };
                *state.watcher_owner.borrow_mut() = Some(owner.to_string());
                state.registered.set(false);
                state.registration_inflight.set(false);
                state.host_registered.set(false);
                check_watcher_host(&connection, &state, owner);
            },
            move |_connection, _name| {
                let Some(state) = watcher_vanished_state.upgrade() else {
                    return;
                };
                *state.watcher_owner.borrow_mut() = None;
                state.host_registered.set(false);
                state.registered.set(false);
                state.registration_inflight.set(false);
                restore_window(&state);
            },
        );
        let watcher_unwatch = Box::new(move || gio::bus_unwatch_name(watcher));

        Ok(Self {
            connection,
            registrations: vec![item_registration, menu_registration],
            owner: Some(owner),
            watcher_unwatch: Some(watcher_unwatch),
            signal_subscriptions,
            state,
        })
    }

    /// True only after a live watcher reports a host and accepted our item.
    pub fn is_available(&self) -> bool {
        let watcher_owner = self.state.watcher_owner.borrow();
        let Some(watcher_owner) = watcher_owner.as_ref() else {
            return false;
        };
        !watcher_owner.is_empty()
            && self.state.service_owned.get()
            && self.state.host_registered.get()
            && self.state.registered.get()
    }

    /// Refresh title, tooltip, icon and DBusMenu revision after a player change.
    pub fn changed(&self, ui: &Ui) {
        let revision = self.state.revision.get().wrapping_add(1).max(1);
        self.state.revision.set(revision);

        let _ = self
            .connection
            .emit_signal(None, ITEM_PATH, ITEM_INTERFACE, "NewTitle", None);
        let _ = self
            .connection
            .emit_signal(None, ITEM_PATH, ITEM_INTERFACE, "NewIcon", None);
        let _ = self
            .connection
            .emit_signal(None, ITEM_PATH, ITEM_INTERFACE, "NewToolTip", None);
        let _ = self.connection.emit_signal(
            None,
            ITEM_PATH,
            ITEM_INTERFACE,
            "NewStatus",
            Some(&("Active",).to_variant()),
        );

        let updated = MENU_IDS
            .into_iter()
            .map(|id| (id, menu_properties(id, &[])))
            .collect::<Vec<_>>();
        let removed = Vec::<(i32, Vec<String>)>::new();
        let _ = self.connection.emit_signal(
            None,
            MENU_PATH,
            MENU_INTERFACE,
            "ItemsPropertiesUpdated",
            Some(&(updated, removed).to_variant()),
        );
        let _ = self.connection.emit_signal(
            None,
            MENU_PATH,
            MENU_INTERFACE,
            "LayoutUpdated",
            Some(&(revision, ROOT_ID).to_variant()),
        );

        let _ = ui;
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.state.alive.set(false);
        self.state.registered.set(false);
        self.state.registration_inflight.set(false);

        if let Some(unwatch) = self.watcher_unwatch.take() {
            unwatch();
        }
        if let Some(owner) = self.owner.take() {
            gio::bus_unown_name(owner);
        }
        for registration in self.registrations.drain(..) {
            let _ = self.connection.unregister_object(registration);
        }
        self.signal_subscriptions.clear();
    }
}

fn item_property(ui: Option<&Ui>, name: &str) -> glib::Variant {
    match name {
        "Category" => "ApplicationStatus".to_variant(),
        "Id" => ITEM_ID.to_variant(),
        "Title" => item_title(ui).to_variant(),
        "Status" => "Active".to_variant(),
        "WindowId" => 0i32.to_variant(),
        "IconName" => ITEM_ID.to_variant(),
        "IconPixmap" => icon_pixmap().to_variant(),
        "OverlayIconName" | "AttentionIconName" | "AttentionMovieName" => {
            String::new().to_variant()
        }
        "OverlayIconPixmap" | "AttentionIconPixmap" => Vec::<Pixmap>::new().to_variant(),
        "ToolTip" => tooltip(ui).to_variant(),
        "ItemIsMenu" => false.to_variant(),
        "Menu" => glib::variant::ObjectPath::try_from(MENU_PATH)
            .expect("菜单对象路径是常量")
            .to_variant(),
        _ => String::new().to_variant(),
    }
}

fn item_title(ui: Option<&Ui>) -> String {
    ui.and_then(Ui::current_track)
        .map_or_else(|| "Buisc".to_string(), |track| track.title)
}

fn tooltip(ui: Option<&Ui>) -> Tooltip {
    let Some(ui) = ui else {
        return (
            ITEM_ID.to_string(),
            icon_pixmap(),
            "Buisc".to_string(),
            "Bilibili 音乐".to_string(),
        );
    };
    let track = ui.current_track();
    let title = track
        .as_ref()
        .map_or_else(|| "Buisc".to_string(), |track| track.title.clone());
    let subtitle = match track {
        Some(track) => {
            let state = if ui.playing() {
                "正在播放"
            } else {
                "已暂停"
            };
            format!("{state} · {}", track.artist)
        }
        None => "Bilibili 音乐".to_string(),
    };
    (ITEM_ID.to_string(), icon_pixmap(), title, subtitle)
}

fn menu_method(
    state: &std::rc::Weak<TrayState>,
    ui: &std::rc::Weak<Ui>,
    method: &str,
    params: glib::Variant,
    invocation: gio::DBusMethodInvocation,
) {
    match method {
        "GetLayout" => {
            let Some((parent, depth, property_names)) = params.get::<(i32, i32, Vec<String>)>()
            else {
                invalid_arguments(invocation);
                return;
            };
            let revision = state.upgrade().map_or(1, |state| state.revision.get());
            match layout_reply(revision, parent, depth, &property_names) {
                Some(reply) => invocation.return_value(Some(&reply)),
                None => invocation
                    .return_dbus_error("com.canonical.dbusmenu.Error.InvalidId", "菜单项不存在"),
            }
        }
        "GetGroupProperties" => {
            let Some((ids, property_names)) = params.get::<(Vec<i32>, Vec<String>)>() else {
                invalid_arguments(invocation);
                return;
            };
            let properties = group_properties(&ids, &property_names);
            invocation.return_value(Some(&(properties,).to_variant()));
        }
        "GetProperty" => {
            let Some((id, name)) = params.get::<(i32, String)>() else {
                invalid_arguments(invocation);
                return;
            };
            let Some(value) = menu_property(id, &name) else {
                invocation.return_dbus_error(
                    "com.canonical.dbusmenu.Error.UnknownProperty",
                    "菜单属性不存在",
                );
                return;
            };
            invocation.return_value(Some(&(value,).to_variant()));
        }
        "Event" => {
            let Some((id, event, _data, _timestamp)) =
                params.get::<(i32, String, glib::Variant, u32)>()
            else {
                invalid_arguments(invocation);
                return;
            };
            if event == "clicked" {
                activate_menu_item(ui, id);
            }
            invocation.return_value(Some(&().to_variant()));
        }
        "EventGroup" => {
            let Some((events,)) = params.get::<(Vec<(i32, String, glib::Variant, u32)>,)>() else {
                invalid_arguments(invocation);
                return;
            };
            let mut errors = Vec::new();
            for (id, event, _data, _timestamp) in events {
                if !valid_menu_id(id) {
                    errors.push(id);
                } else if event == "clicked" {
                    activate_menu_item(ui, id);
                }
            }
            invocation.return_value(Some(&(errors,).to_variant()));
        }
        "AboutToShow" => {
            let Some((id,)) = params.get::<(i32,)>() else {
                invalid_arguments(invocation);
                return;
            };
            if !valid_menu_id(id) {
                invocation
                    .return_dbus_error("com.canonical.dbusmenu.Error.InvalidId", "菜单项不存在");
                return;
            }
            invocation.return_value(Some(&(false,).to_variant()));
        }
        "AboutToShowGroup" => {
            let Some((ids,)) = params.get::<(Vec<i32>,)>() else {
                invalid_arguments(invocation);
                return;
            };
            let errors = ids
                .into_iter()
                .filter(|id| !valid_menu_id(*id))
                .collect::<Vec<_>>();
            let updates = Vec::<i32>::new();
            invocation.return_value(Some(&(updates, errors).to_variant()));
        }
        _ => {
            invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", "未知菜单方法")
        }
    }
}

fn activate_menu_item(ui: &std::rc::Weak<Ui>, id: i32) {
    let Some(ui) = ui.upgrade() else {
        return;
    };
    match id {
        SHOW_ID => ui.show_window(),
        PLAY_PAUSE_ID => ui.toggle(),
        PREVIOUS_ID => ui.previous(),
        NEXT_ID => ui.next(false),
        QUIT_ID => ui.quit(),
        SEPARATOR_ID => {}
        _ => {}
    }
}

fn invalid_arguments(invocation: gio::DBusMethodInvocation) {
    invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "菜单方法参数无效");
}

fn menu_interface_property(name: &str) -> glib::Variant {
    match name {
        "Version" => 3u32.to_variant(),
        "TextDirection" => "ltr".to_variant(),
        "Status" => "normal".to_variant(),
        "IconThemePath" => Vec::<String>::new().to_variant(),
        _ => String::new().to_variant(),
    }
}

fn valid_menu_id(id: i32) -> bool {
    id == ROOT_ID || MENU_IDS.contains(&id)
}

fn menu_label(id: i32) -> Option<&'static str> {
    match id {
        ROOT_ID => Some("Buisc"),
        SHOW_ID => Some("显示窗口"),
        PLAY_PAUSE_ID => Some("播放 / 暂停"),
        PREVIOUS_ID => Some("上一首"),
        NEXT_ID => Some("下一首"),
        SEPARATOR_ID => Some(""),
        QUIT_ID => Some("退出 Buisc"),
        _ => None,
    }
}

fn menu_property(id: i32, name: &str) -> Option<glib::Variant> {
    let value = match name {
        "label" => menu_label(id)?.to_variant(),
        "enabled" => true.to_variant(),
        "visible" => true.to_variant(),
        "type" => match id {
            ROOT_ID => "standard".to_variant(),
            SEPARATOR_ID => "separator".to_variant(),
            _ if valid_menu_id(id) => "standard".to_variant(),
            _ => return None,
        },
        "children-display" if id == ROOT_ID => "standard".to_variant(),
        _ if valid_menu_id(id) => return None,
        _ => return None,
    };
    Some(value)
}

fn menu_properties(id: i32, requested: &[String]) -> HashMap<String, glib::Variant> {
    let mut properties = HashMap::new();
    if !valid_menu_id(id) {
        return properties;
    }
    for name in ["label", "enabled", "visible", "type", "children-display"] {
        if (requested.is_empty() || requested.iter().any(|requested| requested == name))
            && let Some(value) = menu_property(id, name)
        {
            properties.insert(name.to_string(), value);
        }
    }
    properties
}

fn group_properties(
    ids: &[i32],
    requested: &[String],
) -> Vec<(i32, HashMap<String, glib::Variant>)> {
    let ids: Vec<i32> = if ids.is_empty() {
        std::iter::once(ROOT_ID).chain(MENU_IDS).collect()
    } else {
        ids.to_vec()
    };
    ids.into_iter()
        .filter(|id| valid_menu_id(*id))
        .map(|id| (id, menu_properties(id, requested)))
        .collect()
}

fn child_ids(id: i32) -> &'static [i32] {
    if id == ROOT_ID { &MENU_IDS } else { &[] }
}

fn layout_node(
    id: i32,
    depth: i32,
    requested: &[String],
) -> Option<(i32, HashMap<String, glib::Variant>, Vec<glib::Variant>)> {
    if !valid_menu_id(id) {
        return None;
    }
    let children = if depth == 0 {
        Vec::new()
    } else {
        let child_depth = if depth < 0 { -1 } else { depth - 1 };
        child_ids(id)
            .iter()
            .filter_map(|child| {
                layout_node(*child, child_depth, requested).map(|layout| layout.to_variant())
            })
            .collect()
    };
    Some((id, menu_properties(id, requested), children))
}

fn layout_reply(
    revision: u32,
    parent: i32,
    depth: i32,
    requested: &[String],
) -> Option<glib::Variant> {
    let layout = layout_node(parent, depth, requested)?;
    Some((revision, layout).to_variant())
}

fn is_current_watcher(state: &TrayState, sender: &str) -> bool {
    state
        .watcher_owner
        .borrow()
        .as_deref()
        .is_some_and(|owner| owner == sender)
}

fn set_host_state(connection: &gio::DBusConnection, state: &Rc<TrayState>, host_registered: bool) {
    if !state.alive.get() {
        return;
    }
    let was_registered = state.host_registered.replace(host_registered);
    if host_registered {
        try_register_item(connection, state);
    } else {
        state.registered.set(false);
        if was_registered {
            restore_window(state);
        }
    }
}

fn check_watcher_host(
    connection: &gio::DBusConnection,
    state: &Rc<TrayState>,
    expected_owner: &str,
) {
    let weak_state = Rc::downgrade(state);
    let expected_owner = expected_owner.to_string();
    let call_connection = connection.clone();
    connection.call(
        Some(WATCHER_NAME),
        WATCHER_PATH,
        PROPERTIES_INTERFACE,
        "Get",
        Some(&(WATCHER_INTERFACE, "IsStatusNotifierHostRegistered").to_variant()),
        Some(glib::VariantTy::new("(v)").expect("属性回复类型是常量")),
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        move |result| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            if !state.alive.get()
                || state.watcher_owner.borrow().as_deref() != Some(expected_owner.as_str())
            {
                return;
            }
            let host_registered = result
                .ok()
                .and_then(|reply| reply.get::<(glib::Variant,)>())
                .and_then(|(value,)| value.get::<bool>())
                .unwrap_or(false);
            set_host_state(&call_connection, &state, host_registered);
        },
    );
}

fn try_register_item(connection: &gio::DBusConnection, state: &Rc<TrayState>) {
    let Some(expected_owner) = state.watcher_owner.borrow().clone() else {
        return;
    };
    if !state.alive.get()
        || !state.service_owned.get()
        || !state.host_registered.get()
        || state.registered.get()
        || state.registration_inflight.replace(true)
    {
        return;
    }

    let weak_state = Rc::downgrade(state);
    // Register our owned name rather than the shared connection's unique name:
    // releasing it removes the tray even while GTK/MPRIS keep the bus alive.
    let service_name = state.service_name.clone();
    connection.call(
        Some(WATCHER_NAME),
        WATCHER_PATH,
        WATCHER_INTERFACE,
        "RegisterStatusNotifierItem",
        Some(&(service_name.as_str(),).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        move |result| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            if !state.alive.get()
                || state.watcher_owner.borrow().as_deref() != Some(expected_owner.as_str())
            {
                return;
            }
            state.registration_inflight.set(false);
            let still_current = state.alive.get()
                && state.service_owned.get()
                && state.host_registered.get()
                && state.watcher_owner.borrow().as_deref() == Some(expected_owner.as_str());
            if result.is_ok() && still_current {
                state.registered.set(true);
            } else {
                state.registered.set(false);
            }
        },
    );
}

fn restore_window(state: &TrayState) {
    if let Some(ui) = state.ui.upgrade()
        && !ui.window.is_visible()
    {
        ui.show_window();
    }
}

fn icon_pixmap() -> Vec<Pixmap> {
    const SIZE: i32 = 32;
    let bytes = ICON_BYTES.get_or_init(|| {
        let mut bytes = vec![0; (SIZE * SIZE * 4) as usize];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let dx = x - (SIZE - 1) / 2;
                let dy = y - (SIZE - 1) / 2;
                let inside = dx * dx + dy * dy <= 15 * 15;
                let note_head = {
                    let hx = x - 14;
                    let hy = y - 23;
                    hx * hx + 2 * hy * hy <= 18
                };
                let stem = (18..=21).contains(&x) && (7..=23).contains(&y);
                let flag = (18..=26).contains(&x) && (7..=13).contains(&y) && y <= 7 + (x - 18) / 2;
                let color = if note_head || stem || flag {
                    [255, 255, 255, 255]
                } else if inside {
                    [255, 251, 114, 153]
                } else {
                    [0, 0, 0, 0]
                };
                let offset = ((y * SIZE + x) * 4) as usize;
                bytes[offset..offset + 4].copy_from_slice(&color);
            }
        }
        bytes
    });
    vec![(SIZE, SIZE, bytes.clone())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_xml_exports_both_interfaces() {
        let node = gio::DBusNodeInfo::for_xml(XML).expect("valid tray XML");
        assert!(node.lookup_interface(ITEM_INTERFACE).is_some());
        assert!(node.lookup_interface(MENU_INTERFACE).is_some());
    }

    #[test]
    fn dbus_menu_layout_has_standard_signature() {
        let reply = layout_reply(7, ROOT_ID, -1, &[]).expect("root layout");
        assert_eq!(reply.type_().as_str(), "(u(ia{sv}av))");
        let (revision, (_, properties, children)) = reply
            .get::<(
                u32,
                (i32, HashMap<String, glib::Variant>, Vec<glib::Variant>),
            )>()
            .expect("layout fields");
        assert_eq!(revision, 7);
        assert_eq!(children.len(), MENU_IDS.len());
        assert_eq!(
            properties
                .get("label")
                .and_then(glib::Variant::get::<String>),
            Some("Buisc".to_string())
        );
    }

    #[test]
    fn dbus_menu_reply_signatures_match_methods() {
        let properties = group_properties(&[], &[]);
        assert_eq!(
            (properties.clone(),).to_variant().type_().as_str(),
            "(a(ia{sv}))"
        );

        let value = menu_property(SHOW_ID, "label").expect("show label");
        let reply = (value,).to_variant();
        assert_eq!(reply.type_().as_str(), "(v)");
        let (value,) = reply
            .get::<(glib::Variant,)>()
            .expect("single variant reply");
        assert_eq!(value.type_().as_str(), "s");
        assert_eq!(value.get::<String>().as_deref(), Some("显示窗口"));

        let errors = Vec::<i32>::new();
        assert_eq!((errors.clone(),).to_variant().type_().as_str(), "(ai)");
        assert_eq!(
            (errors.clone(), errors).to_variant().type_().as_str(),
            "(aiai)"
        );
    }

    #[test]
    fn icon_pixmap_is_static_argb32() {
        let pixmap = icon_pixmap();
        assert_eq!(pixmap.len(), 1);
        let (width, height, bytes) = &pixmap[0];
        assert_eq!((*width, *height), (32, 32));
        assert_eq!(bytes.len(), 32 * 32 * 4);
        assert_eq!(pixmap.to_variant().type_().as_str(), "a(iiay)");
    }
}
