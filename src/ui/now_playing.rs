use super::*;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::Duration;

use gtk::pango::EllipsizeMode;
use gtk::{Justification, WrapMode, gdk, graphene, gsk, subclass::prelude::*};

const BACKDROP_BASE: gdk::RGBA = gdk::RGBA::new(0.025, 0.02, 0.022, 1.);

mod backdrop_imp {
    use super::*;

    #[derive(Default)]
    pub struct AlbumBackdrop {
        pub(super) picture: RefCell<Option<gtk::Picture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumBackdrop {
        const NAME: &'static str = "BuiscAlbumBackdrop";
        type Type = super::AlbumBackdrop;
        type ParentType = gtk::Widget;

        fn class_init(class: &mut Self::Class) {
            class.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for AlbumBackdrop {
        fn dispose(&self) {
            if let Some(picture) = self.picture.borrow_mut().take() {
                picture.unparent();
            }
        }
    }

    impl WidgetImpl for AlbumBackdrop {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let width = f64::from(widget.width());
            let height = f64::from(widget.height());
            if width <= 0. || height <= 0. {
                return;
            }
            let bounds = graphene::Rect::new(0., 0., width as f32, height as f32);
            snapshot.append_color(&BACKDROP_BASE, &bounds);

            let Some(picture) = self.picture.borrow().clone() else {
                return;
            };
            if picture.paintable().is_none() {
                return;
            }
            let artwork = gtk::Snapshot::new();
            widget.snapshot_child(&picture, &artwork);
            if let Some(node) = artwork.to_node() {
                snapshot.push_blur(42.);
                snapshot.push_clip(&bounds);
                snapshot.append_node(&node);
                snapshot.pop();
                snapshot.pop();
            }

            let stops = [
                gsk::ColorStop::new(0., gdk::RGBA::new(0.02, 0.015, 0.015, 0.72)),
                gsk::ColorStop::new(0.2, gdk::RGBA::new(0.02, 0.015, 0.015, 0.48)),
                gsk::ColorStop::new(0.52, gdk::RGBA::new(0.02, 0.015, 0.015, 0.42)),
                gsk::ColorStop::new(0.78, gdk::RGBA::new(0.01, 0.01, 0.01, 0.52)),
                gsk::ColorStop::new(1., gdk::RGBA::new(0., 0., 0., 0.88)),
            ];
            snapshot.append_linear_gradient(
                &bounds,
                &graphene::Point::new(0., 0.),
                &graphene::Point::new(0., height as f32),
                &stops,
            );
        }
    }
}

glib::wrapper! {
    pub struct AlbumBackdrop(ObjectSubclass<backdrop_imp::AlbumBackdrop>)
        @extends gtk::Widget,
        @implements gtk::Buildable, gtk::ConstraintTarget;
}

impl AlbumBackdrop {
    fn new() -> Self {
        let backdrop: Self = glib::Object::new();
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Cover);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.set_parent(&backdrop);
        *backdrop.imp().picture.borrow_mut() = Some(picture);
        backdrop
    }

