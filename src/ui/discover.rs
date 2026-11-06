use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct Discovery {
    pub(super) scroll: gtk::ScrolledWindow,
    body: gtk::Box,
    cancel: RefCell<Option<Arc<AtomicBool>>>,
}

impl Discovery {
    pub(super) fn new() -> Self {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 22);
        body.set_margin_top(6);
        body.set_margin_bottom(88);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&body)
            .build();
        smooth_scroll::attach(&scroll);
        Self {
            scroll,
            body,
            cancel: RefCell::new(None),
        }
    }

    fn cancel_images(&self) {
        if let Some(cancel) = self.cancel.borrow_mut().take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    fn clear(&self) {
        self.cancel_images();
        while let Some(child) = self.body.first_child() {
            self.body.remove(&child);
        }
    }

    pub(super) fn loading(&self) {
        self.clear();
        self.body
            .append(&label("正在获取 B 站音乐区热榜…", "muted"));
    }

    pub(super) fn render(&self, ui: &Rc<Ui>, tracks: &[Track]) {
        self.clear();
        self.scroll.vadjustment().set_value(0.);
        if tracks.is_empty() {
            self.body.append(&label(
                "暂时无法加载热榜。可重新点击侧栏「音乐热榜」刷新。",
                "muted",
            ));
            return;
        }
        let mut images = Vec::new();
        let highlights = grid();
        for (i, track) in tracks.iter().take(3).enumerate() {
            let (art, picture) = artwork(154, true);
            let copy = gtk::Box::new(gtk::Orientation::Vertical, 4);
            copy.append(&label(&format!("热榜 · 第 {} 名", i + 1), "page-kicker"));
            copy.append(&line(&track.title, "feature-title"));
            copy.append(&line(&track.artist, "muted"));
            art.set_margin_top(6);
            copy.append(&art);
            highlights.attach(&track_button(ui, track, &copy), i as i32, 0, 1, 1);
            images.push((track.cover.clone(), true, picture));
        }
        self.body.append(&highlights);

        let section = gtk::Box::new(gtk::Orientation::Vertical, 9);
        section.append(&section_heading(ui, "热榜精选"));
        let rows = grid();
        rows.set_row_spacing(0);
        for (i, track) in tracks.iter().skip(3).take(12).enumerate() {
            let (art, picture) = artwork(35, false);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("discovery-track");
            row.append(&art);
            let copy = gtk::Box::new(gtk::Orientation::Vertical, 2);
            copy.set_hexpand(true);
            copy.set_valign(gtk::Align::Center);
            copy.append(&line(&track.title, "song-title"));
            copy.append(&line(&track.artist, "muted"));
            row.append(&copy);
            rows.attach(
                &track_button(ui, track, &row),
                (i / 4) as i32,
                (i % 4) as i32,
                1,
                1,
            );
            images.push((track.cover.clone(), false, picture));
        }
        section.append(&rows);
        self.body.append(&section);

        if tracks.len() > 15 {
            let section = gtk::Box::new(gtk::Orientation::Vertical, 9);
            section.append(&section_heading(ui, "继续发现"));
            let gallery = grid();
            for (i, track) in tracks.iter().skip(15).take(6).enumerate() {
                let (art, picture) = artwork(88, true);
                let copy = gtk::Box::new(gtk::Orientation::Vertical, 4);
                copy.append(&art);
                copy.append(&line(&track.title, "song-title"));
                copy.append(&line(&track.artist, "muted"));
                gallery.attach(&track_button(ui, track, &copy), i as i32, 0, 1, 1);
                images.push((track.cover.clone(), true, picture));
            }
            section.append(&gallery);
            self.body.append(&section);
        }

        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel.borrow_mut() = Some(cancel.clone());
        let requests: Vec<_> = images
            .iter()
            .map(|(url, wide, _)| (url.clone(), *wide))
            .collect();
        let (tx, rx) = async_channel::bounded(2);
        let api = ui.api.clone();
        let worker_cancel = cancel.clone();
        std::thread::spawn(move || {
            for (i, (url, wide)) in requests.into_iter().enumerate() {
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                if !url.is_empty()
                    && let Ok(bytes) = api.cover_image(&url, wide)
                    && tx.send_blocking((i, bytes)).is_err()
                {
                    break;
                }
            }
        });
        glib::spawn_future_local(async move {
            while let Ok((i, bytes)) = rx.recv().await {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let (_, wide, picture) = &images[i];
                let loader = gtk::gdk_pixbuf::PixbufLoader::new();
                let size = if *wide { (480, 270) } else { (70, 70) };
                loader.set_size(size.0, size.1);
                let written = loader.write(&bytes).is_ok();
                if loader.close().is_ok()
                    && written
                    && let Some(pixbuf) = loader.pixbuf()
                {
                    picture.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&pixbuf)));
                }
            }
        });
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.get_mut().take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Ui {
    pub(super) fn show_discovery(&self, show: bool) {
        if !show {
            self.discovery.cancel_images();
        }
        self.discovery.scroll.set_visible(show);
        self.list_panel.set_visible(!show);
        self.subtitle.set_visible(!show);
    }
}

fn grid() -> gtk::Grid {
    gtk::Grid::builder()
        .column_spacing(16)
        .column_homogeneous(true)
        .build()
}

fn line(text: &str, class: &str) -> gtk::Label {
    let l = label(text, class);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_width_chars(1);
    l.set_max_width_chars(24);
    l
}

fn artwork(size: i32, wide: bool) -> (gtk::Overlay, gtk::Picture) {
    let art = gtk::Overlay::new();
    art.add_css_class("discovery-art");
    art.set_overflow(gtk::Overflow::Hidden);
    art.set_size_request(if wide { 1 } else { size }, size);
    let fallback = icons::artwork(if wide { 36 } else { 22 });
    if wide {
        // React only to allocation changes; there is no animation or polling.
        let sizing = gtk::DrawingArea::new();
        let weak = art.downgrade();
        let featured = size > 100;
        sizing.connect_resize(move |_, width, _| {
            let height = if featured {
                (width * 9 / 16).clamp(96, 320)
            } else {
                width.clamp(68, 180)
            };
            if weak
                .upgrade()
                .is_some_and(|art| art.height_request() != height)
            {
                let weak = weak.clone();
                glib::idle_add_local_once(move || {
                    if let Some(art) = weak.upgrade() {
                        art.set_height_request(height);
                    }
                });
            }
        });
        art.set_child(Some(&sizing));
        art.add_overlay(&fallback);
    } else {
        art.set_child(Some(&fallback));
    }
    let image = gtk::Picture::new();
    image.set_can_shrink(true);
    image.set_content_fit(gtk::ContentFit::Cover);
    art.add_overlay(&image);
    (art, image)
}

fn track_button(ui: &Rc<Ui>, track: &Track, child: &impl IsA<gtk::Widget>) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("discovery-button");
    button.set_child(Some(child));
    button.set_tooltip_text(Some(&format!("播放 · {}", track.title)));
    let track = track.clone();
    let weak = Rc::downgrade(ui);
    button.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.play_track(track.clone());
        }
    });
    button
}

fn section_heading(ui: &Rc<Ui>, title: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let heading = label(title, "discovery-section");
    heading.set_hexpand(true);
    row.append(&heading);
    let button = button("完整榜单 ›", "small");
    let weak = Rc::downgrade(ui);
    button.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.show_discovery(false);
        }
    });
    row.append(&button);
    row
}
