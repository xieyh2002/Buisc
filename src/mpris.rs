//! Event-driven MPRIS on the existing GLib main loop; no extra runtime or thread.
use crate::ui::Ui;
use gtk::{gio, glib, prelude::*};
use std::{collections::HashMap, rc::Rc};
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const XML: &str = r#"<node>
<interface name="org.mpris.MediaPlayer2">
 <method name="Raise"/><method name="Quit"/>
 <property name="CanQuit" type="b" access="read"/><property name="CanRaise" type="b" access="read"/>
 <property name="HasTrackList" type="b" access="read"/><property name="Identity" type="s" access="read"/>
 <property name="DesktopEntry" type="s" access="read"/>
 <property name="SupportedUriSchemes" type="as" access="read"/><property name="SupportedMimeTypes" type="as" access="read"/>
</interface>
<interface name="org.mpris.MediaPlayer2.Player">
 <method name="Next"/><method name="Previous"/><method name="Pause"/><method name="PlayPause"/><method name="Stop"/><method name="Play"/>
 <method name="Seek"><arg name="Offset" direction="in" type="x"/></method>
 <method name="SetPosition"><arg name="TrackId" direction="in" type="o"/><arg name="Position" direction="in" type="x"/></method>
 <signal name="Seeked"><arg name="Position" type="x"/></signal>
 <property name="PlaybackStatus" type="s" access="read"/>
 <property name="Rate" type="d" access="readwrite"/><property name="MinimumRate" type="d" access="read"/><property name="MaximumRate" type="d" access="read"/>
 <property name="Metadata" type="a{sv}" access="read"/><property name="Volume" type="d" access="readwrite"/>
 <property name="Position" type="x" access="read"/>
 <property name="CanGoNext" type="b" access="read"/><property name="CanGoPrevious" type="b" access="read"/>
 <property name="CanPlay" type="b" access="read"/><property name="CanPause" type="b" access="read"/>
 <property name="CanSeek" type="b" access="read"/><property name="CanControl" type="b" access="read"/>