    fn set_texture(&self, texture: Option<&gdk::Texture>) {
        if let Some(picture) = self.imp().picture.borrow().as_ref() {
            picture.set_paintable(texture);
        }
        self.queue_draw();
    }
}

pub(super) struct Row {
    root: gtk::ListBoxRow,
    id: String,
    active: Cell<bool>,
}

pub(super) struct NowPlaying {
    pub(super) root: gtk::Overlay,
    backdrop: AlbumBackdrop,
    cover: gtk::Picture,
    fallback: gtk::DrawingArea,
    title: gtk::Label,
    artist: gtk::Label,
    quality: gtk::Label,
    lyrics: gtk::TextView,
    lyric_buffer: gtk::TextBuffer,
    offsets: RefCell<BTreeMap<usize, (i32, i32)>>,
    active_line: Cell<Option<usize>>,
    pub(super) side: gtk::Box,
    pub(super) right_stack: gtk::Stack,
    #[cfg(test)]
    pub(super) art: gtk::Overlay,
    #[cfg(test)]
    pub(super) controls: gtk::Box,
    seek: gtk::Scale,
    position: gtk::Label,
    duration: gtk::Label,
    play: gtk::Button,
    favorite: gtk::Button,
    download: gtk::Button,
    queue_toggle: gtk::Button,
    mode: gtk::Button,
    waveform: gtk::DrawingArea,
    spectrum: Rc<RefCell<Vec<f32>>>,
    queue: gtk::ListBox,
    pub(super) queue_rows: RefCell<Vec<Row>>,
    queue_signature: Cell<Option<u64>>,
    seeking: Cell<bool>,
    ui: std::rc::Weak<Ui>,
    timer: RefCell<Option<glib::SourceId>>,
}

impl NowPlaying {
    pub(super) fn new(ui: &Rc<Ui>) -> Self {
        let root = gtk::Overlay::new();
        root.add_css_class("now-playing-page");
        root.set_can_focus(true);
        root.set_visible(false);

        let backdrop = AlbumBackdrop::new();
        root.set_child(Some(&backdrop));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.add_css_class("now-playing-content");
        content.set_hexpand(true);
        content.set_vexpand(true);
        root.add_overlay(&content);
        root.set_measure_overlay(&content, true);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top.add_css_class("now-playing-top");
        top.set_hexpand(true);
        top.set_halign(gtk::Align::Fill);
        top.set_height_request(44);
        top.set_margin_top(12);
        top.set_margin_end(18);
        top.set_margin_start(18);
        let close = gtk::Button::new();
        close.add_css_class("now-playing-close");
        close.set_child(Some(&icons::light(Icon::ChevronDown, 20)));
        close.set_halign(gtk::Align::End);
        close.set_tooltip_text(Some("返回主界面（Esc）"));
        let top_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top_spacer.set_hexpand(true);
        top.append(&top_spacer);
        top.append(&close);
        content.append(&top);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        body.add_css_class("now-playing-body");
        body.set_vexpand(true);
        body.set_margin_start(28);
        body.set_margin_end(28);
        body.set_margin_bottom(16);
        content.append(&body);

        let main_col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main_col.set_hexpand(true);
        main_col.set_vexpand(true);

        let stage = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stage.add_css_class("now-playing-stage");
        stage.set_hexpand(true);
        stage.set_vexpand(true);
        stage.set_valign(gtk::Align::End);
        stage.set_margin_bottom(24);
        let hero = gtk::Box::new(gtk::Orientation::Vertical, 0);
        hero.add_css_class("now-playing-hero");
        hero.set_halign(gtk::Align::Center);
        hero.set_valign(gtk::Align::Center);

        let meta = gtk::Box::new(gtk::Orientation::Vertical, 6);
        meta.set_halign(gtk::Align::Center);
        let title = label("还没有开始播放", "now-playing-song");
        title.set_xalign(0.5);
        title.set_justify(Justification::Center);
        title.set_ellipsize(EllipsizeMode::End);
        title.set_max_width_chars(48);
        let artist = label("双击曲目，听见喜欢", "now-playing-artist");
        artist.set_xalign(0.5);
        artist.set_justify(Justification::Center);
        artist.set_ellipsize(EllipsizeMode::End);
        artist.set_max_width_chars(42);
        let quality = label("音频", "now-playing-quality");
        quality.set_halign(gtk::Align::Center);
        meta.append(&title);
        meta.append(&artist);
        meta.append(&quality);
        hero.append(&meta);

        let art = gtk::Overlay::new();
        art.add_css_class("now-playing-art");
        art.set_halign(gtk::Align::Center);
        art.set_valign(gtk::Align::Center);
        art.set_margin_top(24);
        art.set_size_request(260, 260);
        art.set_height_request(260);
        art.set_overflow(gtk::Overflow::Hidden);
        let fallback = icons::artwork(260);
        art.set_child(Some(&fallback));
        let cover = gtk::Picture::new();
        cover.set_can_shrink(true);
        cover.set_content_fit(gtk::ContentFit::Cover);
        cover.set_visible(false);
        art.add_overlay(&cover);
        hero.append(&art);
        stage.append(&hero);
        main_col.append(&stage);

        let controls = gtk::Box::new(gtk::Orientation::Vertical, 11);
        controls.add_css_class("now-playing-controls");
        controls.set_width_request(480);
        controls.set_hexpand(true);
        controls.set_halign(gtk::Align::Fill);
        controls.set_margin_start(24);
        controls.set_margin_end(24);
        controls.set_margin_bottom(40);

        let spectrum = Rc::new(RefCell::new(Vec::new()));
        let waveform = waveform(spectrum.clone());
        controls.append(&waveform);

        let progress = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        progress.add_css_class("now-playing-progress");
        let position = label("0:00", "now-playing-time");
        position.set_width_chars(5);
        let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1., 1.);
        seek.set_draw_value(false);
        seek.set_hexpand(true);
        seek.set_sensitive(false);
        let duration = label("0:00", "now-playing-time");
        duration.set_xalign(1.);
        duration.set_width_chars(5);
        progress.append(&position);
        progress.append(&seek);
        progress.append(&duration);
        controls.append(&progress);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("now-playing-actions");
        let left_actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let favorite = gtk::Button::new();
        favorite.add_css_class("now-playing-control");
        favorite.set_child(Some(&icons::light(Icon::Heart, 19)));
        favorite.set_tooltip_text(Some("收藏当前歌曲"));
        left_actions.append(&favorite);
        actions.append(&left_actions);

