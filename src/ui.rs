mod account_ui;
mod discover;
mod download_ui;
mod glass;
mod icons;
mod smooth_scroll;
use crate::{
    api::{AudioStream, BiliClient, SearchResult, Track},
    audio::{AudioEvent, AudioPlayer},
    storage::{self, Library},
};
use gtk::{gio, glib, prelude::*};
use icons::Icon;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, PartialEq)]
enum Route {
    Discover,
    Search,
    Favorites,
    History,
    Queue,
    Playlist(String),
    RemoteFavorites,
    RemoteFolder(u64, String),
    RemoteHistory,
}
enum Event {
    List(u64, Result<SearchResult, String>, bool),
    Stream(u64, Box<Track>, Result<AudioStream, String>),
    Cover(u64, Result<Vec<u8>, String>),
    Download(PathBuf, Result<(), String>),
}

pub struct Ui {
    pub window: gtk::ApplicationWindow,
    api: BiliClient,
    account: crate::account::AccountClient,
    account_button: gtk::Button,
    profile: RefCell<Option<crate::account::AccountProfile>>,
    last_report: RefCell<Option<(String, std::time::Instant, u64)>>,
    report_inflight: Cell<bool>,
    audio: RefCell<Option<AudioPlayer>>,
    lib: RefCell<Library>,
    path: PathBuf,
    route: RefCell<Route>,
    current: RefCell<Option<Track>>,
    shown: RefCell<Vec<Track>>,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    heading: gtk::Label,
    subtitle: gtk::Label,
    discovery: discover::Discovery,
    list_panel: gtk::Box,
    status: gtk::Label,
    empty: gtk::Label,
    search: gtk::SearchEntry,
    search_button: gtk::Button,
    has_more: Cell<bool>,
    play_button: gtk::Button,
    now_title: gtk::Label,
    now_artist: gtk::Label,
    cover_stack: gtk::Stack,
    cover_picture: gtk::Picture,
    favorite_button: gtk::Button,
    download_button: gtk::Button,
    downloading: Cell<bool>,
    download_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    seek: gtk::Scale,
    volume_scale: gtk::Scale,
    stopped: Cell<bool>,
    clock: gtk::Label,
    mode: gtk::Button,
    playlist_box: gtk::Box,
    nav: gtk::ListBox,
    tx: async_channel::Sender<Event>,
    rx: RefCell<Option<async_channel::Receiver<Event>>>,
    list_generation: Cell<u64>,
    play_generation: Cell<u64>,
    stream_inflight: Cell<bool>,
    pending_stream: RefCell<Option<(u64, Track)>>,
    page: Cell<u32>,
    seeking: Cell<bool>,
    updating_seek: Cell<bool>,
    timer: RefCell<Option<glib::SourceId>>,
    pending_save: RefCell<Option<glib::SourceId>>,
    loading: Cell<bool>,
    active_requests: Cell<u32>,
    query: RefCell<String>,
    pub mpris: RefCell<Option<crate::mpris::Mpris>>,
    tray: RefCell<Option<crate::tray::Tray>>,
    background_hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    quitting: Cell<bool>,
    settings_window: RefCell<Option<gtk::Window>>,
}
fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.);
    if !class.is_empty() {
        l.add_css_class(class);
    }
    l
}
fn button(text: &str, class: &str) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    if !class.is_empty() {
        b.add_css_class(class);
    }
    b
}
fn margins(w: &impl IsA<gtk::Widget>, n: i32) {
    w.set_margin_top(n);
    w.set_margin_bottom(n);
    w.set_margin_start(n);
    w.set_margin_end(n);
}
pub fn time_label(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

impl Ui {
    pub fn new(app: &gtk::Application) -> Result<Rc<Self>, String> {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(include_str!("../assets/style.css"));
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Buisc")
            .default_width(1140)
            .default_height(760)
            .build();
        window.set_size_request(860, 580);
        let path = storage::data_path();
        let (lib, warning, path) = match storage::load(&path) {
            Ok(l) => (l, None, path),
            Err(e) => (
                Library::default(),
                Some(e),
                path.with_file_name("library.recovered.json"),
            ),
        };
        // Keep client-side resize borders without a separate titlebar row.
        let titlebar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        titlebar.set_visible(false);
        window.set_titlebar(Some(&titlebar));
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.set_vexpand(true);
        root.append(&body);
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 6);
        sidebar.set_width_request(210);
        sidebar.set_hexpand(false);
        sidebar.add_css_class("sidebar");
        margins(&sidebar, 0);
        let brand_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        brand_row.set_margin_start(22);
        brand_row.set_margin_top(24);
        brand_row.set_margin_bottom(5);
        brand_row.append(&icons::artwork(30));
        brand_row.append(&label("Buisc", "brand"));
        let brand_handle = gtk::WindowHandle::new();
        brand_handle.set_child(Some(&brand_row));
        sidebar.append(&brand_handle);
        let tagline = label("Bilibili 音乐播放器", "tagline");
        tagline.set_margin_start(28);
        tagline.set_margin_bottom(24);
        sidebar.append(&tagline);
        let section = label("音乐资料库", "section");
        section.set_margin_start(28);
        sidebar.append(&section);
        let nav = gtk::ListBox::new();
        nav.add_css_class("navigation");
        nav.set_selection_mode(gtk::SelectionMode::Single);
        for (text, icon) in [
            ("音乐热榜", Icon::Home),
            ("搜索", Icon::Search),
            ("本地收藏", Icon::Heart),
            ("最近播放", Icon::History),
            ("播放队列", Icon::Queue),
            ("B站收藏夹", Icon::Cloud),
            ("B站历史", Icon::History),
        ] {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 11);
            margins(&row, 10);
            row.set_margin_start(12);
            row.append(&icons::widget(icon, 18, true));
            row.append(&label(text, ""));
            nav.append(&row);
        }
        sidebar.append(&nav);
        let pl_head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pl_head.set_margin_top(24);
        pl_head.set_margin_start(28);
        pl_head.set_margin_end(20);
        let pl_label = label("我的歌单", "section");
        pl_label.set_hexpand(true);
        pl_head.append(&pl_label);
        let new_playlist = button("", "sidebar-button");
        new_playlist.set_child(Some(&icons::widget(Icon::Plus, 16, false)));
        new_playlist.set_tooltip_text(Some("新建歌单"));
        pl_head.append(&new_playlist);
        sidebar.append(&pl_head);
        let playlist_box = gtk::Box::new(gtk::Orientation::Vertical, 3);
        let pl_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&playlist_box)
            .build();
        sidebar.append(&pl_scroll);
        let account_button = button("扫码登录 B站", "sidebar-button");
        account_button.add_css_class("account-card");
        account_button.set_margin_start(18);
        account_button.set_margin_end(18);
        sidebar.append(&account_button);
        let settings = button("", "sidebar-button");
        let settings_row = gtk::Box::new(gtk::Orientation::Horizontal, 9);
        settings_row.append(&icons::widget(Icon::Settings, 16, false));
        settings_row.append(&label("设置与备份", ""));
        settings.set_child(Some(&settings_row));
        margins(&settings, 18);
        sidebar.append(&settings);
        body.append(&sidebar);
        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.set_widget_name("main-pane");
        main.set_hexpand(true);
        let main_overlay = gtk::Overlay::new();
        main_overlay.set_hexpand(true);
        main_overlay.set_child(Some(&main));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_vexpand(true);
        margins(&content, 30);
        content.set_margin_top(44);
        content.set_margin_bottom(8);
        main.append(&content);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("搜索歌曲、UP 主，或粘贴 BV 号 / 视频链接"));
        search.set_tooltip_text(Some(
            "关键词搜索自动附加“音乐”筛选；BV 号和视频链接可直接打开。",
        ));
        search.set_hexpand(true);
        top.append(&search);
        let search_button = button("搜索", "primary");
        top.append(&search_button);
        let window_controls = gtk::WindowControls::new(gtk::PackType::End);
        window_controls.set_decoration_layout(Some(":minimize,maximize,close"));
        window_controls.add_css_class("corner-window-controls");
        window_controls.set_halign(gtk::Align::End);
        window_controls.set_valign(gtk::Align::Start);
        window_controls.set_margin_top(6);
        window_controls.set_margin_end(8);
        main_overlay.add_overlay(&window_controls);
        let top_handle = gtk::WindowHandle::new();
        top_handle.set_child(Some(&top));
        content.append(&top_handle);
        let heading = label("音乐热榜", "heading");
        heading.set_margin_top(12);
        content.append(&heading);
        let subtitle = label("从 B 站音乐区，找到今天想听的声音。", "muted");
        content.append(&subtitle);
        let discovery = discover::Discovery::new();
        content.append(&discovery.scroll);
        let list_panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
        list_panel.set_vexpand(true);
        list_panel.set_margin_bottom(78);
        content.append(&list_panel);
        let tools = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        tools.add_css_class("toolbar");
        let play_all = button("播放全部", "primary");
        tools.append(&play_all);
        let play_selected = button("播放选中", "");
        play_selected.set_visible(false);
        let favorite = button("收藏", "");
        tools.append(&favorite);
        let enqueue = button("加入队列", "");
        tools.append(&enqueue);
        let add_playlist = button("加入歌单", "");
        tools.append(&add_playlist);
        let bili_favorite = button("B站收藏", "");
        tools.append(&bili_favorite);
        let remove = button("移除", "");
        tools.append(&remove);
        list_panel.append(&tools);
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::SingleSelection::new(Some(store.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        let table = gtk::ColumnView::new(Some(selection.clone()));
        table.set_vexpand(true);
        table.set_show_row_separators(false);
        table.add_css_class("tracks");
        for (name, field, expand, width) in [
            ("#", 0, false, 58),
            ("曲目", 1, true, 200),
            ("UP 主", 2, false, 130),
            ("播放量", 3, false, 104),
            ("时长", 4, false, 68),
        ] {
            let factory = gtk::SignalListItemFactory::new();
            factory.connect_setup(move |_, object| {
                let item = object.downcast_ref::<gtk::ListItem>().expect("list item");
                let text = label("", if field == 1 { "song-title" } else { "muted" });
                text.set_ellipsize(gtk::pango::EllipsizeMode::End);
                text.set_margin_top(9);
                text.set_margin_bottom(9);
                text.set_margin_start(9);
                text.set_margin_end(9);
                item.set_child(Some(&text));
            });
            factory.connect_bind(move |_, object| {
                let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(obj) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                    return;
                };
                let track = obj.borrow::<Track>();
                let text = match field {
                    0 => format!("{:02}", item.position() + 1),
                    1 => track.title.clone(),
                    2 => track.artist.clone(),
                    3 => {
                        if track.play_count >= 10000 {
                            format!("{:.1}万", track.play_count as f64 / 10000.)
                        } else {
                            track.play_count.to_string()
                        }
                    }
                    _ => time_label(track.duration),
                };
                if let Some(l) = item.child().and_downcast::<gtk::Label>() {
                    l.set_text(&text);
                    l.set_tooltip_text(Some(&text));
                }
            });
            let column = gtk::ColumnViewColumn::new(Some(name), Some(factory));
            column.set_expand(expand);
            column.set_fixed_width(width);
            column.set_resizable(field == 1 || field == 2);
            table.append_column(&column);
        }
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&table)
            .build();
        smooth_scroll::attach(&scroll);
        list_panel.append(&scroll);
        let empty = label("正在连接 B 站音乐榜…", "muted");
        empty.set_halign(gtk::Align::Center);
        list_panel.append(&empty);
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let status = label("准备就绪", "status");
        status.set_hexpand(true);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.connect_label_notify(|status| status.set_tooltip_text(Some(&status.text())));
        foot.append(&status);
        content.append(&foot);
        let player = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        player.set_halign(gtk::Align::Center);
        player.set_valign(gtk::Align::End);
        player.set_margin_bottom(26);
        player.set_margin_start(12);
        player.set_margin_end(12);
        player.set_widget_name("player-bar");
        player.add_css_class("player");
        let identity = gtk::Box::new(gtk::Orientation::Horizontal, 9);
        identity.set_valign(gtk::Align::Center);
        let art = gtk::Stack::new();
        art.add_css_class("video-cover");
        art.set_overflow(gtk::Overflow::Hidden);
        art.set_size_request(42, 42);
        art.set_valign(gtk::Align::Center);
        art.add_named(&icons::artwork(42), Some("fallback"));
        let cover_picture = gtk::Picture::new();
        cover_picture.set_can_shrink(true);
        cover_picture.set_content_fit(gtk::ContentFit::Cover);
        art.add_named(&cover_picture, Some("cover"));
        art.set_visible_child_name("fallback");
        identity.append(&art);
        let now = gtk::Box::new(gtk::Orientation::Vertical, 3);
        now.set_valign(gtk::Align::Center);
        let now_title = label("还没有开始播放", "song-title");
        now_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        now_title.set_width_chars(10);
        now_title.set_max_width_chars(13);
        let now_artist = label("双击曲目，听见喜欢", "muted");
        now_artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
        now_artist.set_max_width_chars(13);
        now.append(&now_title);
        now.append(&now_artist);
        identity.append(&now);
        player.append(&identity);

        let center = gtk::Box::new(gtk::Orientation::Vertical, 3);
        let upper = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let transport = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        transport.add_css_class("transport-group");
        transport.set_valign(gtk::Align::Center);
        let previous = gtk::Button::new();
        previous.set_child(Some(&icons::widget(Icon::Previous, 18, false)));
        previous.add_css_class("transport");
        previous.set_tooltip_text(Some("上一首"));
        let play_button = gtk::Button::new();
        icons::set_play(&play_button, false);
        play_button.add_css_class("play");
        play_button.set_tooltip_text(Some("播放 / 暂停"));
        let next = gtk::Button::new();
        next.set_child(Some(&icons::widget(Icon::Next, 18, false)));
        next.add_css_class("transport");
        next.set_tooltip_text(Some("下一首"));
        transport.append(&previous);
        transport.append(&play_button);
        transport.append(&next);
        upper.append(&transport);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        actions.add_css_class("player-actions");
        actions.set_valign(gtk::Align::Center);
        let favorite_button = button("", "transport");
        favorite_button.set_child(Some(&icons::widget(Icon::Heart, 18, false)));
        favorite_button.set_tooltip_text(Some("收藏当前歌曲到本机；同步到账号请用「B站收藏」"));
        let download_button = button("", "transport");
        download_button.set_icon_name("folder-download-symbolic");
        download_button.set_tooltip_text(Some("下载当前歌曲；未播放时下载选中曲目"));
        let mode = button(lib.mode.label(), "small");
        mode.add_css_class("player-mode");
        mode.set_valign(gtk::Align::Center);
        mode.set_tooltip_text(Some("切换顺序 / 单曲循环 / 列表循环 / 随机播放"));
        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 100., 1.);
        volume.set_width_request(64);
        volume.set_value(lib.volume * 100.);
        volume.set_draw_value(false);
        volume.set_tooltip_text(Some("音量"));
        actions.append(&favorite_button);
        actions.append(&download_button);
        actions.append(&mode);
        actions.append(&volume);
        upper.append(&actions);
        center.append(&upper);

        let progress = gtk::Box::new(gtk::Orientation::Horizontal, 9);
        let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1., 1.);
        seek.set_draw_value(false);
        seek.set_sensitive(false);
        seek.set_hexpand(true);
        seek.set_tooltip_text(Some("播放进度"));
        let clock = label("0:00 / 0:00", "muted");
        clock.add_css_class("player-clock");
        clock.set_xalign(1.);
        clock.set_width_chars(13);
        progress.append(&seek);
        progress.append(&clock);
        center.append(&progress);
        player.append(&center);
        body.append(&glass::GlassPane::new(&main_overlay, &player));
        window.set_child(Some(&root));
        let (tx, rx) = async_channel::bounded(16);
        let account = crate::account::AccountClient::new(path.with_file_name("account.json"))?;
        let mut api = BiliClient::new()?;
        api.set_account(account.clone());
        let ui = Rc::new(Self {
            window,
            api,
            account,
            account_button,
            profile: RefCell::new(None),
            last_report: RefCell::new(None),
            report_inflight: Cell::new(false),
            audio: RefCell::new(None),
            lib: RefCell::new(lib),
            path,
            route: RefCell::new(Route::Discover),
            current: RefCell::new(None),
            shown: RefCell::new(vec![]),
            store,
            selection,
            heading,
            subtitle,
            discovery,
            list_panel,
            status,
            empty,
            search,
            search_button,
            has_more: Cell::new(false),
            play_button,
            now_title,
            now_artist,
            cover_stack: art,
            cover_picture,
            favorite_button,
            download_button,
            downloading: Cell::new(false),
            download_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            seek,
            volume_scale: volume.clone(),
            stopped: Cell::new(true),
            clock,
            mode,
            playlist_box,
            nav: nav.clone(),
            tx,
            rx: RefCell::new(Some(rx)),
            list_generation: Cell::new(0),
            play_generation: Cell::new(0),
            stream_inflight: Cell::new(false),
            pending_stream: RefCell::new(None),
            page: Cell::new(1),
            seeking: Cell::new(false),
            updating_seek: Cell::new(false),
            timer: RefCell::new(None),
            pending_save: RefCell::new(None),
            loading: Cell::new(false),
            active_requests: Cell::new(0),
            query: RefCell::new(String::new()),
            mpris: RefCell::new(None),
            tray: RefCell::new(None),
            background_hold: RefCell::new(None),
            quitting: Cell::new(false),
            settings_window: RefCell::new(None),
        });
        let weak = Rc::downgrade(&ui);
        let audio = AudioPlayer::new(move |event| {
            if let Some(ui) = weak.upgrade() {
                ui.audio_event(event);
            }
        })?;
        audio.set_volume(ui.lib.borrow().volume);
        *ui.audio.borrow_mut() = Some(audio);
        macro_rules! clicked {
            ($widget:expr,$body:expr) => {{
                let weak = Rc::downgrade(&ui);
                $widget.connect_clicked(move |_| {
                    if let Some(ui) = weak.upgrade() {
                        $body(ui);
                    }
                });
            }};
        }
        clicked!(ui.search_button, |ui: Rc<Self>| ui.run_search(false));
        {
            let weak = Rc::downgrade(&ui);
            ui.search.connect_activate(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.run_search(false);
                }
            });
        }
        {
            let weak = Rc::downgrade(&ui);
            scroll.connect_edge_reached(move |_, edge| {
                if edge == gtk::PositionType::Bottom
                    && let Some(ui) = weak.upgrade()
                {
                    ui.load_next_page();
                }
            });
            let weak = Rc::downgrade(&ui);
            scroll
                .vadjustment()
                .connect_value_changed(move |adjustment| {
                    let bottom = adjustment.upper() - adjustment.page_size();
                    if bottom > adjustment.lower()
                        && adjustment.value() >= bottom - 1.
                        && let Some(ui) = weak.upgrade()
                    {
                        ui.load_next_page();
                    }
                });
        }
        clicked!(play_all, |ui: Rc<Self>| ui.play_all());
        clicked!(
            play_selected,
            |ui: Rc<Self>| if let Some(t) = ui.selected() {
                ui.play_track(t);
            }
        );
        clicked!(favorite, |ui: Rc<Self>| if let Some(t) = ui.selected() {
            ui.favorite(&t);
        });
        clicked!(enqueue, |ui: Rc<Self>| if let Some(t) = ui.selected() {
            ui.enqueue(t);
        });
        clicked!(add_playlist, |ui: Rc<Self>| ui.choose_playlist());
        clicked!(bili_favorite, |ui: Rc<Self>| ui.favorite_remote());
        clicked!(ui.account_button, |ui: Rc<Self>| ui.account_dialog());
        clicked!(remove, |ui: Rc<Self>| ui.remove_selected());
        clicked!(new_playlist, |ui: Rc<Self>| ui.new_playlist());
        clicked!(settings, |ui: Rc<Self>| ui.settings());
        clicked!(ui.play_button, |ui: Rc<Self>| ui.toggle());
        clicked!(ui.download_button, |ui: Rc<Self>| ui.download_audio());
        clicked!(previous, |ui: Rc<Self>| ui.previous());
        clicked!(next, |ui: Rc<Self>| ui.next(false));
        clicked!(ui.favorite_button, |ui: Rc<Self>| {
            let t = ui.current.borrow().clone();
            if let Some(t) = t {
                ui.favorite(&t);
            }
        });
        clicked!(ui.mode, |ui: Rc<Self>| {
            let next = ui.lib.borrow().mode.next();
            ui.mode.set_label(next.label());
            ui.lib.borrow_mut().mode = next;
            ui.schedule_save();
        });
        {
            let weak = Rc::downgrade(&ui);
            nav.connect_row_selected(move |_, row| {
                if let (Some(ui), Some(row)) = (weak.upgrade(), row) {
                    let route = match row.index() {
                        0 => Route::Discover,
                        1 => Route::Search,
                        2 => Route::Favorites,
                        3 => Route::History,
                        4 => Route::Queue,
                        5 => Route::RemoteFavorites,
                        _ => Route::RemoteHistory,
                    };
                    if *ui.route.borrow() != route {
                        ui.navigate(route);
                    }
                }
            });
        }
        {
            let weak = Rc::downgrade(&ui);
            nav.connect_row_activated(move |_, row| {
                if row.index() == 0
                    && let Some(ui) = weak.upgrade()
                    && *ui.route.borrow() == Route::Discover
                    && !ui.loading.get()
                {
                    ui.navigate(Route::Discover);
                }
            });
        }
        {
            let weak = Rc::downgrade(&ui);
            table.connect_activate(move |_, index| {
                if let Some(ui) = weak.upgrade() {
                    let track = ui.shown.borrow().get(index as usize).cloned();
                    if let Some(track) = track {
                        ui.play_track(track);
                    }
                }
            });
        }
        {
            let weak = Rc::downgrade(&ui);
            volume.connect_value_changed(move |s| {
                if let Some(ui) = weak.upgrade() {
                    let value = s.value() / 100.;
                    ui.lib.borrow_mut().volume = value;
                    if let Some(a) = ui.audio.borrow().as_ref() {
                        a.set_volume(value);
                    }
                    ui.schedule_save();
                    ui.notify_mpris();
                }
            });
        }
        {
            let weak = Rc::downgrade(&ui);
            ui.seek.connect_change_value(move |_, _, value| {
                if let Some(ui) = weak.upgrade()
                    && !ui.updating_seek.get()
                    && let Some(a) = ui.audio.borrow().as_ref()
                    && let Err(error) = a.seek(value.max(0.) as u64)
                {
                    ui.status.set_text(&error);
                }
                glib::Propagation::Proceed
            });
        }
        {
            let weak = Rc::downgrade(&ui);
            ui.window.connect_close_request(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.close_requested()
                } else {
                    glib::Propagation::Proceed
                }
            });
        }
        {
            let weak = Rc::downgrade(&ui);
            ui.window.connect_visible_notify(move |window| {
                if window.is_visible()
                    && let Some(ui) = weak.upgrade()
                {
                    ui.background_hold.borrow_mut().take();
                }
            });
        }
        ui.rebuild_playlists();
        ui.install_actions(app);
        if let Some(warning) = warning {
            ui.status.set_text(&warning);
        }
        Ok(ui)
    }

    pub fn start(self: Rc<Self>) {
        self.window.present();
        let weak = Rc::downgrade(&self);
        let rx = self.rx.borrow_mut().take().expect("event receiver");
        glib::spawn_future_local(async move {
            while let Ok(event) = rx.recv().await {
                if let Some(ui) = weak.upgrade() {
                    ui.handle(event);
                } else {
                    break;
                }
            }
        });
        match crate::mpris::Mpris::new(&self) {
            Ok(m) => *self.mpris.borrow_mut() = Some(m),
            Err(e) => eprintln!("桌面媒体控制不可用：{e}"),
        }
        self.configure_tray(self.lib.borrow().close_to_tray);
        self.navigate(Route::Discover);
        self.refresh_account();
        if std::env::var("BILI_MUSIC_SHOW_LOGIN").as_deref() == Ok("1")
            && !self.account.is_logged_in()
        {
            self.account_dialog();
        }
        // Keep the controller alive with the application window; release it on destroy.
        let keeper = Rc::new(RefCell::new(Some(self.clone())));
        self.window.connect_destroy(move |_| {
            keeper.borrow_mut().take();
        });
    }
    fn configure_tray(self: &Rc<Self>, enabled: bool) {
        if enabled {
            if self.tray.borrow().is_none() {
                match crate::tray::Tray::new(self) {
                    Ok(tray) => *self.tray.borrow_mut() = Some(tray),
                    Err(error) => self.status.set_text(&format!("系统托盘暂不可用：{error}")),
                }
            }
        } else {
            if !self.window.is_visible() {
                self.show_window();
            }
            self.tray.borrow_mut().take();
            self.background_hold.borrow_mut().take();
        }
    }
    fn set_close_to_tray(self: &Rc<Self>, enabled: bool) {
        self.lib.borrow_mut().close_to_tray = enabled;
        self.configure_tray(enabled);
        self.schedule_save();
    }
    fn close_requested(self: &Rc<Self>) -> glib::Propagation {
        self.flush_save();
        let background = !self.quitting.get()
            && self.lib.borrow().close_to_tray
            && self
                .tray
                .borrow()
                .as_ref()
                .is_some_and(|tray| tray.is_available());
        if background {
            self.close_settings();
            if self.background_hold.borrow().is_none()
                && let Some(app) = self.window.application()
            {
                *self.background_hold.borrow_mut() = Some(app.hold());
            }
            self.window.set_visible(false);
            glib::Propagation::Stop
        } else {
            self.finish_shutdown();
            glib::Propagation::Proceed
        }
    }
    fn finish_shutdown(self: &Rc<Self>) {
        self.quitting.set(true);
        self.download_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.close_settings();
        self.stop();
        self.flush_save();
        self.background_hold.borrow_mut().take();
        self.tray.borrow_mut().take();
    }
    pub fn show_window(self: &Rc<Self>) {
        if self.quitting.get() {
            return;
        }
        self.window.present();
        self.background_hold.borrow_mut().take();
    }
    pub fn quit(self: &Rc<Self>) {
        if self.quitting.get() {
            return;
        }
        self.finish_shutdown();
        self.window.destroy();
    }
    fn install_actions(self: &Rc<Self>, app: &gtk::Application) {
        let quit = gio::SimpleAction::new("quit", None);
        let weak = Rc::downgrade(self);
        quit.connect_activate(move |_, _| {
            if let Some(ui) = weak.upgrade() {
                ui.quit();
            }
        });
        app.add_action(&quit);
        app.set_accels_for_action("app.quit", &["<Primary>q"]);
        for (name, accels, kind) in [
            ("play-pause", vec!["<Primary>space", "AudioPlay"], 0),
            ("next", vec!["<Primary>Right", "AudioNext"], 1),
            ("previous", vec!["<Primary>Left", "AudioPrev"], 2),
            ("search", vec!["<Primary>f"], 3),
        ] {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(ui) = weak.upgrade() {
                    match kind {
                        0 => ui.toggle(),
                        1 => ui.next(false),
                        2 => ui.previous(),
                        _ => {
                            ui.search.grab_focus();
                        }
                    }
                }
            });
            app.add_action(&action);
            app.set_accels_for_action(&format!("app.{name}"), &accels);
        }
    }
    fn selected(&self) -> Option<Track> {
        self.selection
            .selected_item()
            .and_downcast::<glib::BoxedAnyObject>()
            .map(|o| o.borrow::<Track>().clone())
    }
    fn set_tracks(&self, tracks: Vec<Track>) {
        let previous = self.shown.borrow();
        let unchanged_prefix = tracks.starts_with(&previous);
        let start = if unchanged_prefix { previous.len() } else { 0 };
        drop(previous);
        if !unchanged_prefix {
            self.store.remove_all();
        }
        for t in &tracks[start..] {
            self.store.append(&glib::BoxedAnyObject::new(t.clone()));
        }
        self.empty.set_visible(tracks.is_empty());
        self.empty
            .set_text("这里还没有曲目。试试搜索一首喜欢的歌。 ");
        self.subtitle.set_text(&format!(
            "{} 首曲目 · 双击播放 · 选中后可收藏或加入歌单",
            tracks.len()
        ));
        *self.shown.borrow_mut() = tracks;
    }
    fn sync_nav(&self) {
        let index = match *self.route.borrow() {
            Route::Discover => Some(0),
            Route::Search => Some(1),
            Route::Favorites => Some(2),
            Route::History => Some(3),
            Route::Queue => Some(4),
            Route::RemoteFavorites => Some(5),
            Route::RemoteHistory => Some(6),
            Route::Playlist(_) | Route::RemoteFolder(_, _) => None,
        };
        self.nav
            .select_row(index.and_then(|i| self.nav.row_at_index(i)).as_ref());
    }
    fn navigate(self: &Rc<Self>, route: Route) {
        self.list_generation.set(self.list_generation.get() + 1);
        *self.route.borrow_mut() = route.clone();
        self.sync_nav();
        self.loading.set(false);
        self.search_button.set_sensitive(true);
        self.has_more.set(false);
        self.show_discovery(route == Route::Discover);
        match route {
            Route::Discover => {
                self.heading.set_text("音乐热榜");
                self.discovery.loading();
                self.fetch_list(false, true);
            }
            Route::Search => {
                self.heading.set_text("搜索音乐");
                self.search.grab_focus();
                self.set_tracks(vec![]);
                self.status
                    .set_text("输入歌曲或 UP 主，也可以直接粘贴 BV 号。");
            }
            Route::Favorites => {
                self.heading.set_text("本地收藏");
                self.set_tracks(self.lib.borrow().favorites.clone());
                self.status.set_text("把喜欢的声音，留在这里。");
            }
            Route::History => {
                self.heading.set_text("最近播放");
                self.set_tracks(self.lib.borrow().history.clone());
                self.status
                    .set_text("保留最近 200 首，所有记录仅存于本机。");
            }
            Route::Queue => {
                self.heading.set_text("播放队列");
                self.set_tracks(self.lib.borrow().queue.clone());
                self.status
                    .set_text("按队列顺序播放，顺序模式到末尾会停止。");
            }
            Route::RemoteFavorites => {
                self.heading.set_text("B站收藏夹");
                self.set_tracks(vec![]);
                self.status
                    .set_text("登录后读取账号收藏夹，收藏内容与 B 站同步。");
                self.open_remote_folders(None);
            }
            Route::RemoteFolder(_, ref name) => {
                self.heading.set_text(&format!("B站 · {name}"));
                self.load_remote(false);
            }
            Route::RemoteHistory => {
                self.heading.set_text("B站播放历史");
                self.load_remote(false);
            }
            Route::Playlist(name) => {
                self.heading.set_text(&name);
                self.set_tracks(
                    self.lib
                        .borrow()
                        .playlists
                        .get(&name)
                        .cloned()
                        .unwrap_or_default(),
                );
                self.status.set_text("你的本地歌单。可在设置中导出备份。");
            }
        }
    }
    fn load_next_page(self: &Rc<Self>) {
        if self.loading.get() || !self.has_more.get() || self.quitting.get() {
            return;
        }
        if matches!(
            *self.route.borrow(),
            Route::Search | Route::RemoteFolder(..) | Route::RemoteHistory
        ) {
            self.run_search(true);
        }
    }
    fn run_search(self: &Rc<Self>, append: bool) {
        if self.loading.get() {
            return;
        }
        if append
            && matches!(
                *self.route.borrow(),
                Route::RemoteFolder(..) | Route::RemoteHistory
            )
        {
            self.load_remote(true);
            return;
        }
        let query = if append {
            self.query.borrow().clone()
        } else {
            self.search.text().trim().to_string()
        };
        if query.is_empty() {
            self.status.set_text("请输入歌曲、UP 主或 BV 号。");
            return;
        }
        if !append {
            self.has_more.set(false);
            *self.query.borrow_mut() = query;
            self.page.set(1);
        }
        *self.route.borrow_mut() = Route::Search;
        self.sync_nav();
        self.heading
            .set_text(&format!("搜索 · {}", self.query.borrow()));
        self.show_discovery(false);
        self.fetch_list(append, false);
    }
    fn fetch_list(self: &Rc<Self>, append: bool, recommend: bool) {
        if self.active_requests.get() >= 3 {
            self.status.set_text("已有请求正在处理，请稍后再试。");
            return;
        }
        self.active_requests.set(self.active_requests.get() + 1);
        self.loading.set(true);
        self.search_button.set_sensitive(false);
        self.status.set_text(if recommend {
            "正在获取 B 站音乐榜…"
        } else {
            "正在搜索…"
        });
        let generation = self.list_generation.get() + 1;
        self.list_generation.set(generation);
        let api = self.api.clone();
        let tx = self.tx.clone();
        let query = self.query.borrow().clone();
        let page = if append { self.page.get() + 1 } else { 1 };
        self.page.set(page);
        std::thread::spawn(move || {
            let result = if recommend {
                api.recommend().map(|tracks| SearchResult {
                    tracks,
                    has_more: false,
                })
            } else {
                api.search(&query, page)
            };
            let _ = tx.send_blocking(Event::List(generation, result, append));
        });
    }
    fn handle(self: &Rc<Self>, event: Event) {
        if matches!(&event, Event::List(..) | Event::Cover(..)) {
            self.active_requests
                .set(self.active_requests.get().saturating_sub(1));
        }
        match event {
            Event::Download(path, result) => {
                self.downloading.set(false);
                self.download_button.set_sensitive(true);
                self.status.set_text(&match result {
                    Ok(()) => format!("音频已保存：{}", path.display()),
                    Err(error) => format!("下载失败：{error}"),
                });
            }
            Event::Cover(generation, result) => {
                if generation != self.play_generation.get() || self.current.borrow().is_none() {
                    return;
                }
                if let Ok(bytes) = result {
                    let loader = gtk::gdk_pixbuf::PixbufLoader::new();
                    loader.set_size(42, 42);
                    if loader.write(&bytes).is_ok()
                        && loader.close().is_ok()
                        && let Some(pixbuf) = loader.pixbuf()
                    {
                        let texture = gtk::gdk::Texture::for_pixbuf(&pixbuf);
                        self.cover_picture.set_paintable(Some(&texture));
                        self.cover_stack.set_visible_child_name("cover");
                    }
                }
            }
            Event::List(generation, result, append) => {
                if generation != self.list_generation.get() {
                    return;
                }
                self.loading.set(false);
                self.search_button.set_sensitive(true);
                match result {
                    Ok(results) => {
                        let mut tracks = if append {
                            self.shown.borrow().clone()
                        } else {
                            vec![]
                        };
                        for t in results.tracks {
                            if !tracks.iter().any(|x| x.id == t.id) {
                                tracks.push(t);
                            }
                        }
                        self.set_tracks(tracks);
                        if *self.route.borrow() == Route::Discover {
                            self.discovery.render(self, &self.shown.borrow());
                        }
                        self.has_more.set(results.has_more);
                        self.status
                            .set_text("已更新 · 音源来自 Bilibili，播放前按需解析音频。");
                    }
                    Err(e) => {
                        if append {
                            self.page.set(self.page.get().saturating_sub(1));
                        } else {
                            self.set_tracks(vec![]);
                            if *self.route.borrow() == Route::Discover {
                                self.discovery.render(self, &[]);
                            }
                        }
                        self.status.set_text(&e);
                        self.empty
                            .set_text("暂时无法获取曲目。可以稍后重新搜索，或播放本地收藏。");
                    }
                }
            }
            Event::Stream(generation, track, result) => {
                self.stream_inflight.set(false);
                let pending = self.pending_stream.borrow_mut().take();
                if let Some((next_generation, next_track)) = pending {
                    if next_generation == self.play_generation.get() && !self.quitting.get() {
                        self.resolve_stream(next_generation, next_track);
                    }
                    return;
                }
                let track = *track;
                if generation != self.play_generation.get() {
                    return;
                }
                match result {
                    Ok(stream) => {
                        *self.current.borrow_mut() = Some(track.clone());
                        self.now_title.set_text(&track.title);
                        self.now_title.set_tooltip_text(Some(&track.title));
                        self.now_artist.set_text(&track.artist);
                        self.load_cover(generation, &track.cover);
                        self.seek.set_range(0., stream.duration.max(1) as f64);
                        self.seek.set_sensitive(true);
                        self.seek.set_value(0.);
                        let result = self
                            .audio
                            .borrow()
                            .as_ref()
                            .ok_or("音频组件不可用".to_string())
                            .and_then(|a| {
                                if !stream.initialization_range.is_empty()
                                    && !stream.index_range.is_empty()
                                {
                                    a.load_dash(
                                        &stream.url,
                                        stream.duration,
                                        &stream.initialization_range,
                                        &stream.index_range,
                                        stream.bandwidth,
                                        &stream.codecs,
                                    )
                                } else {
                                    a.load(&stream.url)
                                }
                            });
                        match result {
                            Ok(()) => {
                                self.stopped.set(false);
                                self.lib.borrow_mut().remember(track);
                                self.schedule_save();
                                self.status.set_text("正在缓冲音频…");
                                self.update_favorite();
                                self.notify_mpris();
                            }
                            Err(e) => self.status.set_text(&e),
                        }
                    }
                    Err(e) => {
                        self.status
                            .set_text(&format!("无法播放 {}：{e}", track.title));
                        icons::set_play(&self.play_button, false);
                    }
                }
            }
        }
    }
    fn reset_cover(&self) {
        self.cover_stack.set_visible_child_name("fallback");
        self.cover_picture.set_paintable(gtk::gdk::Paintable::NONE);
    }
    fn load_cover(&self, generation: u64, source: &str) {
        self.reset_cover();
        if source.is_empty() {
            return;
        }
        self.active_requests.set(self.active_requests.get() + 1);
        let api = self.api.clone();
        let source = source.to_owned();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(Event::Cover(generation, api.cover_thumbnail(&source)));
        });
    }
    fn enqueue(self: &Rc<Self>, track: Track) {
        let mut lib = self.lib.borrow_mut();
        if !lib.queue.iter().any(|t| t.id == track.id) {
            if lib.queue.len() >= 3000 {
                self.status.set_text("队列已达到 3000 首上限。");
                return;
            }
            lib.queue.push(track);
        }
        drop(lib);
        self.schedule_save();
        self.status.set_text("已加入播放队列。");
    }
    pub fn play_track(self: &Rc<Self>, track: Track) {
        self.sync_history(true);
        self.stopped.set(true);
        self.remove_timer();
        if let Some(a) = self.audio.borrow().as_ref() {
            a.stop();
        }
        self.enqueue(track.clone());
        let generation = self.play_generation.get() + 1;
        self.play_generation.set(generation);
        self.status.set_text(&format!("正在解析：{}", track.title));
        if self.stream_inflight.get() {
            *self.pending_stream.borrow_mut() = Some((generation, track));
        } else {
            self.resolve_stream(generation, track);
        }
    }
    fn resolve_stream(&self, generation: u64, track: Track) {
        self.stream_inflight.set(true);
        let api = self.api.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = api.resolve(&track.bvid);
            let _ = tx.send_blocking(Event::Stream(generation, Box::new(track), result));
        });
    }
    fn play_all(self: &Rc<Self>) {
        let tracks = self.shown.borrow().clone();
        if let Some(first) = tracks.first() {
            self.lib.borrow_mut().queue = tracks.clone();
            self.schedule_save();
            self.play_track(first.clone());
        } else {
            self.status.set_text("列表里还没有曲目。");
        }
    }
    pub fn toggle(self: &Rc<Self>) {
        let track = self.current.borrow().clone();
        if let Some(track) = track {
            if self.stopped.replace(false) {
                self.play_track(track);
            } else if let Some(a) = self.audio.borrow().as_ref() {
                a.toggle();
            }
        } else {
            let track = self
                .lib
                .borrow()
                .queue
                .first()
                .cloned()
                .or_else(|| self.selected());
            if let Some(track) = track {
                self.play_track(track);
            } else {
                self.play_all();
            }
        }
    }
    pub fn pause(self: &Rc<Self>) {
        self.pending_stream.borrow_mut().take();
        self.sync_history(true);
        self.play_generation.set(self.play_generation.get() + 1);
        if let Some(a) = self.audio.borrow().as_ref() {
            a.pause();
        }
    }
    pub fn play(self: &Rc<Self>) {
        let track = self.current.borrow().clone();
        if let Some(track) = track {
            if self.stopped.replace(false) {
                self.play_track(track);
            } else if let Some(a) = self.audio.borrow().as_ref() {
                a.play();
            }
        } else {
            self.toggle();
        }
    }
    pub fn stop(self: &Rc<Self>) {
        self.pending_stream.borrow_mut().take();
        self.sync_history(true);
        self.stopped.set(true);
        self.remove_timer();
        self.play_generation.set(self.play_generation.get() + 1);
        if let Some(a) = self.audio.borrow().as_ref() {
            a.stop();
        }
        icons::set_play(&self.play_button, false);
        self.notify_mpris();
    }
    pub fn next(self: &Rc<Self>, auto: bool) {
        let lib = self.lib.borrow();
        let id = self.current.borrow().as_ref().map(|t| t.id.clone());
        let index = lib.queue.iter().position(|t| Some(&t.id) == id.as_ref());
        let random = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as u64;
        let next = storage::next_index(index, lib.queue.len(), &lib.mode, auto, random);
        let track = next.and_then(|i| lib.queue.get(i).cloned());
        drop(lib);
        if let Some(t) = track {
            self.play_track(t);
        } else {
            self.stop();
            self.status.set_text("队列播放完毕。");
        }
    }
    pub fn previous(self: &Rc<Self>) {
        let lib = self.lib.borrow();
        let id = self.current.borrow().as_ref().map(|t| t.id.clone());
        let index = lib
            .queue
            .iter()
            .position(|t| Some(&t.id) == id.as_ref())
            .unwrap_or(0);
        let t = lib
            .queue
            .get(if index == 0 {
                lib.queue.len().saturating_sub(1)
            } else {
                index - 1
            })
            .cloned();
        drop(lib);
        if let Some(t) = t {
            self.play_track(t);
        }
    }
    fn favorite(self: &Rc<Self>, track: &Track) {
        self.lib.borrow_mut().toggle_favorite(track);
        self.update_favorite();
        self.schedule_save();
        let route = self.route.borrow().clone();
        if route == Route::Favorites {
            self.navigate(route);
        }
        self.status.set_text("收藏已更新。");
    }
    fn update_favorite(&self) {
        let yes = self
            .current
            .borrow()
            .as_ref()
            .is_some_and(|t| self.lib.borrow().favorites.iter().any(|f| f.id == t.id));
        self.favorite_button
            .set_child(Some(&icons::widget(Icon::Heart, 19, yes)));
    }
    fn remove_selected(self: &Rc<Self>) {
        let Some(track) = self.selected() else {
            return;
        };
        let route = self.route.borrow().clone();
        let mut lib = self.lib.borrow_mut();
        match &route {
            Route::Favorites => lib.favorites.retain(|t| t.id != track.id),
            Route::History => lib.history.retain(|t| t.id != track.id),
            Route::Queue => lib.queue.retain(|t| t.id != track.id),
            Route::Playlist(name) => {
                if let Some(tracks) = lib.playlists.get_mut(name) {
                    tracks.retain(|t| t.id != track.id);
                }
            }
            _ => {
                drop(lib);
                self.status
                    .set_text("可在收藏、最近播放、队列或歌单中移除曲目。");
                return;
            }
        }
        drop(lib);
        self.schedule_save();
        self.navigate(route);
        self.update_favorite();
    }
    fn audio_event(self: &Rc<Self>, event: AudioEvent) {
        match event {
            AudioEvent::Ended => self.next(true),
            AudioEvent::Error(e) => {
                self.stopped.set(true);
                self.remove_timer();
                self.status.set_text(&format!("音频播放失败：{e}"));
                icons::set_play(&self.play_button, false);
                self.notify_mpris();
            }
            AudioEvent::Playing(playing) => {
                icons::set_play(&self.play_button, playing);
                if playing {
                    self.status.set_text("正在播放 · 仅音频流，无视频解码");
                    self.ensure_timer();
                } else {
                    self.remove_timer();
                }
                self.notify_mpris();
            }
            AudioEvent::Duration(d) => {
                if d > 0 {
                    self.seek.set_range(0., d as f64);
                }
            }
        }
    }
    fn ensure_timer(self: &Rc<Self>) {
        if self.timer.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            let Some(ui) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if let Some(a) = ui.audio.borrow().as_ref() {
                let pos = a.position();
                let duration = a.duration();
                ui.updating_seek.set(true);
                if !ui.seeking.get() {
                    ui.seek.set_value(pos as f64);
                }
                ui.updating_seek.set(false);
                if duration > 0 {
                    ui.seek.set_range(0., duration as f64);
                }
                ui.clock
                    .set_text(&format!("{} / {}", time_label(pos), time_label(duration)));
            }
            ui.sync_history(false);
            glib::ControlFlow::Continue
        });
        *self.timer.borrow_mut() = Some(id);
    }
    fn remove_timer(&self) {
        if let Some(id) = self.timer.borrow_mut().take() {
            id.remove();
        }
    }
    fn schedule_save(self: &Rc<Self>) {
        if let Some(id) = self.pending_save.borrow_mut().take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            if let Some(ui) = weak.upgrade() {
                ui.pending_save.borrow_mut().take();
                ui.flush_save();
            }
        });
        *self.pending_save.borrow_mut() = Some(id);
    }
    fn flush_save(&self) {
        if let Err(e) = storage::save(&self.path, &self.lib.borrow()) {
            self.status.set_text(&e);
        }
    }
    fn rebuild_playlists(self: &Rc<Self>) {
        while let Some(child) = self.playlist_box.first_child() {
            self.playlist_box.remove(&child);
        }
        for name in self.lib.borrow().playlists.keys() {
            let b = button(&format!("♫   {name}"), "sidebar-button");
            b.set_halign(gtk::Align::Fill);
            b.set_margin_start(15);
            b.set_margin_end(15);
            let name = name.clone();
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.navigate(Route::Playlist(name.clone()));
                }
            });
            self.playlist_box.append(&b);
        }
    }
    fn new_playlist(self: &Rc<Self>) {
        let dialog = gtk::Dialog::builder()
            .title("新建歌单")
            .transient_for(&self.window)
            .modal(true)
            .build();
        dialog.add_button("取消", gtk::ResponseType::Cancel);
        dialog.add_button("创建", gtk::ResponseType::Accept);
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("歌单名称"));
        entry.set_max_length(60);
        margins(&entry, 20);
        dialog.content_area().append(&entry);
        dialog.set_default_response(gtk::ResponseType::Accept);
        entry.set_activates_default(true);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d, r| {
            if r == gtk::ResponseType::Accept
                && let Some(ui) = weak.upgrade()
            {
                let name = entry.text().trim().to_string();
                if name.is_empty() {
                    ui.status.set_text("歌单名称不能为空。");
                } else if ui.lib.borrow().playlists.len() >= 100 {
                    ui.status.set_text("最多创建 100 个歌单。");
                } else {
                    ui.lib
                        .borrow_mut()
                        .playlists
                        .entry(name.clone())
                        .or_default();
                    ui.rebuild_playlists();
                    ui.schedule_save();
                    ui.navigate(Route::Playlist(name));
                }
            }
            d.close();
        });
        dialog.present();
    }
    fn choose_playlist(self: &Rc<Self>) {
        let Some(track) = self.selected() else {
            self.status.set_text("请先选中一首曲目。");
            return;
        };
        let names: Vec<_> = self.lib.borrow().playlists.keys().cloned().collect();
        if names.is_empty() {
            self.status.set_text("请先点击侧栏 + 创建歌单。");
            self.new_playlist();
            return;
        }
        let dialog = gtk::Dialog::builder()
            .title("加入歌单")
            .transient_for(&self.window)
            .modal(true)
            .build();
        dialog.add_button("取消", gtk::ResponseType::Cancel);
        dialog.add_button("添加", gtk::ResponseType::Accept);
        let strings: Vec<_> = names.iter().map(String::as_str).collect();
        let choice = gtk::DropDown::from_strings(&strings);
        margins(&choice, 20);
        dialog.content_area().append(&choice);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d, r| {
            if r == gtk::ResponseType::Accept
                && let (Some(ui), Some(name)) =
                    (weak.upgrade(), names.get(choice.selected() as usize))
            {
                let mut lib = ui.lib.borrow_mut();
                let tracks = lib.playlists.entry(name.clone()).or_default();
                if !tracks.iter().any(|t| t.id == track.id) && tracks.len() < 3000 {
                    tracks.push(track.clone());
                }
                drop(lib);
                ui.schedule_save();
                ui.status.set_text(&format!("已加入歌单「{name}」。"));
            }
            d.close();
        });
        dialog.present();
    }
    fn close_settings(&self) {
        let settings = self.settings_window.borrow_mut().take();
        if let Some(settings) = settings {
            settings.destroy();
        }
    }
    fn settings(self: &Rc<Self>) {
        if let Some(window) = self.settings_window.borrow().as_ref() {
            window.present();
            return;
        }
        let dialog = gtk::Window::builder()
            .application(
                self.window
                    .application()
                    .as_ref()
                    .expect("application window"),
            )
            .title("设置与备份")
            .transient_for(&self.window)
            .destroy_with_parent(true)
            .modal(false)
            .default_width(550)
            .resizable(false)
            .build();
        let header = gtk::HeaderBar::new();
        header.set_decoration_layout(Some(":close"));
        header.set_title_widget(Some(&label("设置与备份", "window-title")));
        dialog.set_titlebar(Some(&header));
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                if let Some(ui) = weak.upgrade() {
                    ui.close_settings();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dialog.add_controller(keys);
        let area = gtk::Box::new(gtk::Orientation::Vertical, 15);
        dialog.set_child(Some(&area));
        area.set_spacing(15);
        margins(&area, 24);
        area.append(&label("Buisc", "title-2"));
        let text = label(
            &format!(
                "Rust + GTK4 + GStreamer · v{}\n\n按需解析音频，不加载视频，不缓存音频文件。\n暂停时停止进度更新，空闲时不轮询网络。\n支持游客与扫码登录，普通 AAC；多 P 视频目前播放第一 P。\n\n快捷键：Ctrl+空格 播放/暂停 · Ctrl+←/→ 切歌\nCtrl+F 搜索 · 系统媒体面板通过 MPRIS 控制\n\n资料库：{}",
                env!("CARGO_PKG_VERSION"),
                self.path.display()
            ),
            "muted",
        );
        text.set_wrap(true);
        text.set_selectable(true);
        area.append(&text);
        let renderer = self
            .window
            .renderer()
            .map(|renderer| {
                let kind = renderer.type_();
                let name = kind.name().to_ascii_lowercase();
                if name.contains("cairo") {
                    "Cairo（CPU）"
                } else if name.contains("vulkan") {
                    "Vulkan"
                } else if name.contains("glrenderer") {
                    "OpenGL"
                } else {
                    kind.name()
                }
            })
            .unwrap_or("尚未初始化");
        let graphics_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let graphics_label = label("界面渲染", "");
        graphics_label.set_hexpand(true);
        graphics_row.append(&graphics_label);
        let graphics = gtk::DropDown::from_strings(&["自动（优先 GPU）", "兼容模式（CPU）"]);
        graphics.set_selected(u32::from(crate::graphics::cpu_only()));
        graphics_row.append(&graphics);
        area.append(&graphics_row);
        let graphics_status = label(
            &format!("当前：{renderer} · 更改后需完全退出并重开"),
            "muted",
        );
        graphics_status.set_wrap(true);
        area.append(&graphics_status);
        graphics.connect_selected_notify(move |choice| {
            match crate::graphics::save(choice.selected() == 1) {
                Ok(()) => graphics_status.set_text("已保存，完全退出并重新打开后生效。"),
                Err(error) => graphics_status.set_text(&format!("无法保存渲染设置：{error}")),
            }
        });
        let close_to_tray = gtk::CheckButton::with_label("关闭窗口后在托盘后台运行");
        close_to_tray.set_active(self.lib.borrow().close_to_tray);
        close_to_tray.set_tooltip_text(Some(
            "保留播放，可从托盘重新打开。桌面不支持系统托盘时，关闭窗口仍会退出。",
        ));
        area.append(&close_to_tray);
        {
            let weak = Rc::downgrade(self);
            close_to_tray.connect_toggled(move |button| {
                if let Some(ui) = weak.upgrade() {
                    ui.set_close_to_tray(button.is_active());
                }
            });
        }
        let sync = gtk::CheckButton::with_label("登录后同步播放进度到 B站历史");
        sync.set_active(self.lib.borrow().sync_history);
        area.append(&sync);
        {
            let weak = Rc::downgrade(self);
            sync.connect_toggled(move |b| {
                if let Some(ui) = weak.upgrade() {
                    ui.lib.borrow_mut().sync_history = b.is_active();
                    ui.schedule_save();
                }
            });
        }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let export = button("导出资料库", "primary");
        let import = button("导入并替换资料库", "");
        row.append(&export);
        row.append(&import);
        let quit = button("退出 Buisc", "");
        row.append(&quit);
        {
            let weak = Rc::downgrade(self);
            quit.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.quit();
                }
            });
        }
        area.append(&row);
        {
            let weak = Rc::downgrade(self);
            export.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.backup(false);
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            import.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.backup(true);
                }
            });
        }
        let close = button("关闭", "");
        close.set_halign(gtk::Align::End);
        close.set_widget_name("settings-close");
        {
            let weak = Rc::downgrade(self);
            close.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.close_settings();
                }
            });
        }
        area.append(&close);
        {
            let weak = Rc::downgrade(self);
            dialog.connect_close_request(move |window| {
                if let Some(ui) = weak.upgrade() {
                    let current = ui.settings_window.borrow().as_ref() == Some(window);
                    if current {
                        ui.settings_window.borrow_mut().take();
                    }
                }
                glib::Propagation::Proceed
            });
        }
        *self.settings_window.borrow_mut() = Some(dialog.clone());
        dialog.present();
    }
    fn backup(self: &Rc<Self>, import: bool) {
        let chooser = gtk::FileChooserNative::new(
            Some(if import {
                "导入备份（将替换当前资料库）"
            } else {
                "导出资料库"
            }),
            Some(&self.window),
            if import {
                gtk::FileChooserAction::Open
            } else {
                gtk::FileChooserAction::Save
            },
            Some(if import { "导入并替换" } else { "导出" }),
            Some("取消"),
        );
        if !import {
            chooser.set_current_name("bili-music-backup.json");
        }
        let filter = gtk::FileFilter::new();
        filter.add_pattern("*.json");
        filter.set_name(Some("JSON 资料库"));
        chooser.add_filter(&filter);
        let weak = Rc::downgrade(self);
        chooser.connect_response(move |d, r| {
            if r == gtk::ResponseType::Accept
                && let (Some(ui), Some(path)) = (weak.upgrade(), d.file().and_then(|f| f.path()))
            {
                if import {
                    match storage::load(&path) {
                        Ok(lib) => {
                            ui.stop();
                            *ui.current.borrow_mut() = None;
                            ui.reset_cover();
                            *ui.lib.borrow_mut() = lib;
                            ui.mode.set_label(ui.lib.borrow().mode.label());
                            let imported_volume = ui.lib.borrow().volume;
                            ui.volume_scale.set_value(imported_volume * 100.);
                            if let Some(a) = ui.audio.borrow().as_ref() {
                                a.set_volume(ui.lib.borrow().volume);
                            }
                            ui.rebuild_playlists();
                            let close_to_tray = ui.lib.borrow().close_to_tray;
                            ui.configure_tray(close_to_tray);
                            ui.flush_save();
                            ui.navigate(Route::Favorites);
                            ui.now_title.set_text("还没有开始播放");
                            ui.now_artist.set_text("资料库已导入");
                            ui.update_favorite();
                        }
                        Err(e) => ui.status.set_text(&e),
                    }
                } else {
                    match storage::save(&path, &ui.lib.borrow()) {
                        Ok(()) => ui.status.set_text("资料库已导出。"),
                        Err(e) => ui.status.set_text(&e),
                    }
                }
            }
            d.destroy();
        });
        chooser.show();
    }
    pub fn playback_status(&self) -> &'static str {
        if self.playing() {
            "Playing"
        } else if self.stopped.get() || self.current.borrow().is_none() {
            "Stopped"
        } else {
            "Paused"
        }
    }
    pub fn playing(&self) -> bool {
        self.audio.borrow().as_ref().is_some_and(|a| a.is_playing())
    }
    pub fn position(&self) -> u64 {
        self.audio.borrow().as_ref().map_or(0, |a| a.position())
    }
    pub fn current_track(&self) -> Option<Track> {
        self.current.borrow().clone()
    }
    pub fn volume(&self) -> f64 {
        self.lib.borrow().volume
    }
    pub fn set_volume(self: &Rc<Self>, v: f64) {
        if v.is_finite() {
            let v = v.clamp(0., 1.);
            self.lib.borrow_mut().volume = v;
            self.volume_scale.set_value(v * 100.);
            if let Some(a) = self.audio.borrow().as_ref() {
                a.set_volume(v);
            }
            self.schedule_save();
        }
    }
    pub fn seek_to(&self, seconds: u64) {
        if let Some(a) = self.audio.borrow().as_ref()
            && let Err(error) = a.seek(seconds)
        {
            self.status.set_text(&error);
        }
    }
    pub fn notify_mpris(&self) {
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.changed(self);
        }
        if let Some(m) = self.mpris.borrow().as_ref() {
            m.changed(self);
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn wait_until(timeout: u64, test: impl Fn() -> bool) {
        let context = glib::MainContext::default();
        let start = Instant::now();
        while !test() {
            while context.pending() {
                context.iteration(false);
            }
            assert!(
                start.elapsed() < Duration::from_secs(timeout),
                "integration test timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn pump(seconds: u64) {
        let event_loop = glib::MainLoop::new(None, false);
        let quit_loop = event_loop.clone();
        glib::timeout_add_local_once(Duration::from_secs(seconds), move || quit_loop.quit());
        event_loop.run();
    }
    fn snapshot(ui: &Ui, path: &str) {
        let paintable = gtk::WidgetPaintable::new(Some(&ui.window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(
            &snapshot,
            ui.window.width() as f64,
            ui.window.height() as f64,
        );
        let node = snapshot.to_node().expect("render node");
        let renderer = ui.window.renderer().expect("renderer");
        renderer
            .render_texture(&node, None)
            .save_to_png(path)
            .expect("screenshot");
    }
    fn stats(stage: &str) {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let rss = status
            .lines()
            .find(|l| l.starts_with("VmRSS:"))
            .unwrap_or("");
        let pss = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
        let pss = pss.lines().find(|l| l.starts_with("Pss:")).unwrap_or("");
        let cpu_before = cpu_ticks();
        let start = Instant::now();
        pump(5);
        let secs = start.elapsed().as_secs_f64();
        let ticks = cpu_ticks().saturating_sub(cpu_before);
        println!(
            "RESOURCE {stage}: {rss}, {pss}, CPU(single core) {:.2}%",
            ticks as f64 / secs
        );
    }
    fn cpu_ticks() -> u64 {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .map(|(_, tail)| tail.split_whitespace().collect())
            .unwrap_or_default();
        fields
            .get(11)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
            + fields
                .get(12)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
    }
    fn find_widget(
        root: &gtk::Widget,
        predicate: &impl Fn(&gtk::Widget) -> bool,
    ) -> Option<gtk::Widget> {
        if predicate(root) {
            return Some(root.clone());
        }
        let mut child = root.first_child();
        while let Some(widget) = child {
            if let Some(found) = find_widget(&widget, predicate) {
                return Some(found);
            }
            child = widget.next_sibling();
        }
        None
    }
    fn click_window_close(window: &gtk::Window) {
        let close = find_widget(window.upcast_ref(), &|widget| {
            widget.is::<gtk::Button>() && widget.has_css_class("close")
        })
        .and_downcast::<gtk::Button>()
        .expect("window close button");
        close.emit_clicked();
    }

    fn dbus_call(
        connection: &gio::DBusConnection,
        destination: &str,
        path: &str,
        interface: &str,
        method: &str,
        parameters: Option<&glib::Variant>,
    ) -> glib::Variant {
        glib::MainContext::default()
            .block_on(connection.call_future(
                Some(destination),
                path,
                interface,
                method,
                parameters,
                None,
                gio::DBusCallFlags::NONE,
                5000,
            ))
            .expect("D-Bus tray call")
    }

    #[test]
    #[ignore = "requires desktop with a real StatusNotifier host and isolated BILI_MUSIC_TEST data; no network"]
    fn native_tray_lifecycle() {
        assert_eq!(std::env::var("BILI_MUSIC_TEST").as_deref(), Ok("1"));
        assert!(
            std::env::var("XDG_DATA_HOME")
                .unwrap_or_default()
                .contains("bili-test-")
        );
        gtk::init().unwrap();
        gstreamer::init().unwrap();
        let app = gtk::Application::builder()
            .application_id("io.github.bilimusic.Test")
            .build();
        app.register(gio::Cancellable::NONE).unwrap();
        let connection = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).unwrap();
        let ui = Ui::new(&app).unwrap();
        ui.window.present();
        pump(1);
        // Slow cover/list requests must not discard the latest playback request.
        ui.active_requests.set(3);
        ui.stream_inflight.set(true);
        ui.play_track(Track {
            id: "BV1xx411c7mD".into(),
            bvid: "BV1xx411c7mD".into(),
            title: "待播放 A".into(),
            ..Default::default()
        });
        ui.play_track(Track {
            id: "BV1BDk2YCEHF".into(),
            bvid: "BV1BDk2YCEHF".into(),
            title: "待播放 B".into(),
            ..Default::default()
        });
        assert_eq!(
            ui.pending_stream.borrow().as_ref().unwrap().1.title,
            "待播放 B"
        );
        ui.pause();
        assert!(ui.pending_stream.borrow().is_none());
        ui.stream_inflight.set(false);
        ui.active_requests.set(0);
        ui.lib.borrow_mut().queue.clear();
        // Appending results must preserve existing rows and the selection.
        let first = Track {
            id: "row1".into(),
            title: "第一页".into(),
            ..Default::default()
        };
        let second = Track {
            id: "row2".into(),
            title: "第二页".into(),
            ..Default::default()
        };
        ui.set_tracks(vec![first.clone()]);
        let original = ui.store.item(0).unwrap();
        ui.selection.set_selected(0);
        ui.set_tracks(vec![first, second]);
        assert_eq!(ui.store.item(0).as_ref(), Some(&original));
        assert_eq!(ui.selection.selected(), 0);
        *ui.route.borrow_mut() = Route::Search;
        ui.has_more.set(true);
        ui.loading.set(true);
        let page = ui.page.get();
        let list_scroll = find_widget(ui.window.upcast_ref(), &|widget| {
            widget
                .downcast_ref::<gtk::ScrolledWindow>()
                .is_some_and(|scroll| {
                    scroll
                        .child()
                        .is_some_and(|child| child.is::<gtk::ColumnView>())
                })
        })
        .and_downcast::<gtk::ScrolledWindow>()
        .unwrap();
        list_scroll.emit_by_name::<()>("edge-reached", &[&gtk::PositionType::Bottom]);
        assert_eq!(ui.page.get(), page, "loading must block duplicate pages");
        ui.loading.set(false);
        ui.has_more.set(false);
        list_scroll.emit_by_name::<()>("edge-reached", &[&gtk::PositionType::Bottom]);
        assert_eq!(ui.page.get(), page, "exhausted pages must not load");
        *ui.route.borrow_mut() = Route::Discover;
        ui.set_tracks(vec![]);
        assert!(!ui.lib.borrow().close_to_tray);
        ui.settings();
        let settings = ui.settings_window.borrow().clone().unwrap();
        assert!(
            !settings.is_modal(),
            "settings must not block the main window controls"
        );
        let checkbox = find_widget(settings.upcast_ref(), &|widget| {
            widget
                .downcast_ref::<gtk::CheckButton>()
                .is_some_and(|button| button.label().as_deref() == Some("关闭窗口后在托盘后台运行"))
        })
        .and_downcast::<gtk::CheckButton>()
        .unwrap();
        checkbox.set_active(true);
        wait_until(10, || {
            ui.tray
                .borrow()
                .as_ref()
                .is_some_and(|tray| tray.is_available())
        });
        ui.flush_save();
        assert!(storage::load(&ui.path).unwrap().close_to_tray);
        // Real, silent GStreamer playback verifies that close-to-tray does not
        // stop the audio pipeline. The fixture stays in the isolated test data.
        let wav_path = ui.path.with_file_name("tray-test-silence.wav");
        let sample_rate = 8000u32;
        let data_size = sample_rate * 2 * 30;
        let mut wav = Vec::with_capacity((44 + data_size) as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.resize((44 + data_size) as usize, 0);
        std::fs::write(&wav_path, wav).unwrap();
        *ui.current.borrow_mut() = Some(Track {
            id: "BV1BDk2YCEHF".into(),
            bvid: "BV1BDk2YCEHF".into(),
            title: "托盘静音验证".into(),
            ..Default::default()
        });
        let cover = include_bytes!("../tests/fixtures/cover.png").to_vec();
        let generation = ui.play_generation.get();
        ui.handle(Event::Cover(generation, Ok(cover.clone())));
        assert_eq!(
            ui.cover_stack.visible_child_name().as_deref(),
            Some("cover")
        );
        assert!(ui.cover_picture.paintable().is_some());
        ui.reset_cover();
        ui.handle(Event::Cover(generation + 1, Ok(cover)));
        assert_eq!(
            ui.cover_stack.visible_child_name().as_deref(),
            Some("fallback")
        );
        assert!(ui.cover_picture.paintable().is_none());
        ui.handle(Event::Cover(generation, Ok(b"invalid image".to_vec())));
        assert_eq!(
            ui.cover_stack.visible_child_name().as_deref(),
            Some("fallback")
        );
        ui.stopped.set(false);
        ui.audio.borrow().as_ref().unwrap().set_volume(0.);
        ui.audio
            .borrow()
            .as_ref()
            .unwrap()
            .load_test_file(&wav_path)
            .unwrap();
        wait_until(5, || ui.playing());

        let registered = dbus_call(
            &connection,
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.freedesktop.DBus.Properties",
            "Get",
            Some(
                &(
                    "org.kde.StatusNotifierWatcher",
                    "RegisteredStatusNotifierItems",
                )
                    .to_variant(),
            ),
        );
        let (items,) = registered.get::<(glib::Variant,)>().unwrap();
        let items = items.get::<Vec<String>>().unwrap();
        let unique = connection.unique_name().unwrap();
        assert!(
            items
                .iter()
                .any(|item| item.starts_with(&format!("{unique}/"))
                    || item.starts_with(&format!(
                        "org.kde.StatusNotifierItem-{}-",
                        std::process::id()
                    ))),
            "tray was not in watcher list: {items:?}"
        );
        let destination = unique.as_str();
        let path = "/StatusNotifierItem";
        let layout = dbus_call(
            &connection,
            destination,
            "/MenuBar",
            "com.canonical.dbusmenu",
            "GetLayout",
            Some(&(0i32, -1i32, Vec::<String>::new()).to_variant()),
        );
        assert_eq!(layout.type_().as_str(), "(u(ia{sv}av))");
        assert!(layout.print(false).contains("退出 Buisc"));
        let menu_label = dbus_call(
            &connection,
            destination,
            "/MenuBar",
            "com.canonical.dbusmenu",
            "GetProperty",
            Some(&(1i32, "label").to_variant()),
        );
        let (menu_label,) = menu_label.get::<(glib::Variant,)>().unwrap();
        assert_eq!(menu_label.type_().as_str(), "s");

        let settings_close = find_widget(settings.upcast_ref(), &|widget| {
            widget.widget_name() == "settings-close"
        })
        .and_downcast::<gtk::Button>()
        .unwrap();
        settings_close.emit_clicked();
        pump(1);
        assert!(!settings.is_visible());
        assert!(ui.settings_window.borrow().is_none());
        ui.settings();
        let settings = ui.settings_window.borrow().clone().unwrap();
        pump(1);
        click_window_close(&settings);
        pump(1);
        assert!(!settings.is_visible());
        assert!(ui.settings_window.borrow().is_none());
        ui.settings();
        let settings = ui.settings_window.borrow().clone().unwrap();
        ui.settings();
        assert_eq!(ui.settings_window.borrow().as_ref(), Some(&settings));
        pump(1);
        click_window_close(ui.window.upcast_ref());
        pump(1);
        assert!(!settings.is_visible());
        assert!(ui.settings_window.borrow().is_none());
        assert!(!ui.window.is_visible());
        assert!(!ui.quitting.get());
        assert!(ui.background_hold.borrow().is_some());
        assert!(ui.playing(), "closing to tray stopped real audio");
        dbus_call(
            &connection,
            destination,
            "/MenuBar",
            "com.canonical.dbusmenu",
            "Event",
            Some(&(2i32, "clicked", 0i32.to_variant(), 0u32).to_variant()),
        );
        wait_until(5, || !ui.playing());
        assert!(!ui.window.is_visible());
        dbus_call(
            &connection,
            destination,
            "/MenuBar",
            "com.canonical.dbusmenu",
            "Event",
            Some(&(2i32, "clicked", 0i32.to_variant(), 0u32).to_variant()),
        );
        wait_until(5, || ui.playing());
        assert!(!ui.window.is_visible());
        dbus_call(
            &connection,
            destination,
            path,
            "org.kde.StatusNotifierItem",
            "Activate",
            Some(&(0i32, 0i32).to_variant()),
        );
        assert!(ui.window.is_visible());
        assert!(ui.background_hold.borrow().is_none());
        ui.window.close();
        pump(1);
        assert!(crate::present_existing_window(&app));
        assert!(ui.window.is_visible());
        assert_eq!(
            app.windows()
                .into_iter()
                .filter(|window| window.is::<gtk::ApplicationWindow>())
                .count(),
            1
        );

        ui.window.close();
        pump(1);
        ui.set_close_to_tray(false);
        assert!(ui.window.is_visible());
        assert!(ui.tray.borrow().is_none());
        assert!(ui.background_hold.borrow().is_none());
        wait_until(5, || {
            let registered = dbus_call(
                &connection,
                "org.kde.StatusNotifierWatcher",
                "/StatusNotifierWatcher",
                "org.freedesktop.DBus.Properties",
                "Get",
                Some(
                    &(
                        "org.kde.StatusNotifierWatcher",
                        "RegisteredStatusNotifierItems",
                    )
                        .to_variant(),
                ),
            );
            let (items,) = registered.get::<(glib::Variant,)>().unwrap();
            !items.get::<Vec<String>>().unwrap().iter().any(|item| {
                item.starts_with(&format!("{unique}/"))
                    || item.starts_with(&format!(
                        "org.kde.StatusNotifierItem-{}-",
                        std::process::id()
                    ))
            })
        });
        ui.set_close_to_tray(true);
        wait_until(10, || {
            ui.tray
                .borrow()
                .as_ref()
                .is_some_and(|tray| tray.is_available())
        });
        ui.window.close();
        pump(1);
        dbus_call(
            &connection,
            destination,
            "/MenuBar",
            "com.canonical.dbusmenu",
            "Event",
            Some(&(6i32, "clicked", 0i32.to_variant(), 0u32).to_variant()),
        );
        assert!(ui.quitting.get());
        assert!(ui.background_hold.borrow().is_none());
        assert!(ui.tray.borrow().is_none());
        drop(ui);
        pump(1);

        // A missing tray never leaves an inaccessible background process.
        let fallback = Ui::new(&app).unwrap();
        fallback.window.present();
        fallback.lib.borrow_mut().close_to_tray = true;
        assert!(fallback.tray.borrow().is_none());
        fallback.window.close();
        pump(1);
        assert!(fallback.quitting.get());
        assert!(!fallback.window.is_visible());
        let settings_exit = Ui::new(&app).unwrap();
        settings_exit.window.present();
        settings_exit.settings();
        let settings = settings_exit.settings_window.borrow().clone().unwrap();
        let exit = find_widget(settings.upcast_ref(), &|widget| {
            widget
                .downcast_ref::<gtk::Button>()
                .is_some_and(|button| button.label().as_deref() == Some("退出 Buisc"))
        })
        .and_downcast::<gtk::Button>()
        .unwrap();
        exit.emit_clicked();
        pump(1);
        assert!(settings_exit.quitting.get());
        assert!(!settings.is_visible());
        println!(
            "tray/settings checkbox/settings close button/window close with settings open/audio continues/pause-resume/restore/disable/quit buttons/no-tray fallback: passed"
        );
    }

    #[test]
    #[ignore = "desktop-only visual preview; uses isolated data and one chart request"]
    fn native_layout_preview() {
        assert_eq!(std::env::var("BILI_MUSIC_TEST").as_deref(), Ok("1"));
        assert!(
            std::env::var("XDG_DATA_HOME")
                .unwrap_or_default()
                .contains("bili-test-")
        );
        gtk::init().unwrap();
        gstreamer::init().unwrap();
        let app = gtk::Application::builder()
            .application_id("io.github.bilimusic.Test")
            .build();
        app.register(gio::Cancellable::NONE).unwrap();
        let ui = Ui::new(&app).unwrap();
        ui.clone().start();
        wait_until(30, || !ui.loading.get());
        pump(4);
        snapshot(
            &ui,
            &std::env::var("BILI_MUSIC_SCREENSHOT").expect("screenshot output path"),
        );
        assert_eq!(ui.heading.text(), "音乐热榜");
        println!(
            "GTK renderer: {}",
            ui.window.renderer().unwrap().type_().name()
        );
        assert!(ui.window.width() >= 860);
        assert!(!ui.window.titlebar().unwrap().is_visible());
        let main = find_widget(ui.window.upcast_ref(), &|widget| {
            widget.widget_name() == "main-pane"
        })
        .unwrap();
        let player = find_widget(ui.window.upcast_ref(), &|widget| {
            widget.widget_name() == "player-bar"
        })
        .unwrap();
        let pane = player.parent().and_downcast::<glass::GlassPane>().unwrap();
        assert_eq!(player.valign(), gtk::Align::End);
        assert_eq!(player.halign(), gtk::Align::Center);
        assert_eq!(pane.first_child(), main.parent());
        assert!(player.width() <= main.width());
        let controls = find_widget(ui.window.upcast_ref(), &|widget| {
            widget.is::<gtk::WindowControls>()
        })
        .unwrap();
        assert!(controls.is_mapped());
        assert!(ui.discovery.scroll.is_visible());
        assert!(!ui.list_panel.is_visible());
        if !ui.shown.borrow().is_empty() {
            let full_chart = find_widget(ui.window.upcast_ref(), &|widget| {
                widget
                    .downcast_ref::<gtk::Button>()
                    .is_some_and(|button| button.label().as_deref() == Some("完整榜单 ›"))
            })
            .and_downcast::<gtk::Button>()
            .unwrap();
            full_chart.emit_clicked();
            assert!(ui.list_panel.is_visible());
            assert!(!ui.discovery.scroll.is_visible());
        }
        ui.window.close();
        pump(1);
    }
    #[test]
    #[ignore = "requires a desktop session, network, and BILI_MUSIC_TEST=1 with isolated XDG_DATA_HOME"]
    fn live_native_player() {
        assert_eq!(std::env::var("BILI_MUSIC_TEST").as_deref(), Ok("1"));
        assert!(
            std::env::var("XDG_DATA_HOME")
                .unwrap_or_default()
                .contains("bili-test-")
        );
        gtk::init().expect("GTK");
        gstreamer::init().unwrap();
        let app = gtk::Application::builder()
            .application_id("io.github.bilimusic.Test")
            .build();
        app.register(gio::Cancellable::NONE).unwrap();
        let ui = Ui::new(&app).unwrap();
        ui.clone().start();
        wait_until(40, || !ui.loading.get());
        assert!(!ui.shown.borrow().is_empty(), "{}", ui.status.text());
        pump(1);
        if let Ok(path) = std::env::var("BILI_MUSIC_SCREENSHOT") {
            snapshot(&ui, &path);
        }
        println!(
            "INITIAL history IDs: {:?}",
            ui.lib
                .borrow()
                .history
                .iter()
                .map(|t| t.bvid.as_str())
                .collect::<Vec<_>>()
        );
        stats("idle");
        ui.search.set_text("纯音乐");
        ui.run_search(false);
        wait_until(40, || !ui.loading.get());
        assert!(!ui.shown.borrow().is_empty(), "{}", ui.status.text());
        ui.search
            .set_text("https://www.bilibili.com/video/BV1BDk2YCEHF");
        ui.run_search(false);
        wait_until(40, || !ui.loading.get());
        assert_eq!(ui.shown.borrow().len(), 1, "{}", ui.status.text());
        ui.selection.set_selected(0);
        let track = ui.selected().unwrap();
        ui.favorite(&track);
        assert_eq!(ui.lib.borrow().favorites.len(), 1);
        ui.lib
            .borrow_mut()
            .playlists
            .insert("测试歌单".into(), vec![track.clone()]);
        ui.rebuild_playlists();
        ui.navigate(Route::Playlist("测试歌单".into()));
        assert_eq!(ui.store.n_items(), 1);
        ui.set_volume(0.);
        ui.play_track(track.clone());
        wait_until(40, || ui.position() >= 2);
        assert!(ui.playing(), "{}", ui.status.text());
        assert_eq!(ui.current_track().unwrap().id, track.id);
        wait_until(15, || {
            ui.cover_stack.visible_child_name().as_deref() == Some("cover")
        });
        assert!(ui.cover_picture.paintable().is_some());
        if let Ok(path) = std::env::var("BILI_MUSIC_SCREENSHOT") {
            pump(1);
            snapshot(&ui, &path);
        }
        stats("playing");
        println!(
            "SEEK before: position={} status={}",
            ui.position(),
            ui.status.text()
        );
        ui.seek_to(30);
        pump(5);
        println!(
            "SEEK after: position={} duration={} playing={} status={}",
            ui.position(),
            ui.audio.borrow().as_ref().unwrap().duration(),
            ui.playing(),
            ui.status.text()
        );
        wait_until(15, || ui.position() >= 30);
        ui.pause();
        wait_until(5, || !ui.playing());
        pump(1);
        assert!(ui.timer.borrow().is_none());
        stats("paused");
        ui.play();
        wait_until(5, || ui.playing());
        ui.stop();
        pump(1);
        assert_eq!(ui.playback_status(), "Stopped");
        let backup = ui.path.with_file_name("backup.json");
        storage::save(&backup, &ui.lib.borrow()).unwrap();
        assert_eq!(storage::load(&backup).unwrap().playlists.len(), 1);
        ui.flush_save();
        println!(
            "FINAL history IDs: {:?}; queue IDs: {:?}",
            ui.lib
                .borrow()
                .history
                .iter()
                .map(|t| t.bvid.as_str())
                .collect::<Vec<_>>(),
            ui.lib
                .borrow()
                .queue
                .iter()
                .map(|t| t.bvid.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(storage::load(&ui.path).unwrap().history.len(), 1);
        ui.window.close();
        pump(1);
    }
}