</interface></node>"#;
pub struct Mpris {
    connection: gio::DBusConnection,
    registrations: Vec<gio::RegistrationId>,
    owner: Option<gio::OwnerId>,
}
fn metadata(ui: &Ui) -> glib::Variant {
    let mut map: HashMap<String, glib::Variant> = HashMap::new();
    if let Some(t) = ui.current_track() {
        let path =
            glib::variant::ObjectPath::try_from(format!("/io/github/bilimusic/track/{}", t.bvid))
                .expect("validated BV");
        map.insert("mpris:trackid".into(), path.to_variant());
        map.insert(
            "mpris:length".into(),
            ((t.duration.min(i64::MAX as u64 / 1_000_000) as i64) * 1_000_000).to_variant(),
        );
        map.insert("xesam:title".into(), t.title.to_variant());
        map.insert("xesam:artist".into(), vec![t.artist].to_variant());
        map.insert(
            "xesam:url".into(),
            format!("https://www.bilibili.com/video/{}", t.bvid).to_variant(),
        );
    }
    map.to_variant()
}
fn property(ui: &Ui, name: &str) -> glib::Variant {
    match name {
        "Identity" => "Buisc".to_variant(),
        "DesktopEntry" => "io.github.bilimusic.Linux".to_variant(),
        "CanQuit" | "CanRaise" | "CanGoNext" | "CanGoPrevious" | "CanPlay" | "CanPause"
        | "CanControl" => true.to_variant(),
        "HasTrackList" => false.to_variant(),
        "CanSeek" => ui.current_track().is_some().to_variant(),
        "SupportedUriSchemes" | "SupportedMimeTypes" => Vec::<String>::new().to_variant(),
        "PlaybackStatus" => ui.playback_status().to_variant(),
        "Metadata" => metadata(ui),
        "Volume" => ui.volume().to_variant(),
        "Position" => {
            ((ui.position().min(i64::MAX as u64 / 1_000_000) as i64) * 1_000_000).to_variant()
        }
        "Rate" | "MinimumRate" | "MaximumRate" => 1f64.to_variant(),
        _ => false.to_variant(),
    }
}
impl Mpris {
    pub fn new(ui: &Rc<Ui>) -> Result<Self, String> {
        let connection = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
            .map_err(|e| e.to_string())?;
        let node = gio::DBusNodeInfo::for_xml(XML).map_err(|e| e.to_string())?;
        let mut registrations = vec![];
        for interface in ["org.mpris.MediaPlayer2", PLAYER] {
            let info = node
                .lookup_interface(interface)
                .ok_or("MPRIS interface missing")?;
            let weak = Rc::downgrade(ui);
            let weak_prop = weak.clone();
            let weak_set = weak.clone();
            let registration = connection
                .register_object(PATH, &info)
                .method_call(move |connection, _, _, _, method, params, invocation| {
                    let Some(ui) = weak.upgrade() else {
                        invocation.return_dbus_error("org.mpris.Error.Closed", "播放器已关闭");
                        return;
                    };
                    match method {
                        "Raise" => ui.show_window(),
                        "Quit" => ui.quit(),
                        "Next" => ui.next(false),
                        "Previous" => ui.previous(),
                        "Pause" => ui.pause(),
                        "PlayPause" => ui.toggle(),
                        "Stop" => ui.stop(),
                        "Play" => ui.play(),
                        "Seek" => {
                            if let Some((offset,)) = params.get::<(i64,)>() {
                                let pos = (ui.position() as i64)
                                    .saturating_mul(1_000_000)
                                    .saturating_add(offset)
                                    .max(0);
                                ui.seek_to((pos / 1_000_000) as u64);
                                let _ = connection.emit_signal(
                                    None,
                                    PATH,
                                    PLAYER,
                                    "Seeked",
                                    Some(&(pos,).to_variant()),
                                );
                            }
                        }
                        "SetPosition" => {
                            if let Some((id, pos)) =
                                params.get::<(glib::variant::ObjectPath, i64)>()
                                && ui.current_track().is_some_and(|t| {
                                    id.as_str() == format!("/io/github/bilimusic/track/{}", t.bvid)
                                })
                                && pos >= 0
                            {
                                ui.seek_to((pos / 1_000_000) as u64);
                                let _ = connection.emit_signal(
                                    None,
                                    PATH,
                                    PLAYER,
                                    "Seeked",
                                    Some(&(pos,).to_variant()),
                                );
                            }
                        }
                        _ => {
                            invocation.return_dbus_error(
                                "org.freedesktop.DBus.Error.UnknownMethod",
                                "未知方法",
                            );
                            return;
                        }
                    }
                    invocation.return_value(Some(&().to_variant()));
                })
                .property(move |_, _, _, _, name| {
                    weak_prop
                        .upgrade()
                        .map_or_else(|| false.to_variant(), |ui| property(&ui, name))
                })
                .set_property(move |_, _, _, _, name, value| {
                    let Some(ui) = weak_set.upgrade() else {
                        return false;
                    };
                    match name {
                        "Volume" => {
                            if let Some(v) = value.get::<f64>() {
                                ui.set_volume(v);
                                ui.notify_mpris();
                                true
                            } else {
                                false
                            }
                        }
                        "Rate" => value.get::<f64>() == Some(1.),
                        _ => false,
                    }
                })
                .build()
                .map_err(|e| e.to_string())?;
            registrations.push(registration);
        }
        let bus_name =
            if ui.window.application().is_some_and(|app| {
                app.application_id().as_deref() == Some("io.github.bilimusic.Test")
            }) {
                "org.mpris.MediaPlayer2.bilimusic.test"
            } else {
                "org.mpris.MediaPlayer2.bilimusic"
            };
        let owner = gio::bus_own_name_on_connection(
            &connection,
            bus_name,
            gio::BusNameOwnerFlags::NONE,
            |_, _| {},
            |_, _| {},
        );
        Ok(Self {
            connection,
            registrations,
            owner: Some(owner),
        })
    }
    pub fn changed(&self, ui: &Ui) {
        let mut changed: HashMap<String, glib::Variant> = HashMap::new();
        for name in ["PlaybackStatus", "Metadata", "Volume", "CanSeek"] {
            changed.insert(name.into(), property(ui, name));
        }
        let _ = self.connection.emit_signal(
            None,
            PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            Some(&(PLAYER, changed, Vec::<String>::new()).to_variant()),
        );
    }
}
impl Drop for Mpris {
    fn drop(&mut self) {
        for id in self.registrations.drain(..) {
            let _ = self.connection.unregister_object(id);
        }
        if let Some(owner) = self.owner.take() {
            gio::bus_unown_name(owner);
        }
    }
}