        let transport = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        transport.set_hexpand(true);
        transport.set_halign(gtk::Align::Center);
        let previous = gtk::Button::new();
        previous.add_css_class("now-playing-control");
        previous.set_child(Some(&icons::light(Icon::Previous, 20)));
        previous.set_tooltip_text(Some("上一首"));
        let play = gtk::Button::new();
        play.add_css_class("now-playing-control");
        play.add_css_class("now-playing-play");
        icons::set_play(&play, false);
        let next = gtk::Button::new();
        next.add_css_class("now-playing-control");
        next.set_child(Some(&icons::light(Icon::Next, 20)));
        next.set_tooltip_text(Some("下一首"));
        transport.append(&previous);
        transport.append(&play);
        transport.append(&next);
        actions.append(&transport);

        let right_actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let queue_toggle = gtk::Button::new();
        queue_toggle.add_css_class("now-playing-control");
        queue_toggle.set_child(Some(&icons::light(Icon::Queue, 19)));
        queue_toggle.set_tooltip_text(Some("显示播放队列"));
        let download = gtk::Button::new();
        download.add_css_class("now-playing-control");
        download.set_child(Some(&icons::light(Icon::Download, 19)));
        download.set_tooltip_text(Some("下载当前歌曲"));
        let mode = button(ui.lib.borrow().mode.label(), "now-playing-mode");
        let weak = Rc::downgrade(ui);
        mode.connect_clicked(move |button| {
            if let Some(ui) = weak.upgrade() {
                let next = ui.lib.borrow().mode.next();
                button.set_label(next.label());
                ui.mode.set_label(next.label());
                ui.lib.borrow_mut().mode = next;
                ui.schedule_save();
            }
        });
        right_actions.append(&queue_toggle);
        right_actions.append(&download);
        right_actions.append(&mode);
        actions.append(&right_actions);
        controls.append(&actions);
        main_col.append(&controls);
        body.append(&main_col);

        let side = gtk::Box::new(gtk::Orientation::Vertical, 8);
        side.add_css_class("now-playing-side");
        side.set_width_request(280);
        side.set_hexpand(false);
        side.set_vexpand(true);
        side.set_visible(false);

        let right_stack = gtk::Stack::new();
        right_stack.set_hexpand(true);
        right_stack.set_vexpand(true);
        right_stack.set_transition_type(gtk::StackTransitionType::Crossfade);

        let lyric_frame = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .overlay_scrolling(false)
            .vexpand(true)
            .build();
        lyric_frame.add_css_class("now-playing-lyrics-frame");
        let lyrics = gtk::TextView::new();
        lyrics.set_editable(false);
        lyrics.set_cursor_visible(false);
        lyrics.set_wrap_mode(WrapMode::Word);
        lyrics.set_justification(Justification::Center);
        lyrics.set_left_margin(18);
        lyrics.set_right_margin(18);
        lyrics.set_top_margin(28);
        lyrics.set_bottom_margin(28);
        let lyric_buffer = lyrics.buffer();
        lyric_buffer.create_tag(
            Some("active"),
            &[
                ("foreground", &"#f4f4f6"),
                ("weight", &650i32),
                ("scale", &1.1f64),
                ("pixels-above-lines", &12i32),
                ("pixels-below-lines", &12i32),
            ],
        );
        lyric_buffer.create_tag(
            Some("normal"),
            &[
                ("foreground", &"#8f8f95"),
                ("scale", &1.0f64),
                ("pixels-above-lines", &10i32),
                ("pixels-below-lines", &10i32),
            ],
        );
        lyric_frame.set_child(Some(&lyrics));
        let lyrics_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        lyrics_page.set_vexpand(true);
        lyrics_page.append(&lyric_frame);
        right_stack.add_named(&lyrics_page, Some("lyrics"));

        let queue_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
        queue_page.set_vexpand(true);
        let queue_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();
        queue_scroll.add_css_class("now-playing-queue-frame");
        let queue = gtk::ListBox::new();
        queue.set_selection_mode(gtk::SelectionMode::None);
        queue.add_css_class("now-playing-queue");
        queue_scroll.set_child(Some(&queue));
        queue_page.append(&queue_scroll);
        right_stack.add_named(&queue_page, Some("queue"));
        side.append(&right_stack);
        body.append(&side);

        let weak_ui = Rc::downgrade(ui);
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(ui) = weak_ui.upgrade() {
                    ui.close_now_playing();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        root.add_controller(key);

        let weak_ui = Rc::downgrade(ui);
        close.connect_clicked(move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.close_now_playing();
            }
        });
        let weak_ui = Rc::downgrade(ui);
        seek.connect_change_value(move |_, _, value| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.seek_to(value.max(0.) as u64);
            }
            glib::Propagation::Proceed
        });
        let weak_ui = Rc::downgrade(ui);
        play.connect_clicked(move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.toggle();
            }
        });
        let weak_ui = Rc::downgrade(ui);
        previous.connect_clicked(move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.previous();
            }
        });
        let weak_ui = Rc::downgrade(ui);
        next.connect_clicked(move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.next(false);
            }
        });
        let weak_ui = Rc::downgrade(ui);
        favorite.connect_clicked(move |_| {
            if let Some(ui) = weak_ui.upgrade()
                && let Some(track) = ui.current.borrow().clone()
            {
                ui.favorite(&track);
            }
        });
        let weak_ui = Rc::downgrade(ui);
        download.connect_clicked(move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.download_current();
            }
        });
        let weak_ui = Rc::downgrade(ui);
        queue_toggle.connect_clicked(move |_| {
            if let Some(ui) = weak_ui.upgrade()
                && let Some(view) = ui.now_playing.borrow().as_ref()
            {
                view.toggle_queue();
            }
        });
        let weak_ui = Rc::downgrade(ui);
        queue.connect_row_activated(move |_, row| {
            if let Some(ui) = weak_ui.upgrade() {
                let index = ui.now_playing.borrow().as_ref().and_then(|view| {
                    view.queue_rows
                        .borrow()
                        .iter()
                        .position(|item| item.root == *row)
                });
                if let Some(index) = index {
                    ui.play_queue_index(index);
                }
            }
        });

        ui.window_overlay.add_overlay(&root);

        Self {
            root,
            backdrop,
            cover,
            fallback,
            title,
            artist,
            quality,
            lyrics,
            lyric_buffer,
            offsets: RefCell::new(BTreeMap::new()),
            active_line: Cell::new(None),
            side,
            right_stack,
            #[cfg(test)]
            art,
            #[cfg(test)]
            controls,
            seek,
            position,
            duration,
            play,
            favorite,
            download,
            queue_toggle,
            mode,
            waveform,
            spectrum,
            queue,
            queue_rows: RefCell::new(Vec::new()),
            queue_signature: Cell::new(None),
            seeking: Cell::new(false),
            ui: Rc::downgrade(ui),
            timer: RefCell::new(None),
        }
    }

    pub(super) fn present(&self) {
        self.root.set_visible(true);
        self.root.grab_focus();
        if self.timer.borrow().is_none() {
            let weak = self.ui.clone();
            let timer = glib::timeout_add_local(Duration::from_millis(120), move || {
                let Some(ui) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if let Some(view) = ui.now_playing.borrow().as_ref() {
                    view.refresh();
                }
                glib::ControlFlow::Continue
            });
            *self.timer.borrow_mut() = Some(timer);
        }
    }

    pub(super) fn close(&self) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
        }
        self.root.set_visible(false);
    }

    pub(super) fn set_track(&self, track: Option<&Track>) {
        self.title
            .set_text(track.map(|t| t.title.as_str()).unwrap_or("还没有开始播放"));
        self.artist.set_text(
            track
                .map(|t| t.artist.as_str())
                .unwrap_or("双击曲目，听见喜欢"),
        );
        self.title
            .set_tooltip_text(track.map(|track| track.title.as_str()));
        self.favorite.set_sensitive(track.is_some());
        self.download.set_sensitive(track.is_some());
        self.queue_toggle.set_sensitive(track.is_some());
    }

    pub(super) fn set_quality(&self, quality: &str) {
        self.quality.set_text(quality);
    }

    pub(super) fn set_mode(&self, mode: &str) {
        self.mode.set_label(mode);
    }

    pub(super) fn set_cover(&self, texture: Option<gdk::Texture>, backdrop: Option<gdk::Texture>) {
        self.backdrop.set_texture(backdrop.as_ref());
        if let Some(texture) = texture {
            self.cover.set_paintable(Some(&texture));
            self.cover.set_visible(true);
            self.fallback.set_visible(false);
        } else {
            self.cover.set_paintable(gdk::Paintable::NONE);
            self.cover.set_visible(false);
            self.fallback.set_visible(true);
        }
    }

    pub(super) fn set_playing(&self, playing: bool) {
        icons::set_play(&self.play, playing);
        if !playing {
            self.spectrum.borrow_mut().clear();
        }
        self.waveform.queue_draw();
    }

    pub(super) fn set_spectrum(&self, levels: Vec<f32>) {
        *self.spectrum.borrow_mut() = levels;
        self.waveform.queue_draw();
    }

    pub(super) fn set_position(&self, position: u64, duration: u64) {
        if !self.seeking.get() {
            self.seek.set_range(0., duration.max(1) as f64);
            self.seek.set_value(position.min(duration.max(1)) as f64);
            self.seek.set_sensitive(duration > 0);
        }
        self.position.set_text(&super::time_label(position));
        self.duration.set_text(&super::time_label(duration));
    }

    pub(super) fn set_lyrics(&self, lyrics: &Lyrics) {
        self.lyric_buffer.set_text("");
        self.offsets.borrow_mut().clear();
        self.active_line.set(None);
        let segments = lyrics.segments();
        let mut iter = self.lyric_buffer.start_iter();
        for (index, segment) in segments.iter().enumerate() {
            let start = iter.offset();
            self.lyric_buffer.insert(&mut iter, segment.text());
            let end = iter.offset();
            self.offsets.borrow_mut().insert(index, (start, end));
            if index + 1 < segments.len() {
                self.lyric_buffer.insert(&mut iter, "\n");
            }
        }
        if let Some(tag) = self.lyric_buffer.tag_table().lookup("normal") {
            self.lyric_buffer.apply_tag(
                &tag,
                &self.lyric_buffer.start_iter(),
                &self.lyric_buffer.end_iter(),
            );
        }
        if segments.is_empty() {
            self.side.set_visible(false);
        } else {
            self.right_stack.set_visible_child_name("lyrics");
            self.side.set_visible(true);
        }
        self.update_active_line(lyrics, 0);
    }

    pub(super) fn clear_lyrics(&self) {
        self.lyric_buffer.set_text("");
        self.offsets.borrow_mut().clear();
        self.active_line.set(None);
        self.side.set_visible(false);
    }

    fn toggle_queue(&self) {
        if self.side.is_visible()
            && self.right_stack.visible_child_name().as_deref() == Some("queue")
        {
            self.side.set_visible(false);
        } else {
            self.right_stack.set_visible_child_name("queue");
            self.side.set_visible(true);
        }
    }

    pub(super) fn refresh_queue(&self, tracks: &[Track], current_id: Option<&str>) {
        let mut signature = DefaultHasher::new();
        for track in tracks {
            track.id.hash(&mut signature);
        }
        self.queue_signature.set(Some(signature.finish()));
        while let Some(row) = self.queue.row_at_index(0) {
            self.queue.remove(&row);
        }
        self.queue_rows.borrow_mut().clear();
        for (index, track) in tracks.iter().enumerate() {
            let root = gtk::ListBoxRow::new();
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row_box.set_margin_top(8);
            row_box.set_margin_bottom(8);
            row_box.set_margin_start(8);
            row_box.set_margin_end(8);
            let number = label(&format!("{:02}", index + 1), "now-playing-queue-index");
            let copy = gtk::Box::new(gtk::Orientation::Vertical, 2);
            copy.set_hexpand(true);
            let title = label(&track.title, "now-playing-queue-title");
            title.set_ellipsize(EllipsizeMode::End);
            title.set_max_width_chars(24);
            let meta = label(&track.artist, "now-playing-queue-meta");
            meta.set_ellipsize(EllipsizeMode::End);
            meta.set_max_width_chars(24);
            copy.append(&title);
            copy.append(&meta);
            row_box.append(&number);
            row_box.append(&copy);
            root.set_child(Some(&row_box));
            let active = current_id == Some(track.id.as_str());
            if active {
                root.add_css_class("active");
            }
            self.queue.append(&root);
            self.queue_rows.borrow_mut().push(Row {
                root,
                id: track.id.clone(),
                active: Cell::new(active),
            });
        }
    }

    fn refresh(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let (position, duration) = ui
            .audio
            .borrow()
            .as_ref()
            .map(|audio| (audio.position(), audio.duration()))
            .unwrap_or_else(|| {
                (
                    0,
                    ui.current
                        .borrow()
                        .as_ref()
                        .map(|track| track.duration)
                        .unwrap_or_default(),
                )
            });
        self.set_position(position, duration);
        self.update_active_line(&ui.lyrics.borrow(), position.saturating_mul(1000));
        self.waveform.queue_draw();

        let queue = ui.lib.borrow().queue.clone();
        let current_id = ui.current_id();
        let mut signature = DefaultHasher::new();
        for track in &queue {
            track.id.hash(&mut signature);
        }
        let signature = Some(signature.finish());
        if signature != self.queue_signature.get() {
            self.refresh_queue(&queue, current_id.as_deref());
        } else if let Some(id) = current_id.as_deref() {
            for row in self.queue_rows.borrow().iter() {
                let active = row.id == id;
                if active != row.active.get() {
                    row.active.set(active);
                    if active {
                        row.root.add_css_class("active");
                    } else {
                        row.root.remove_css_class("active");
                    }
                }
            }
        }
    }

    pub(super) fn update_active_line(&self, lyrics: &Lyrics, position_ms: u64) {
        let active = lyrics.active_index(position_ms);
        if active == self.active_line.get() {
            return;
        }
        if let Some(previous) = self.active_line.get()
            && let Some((start, end)) = self.offsets.borrow().get(&previous).copied()
            && let Some(tag) = self.lyric_buffer.tag_table().lookup("normal")
        {
            self.lyric_buffer.apply_tag(
                &tag,
                &self.lyric_buffer.iter_at_offset(start),
                &self.lyric_buffer.iter_at_offset(end),
            );
        }
        self.active_line.set(active);
        if let Some(index) = active
            && let Some((start, end)) = self.offsets.borrow().get(&index).copied()
            && let Some(tag) = self.lyric_buffer.tag_table().lookup("active")
        {
            self.lyric_buffer.apply_tag(
                &tag,
                &self.lyric_buffer.iter_at_offset(start),
                &self.lyric_buffer.iter_at_offset(end),
            );
            let mut cursor = self.lyric_buffer.iter_at_offset(start);
            self.lyrics
                .scroll_to_iter(&mut cursor, 0.25, true, 0.5, 0.3);
        }
    }
}

pub(super) fn blurred_cover(pixbuf: &gtk::gdk_pixbuf::Pixbuf) -> Option<gdk::Texture> {
    let width = pixbuf.width().max(1);
    let height = pixbuf.height().max(1);
    let small_width = 160;
    let small_height = (height * small_width / width).max(1);
    let small = pixbuf.scale_simple(
        small_width,
        small_height,
        gtk::gdk_pixbuf::InterpType::Nearest,
    )?;
    let large_width = 1920;
    let large_height = (height * large_width / width).max(1);
    let blurred = small.scale_simple(
        large_width,
        large_height,
        gtk::gdk_pixbuf::InterpType::Nearest,
    )?;
    Some(gdk::Texture::for_pixbuf(&blurred))
}

fn waveform(spectrum: Rc<RefCell<Vec<f32>>>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.add_css_class("now-playing-waveform");
    area.set_content_height(58);
    area.set_hexpand(true);
    let smoothed = Rc::new(RefCell::new(Vec::<f64>::new()));
    let draw_smoothed = Rc::clone(&smoothed);
    area.set_draw_func(move |_, cr, width, height| {
        let width = f64::from(width);
        let height = f64::from(height);
        if width < 2. || height < 2. {
            return;
        }
        let gradient = gtk::cairo::LinearGradient::new(0., height * 0.22, 0., height * 0.78);
        gradient.add_color_stop_rgba(0., 0.72, 1., 0.79, 0.94);
        gradient.add_color_stop_rgba(0.55, 0.34, 0.95, 0.53, 0.9);
        gradient.add_color_stop_rgba(1., 0.16, 0.78, 0.42, 0.8);
        let _ = cr.set_source(&gradient);
        cr.set_line_width(2.4);
        cr.set_line_cap(gtk::cairo::LineCap::Round);

        let side_margin = (width * 0.04).clamp(10., 34.);
        let usable = (width - side_margin * 2.).max(1.);
        let spacing = 5.8;
        let count = (usable / spacing).floor().max(1.) as usize;
        let levels = spectrum.borrow();
        let mut smoothed = draw_smoothed.borrow_mut();
        if smoothed.len() != levels.len() {
            smoothed.resize(levels.len(), 0.06);
        }
        for (index, value) in smoothed.iter_mut().enumerate() {
            let target = levels.get(index).copied().unwrap_or(0.06) as f64;
            let weight = if target > *value { 0.72 } else { 0.24 };
            *value += (target.max(0.04) - *value) * weight;
        }
        let peak = smoothed.iter().copied().fold(0.06_f64, f64::max).max(0.18);
        for index in 0..count {
            let t = index as f64 / (count.saturating_sub(1).max(1)) as f64;
            let edge = (std::f64::consts::PI * t).sin().powf(0.35);
            let source = smoothed
                .get((t * smoothed.len() as f64) as usize)
                .copied()
                .unwrap_or(0.06)
                / peak;
            let energy = source.min(1.).powf(0.72);
            let bar =
                (height * (0.08 + energy * 0.84) * (0.66 + edge * 0.34)).clamp(3., height - 6.);
            let x = side_margin + index as f64 * spacing + spacing * 0.5;
            let center = height * 0.5;
            cr.move_to(x, center - bar * 0.5);
            cr.line_to(x, center + bar * 0.5);
            let _ = cr.stroke();
        }
    });
    area
}
