use super::*;
use crate::account::{AccountProfile, Folder, LoginPoll, RemoteTracks};
use qrcode::{Color, QrCode};
use std::time::{Duration, Instant};

fn background<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
    callback: impl FnOnce(T) + 'static,
) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(job());
    });
    glib::spawn_future_local(async move {
        if let Ok(value) = rx.recv().await {
            callback(value);
        }
    });
}

impl Ui {
    pub(super) fn refresh_account(self: &Rc<Self>) {
        if !self.account.is_logged_in() {
            self.account_button.set_label("扫码登录 B站");
            return;
        }
        self.account_button.set_label("正在验证登录…");
        let account = self.account.clone();
        let weak = Rc::downgrade(self);
        background(
            move || account.profile(),
            move |result| {
                if let Some(ui) = weak.upgrade() {
                    match result {
                        Ok(Some(profile)) => ui.logged_in(profile),
                        Ok(None) => {
                            *ui.profile.borrow_mut() = None;
                            ui.account_button.set_label("登录已失效 · 重新扫码");
                        }
                        Err(error) => {
                            ui.account_button.set_label("B站账号 · 点击重试");
                            ui.status.set_text(&error);
                        }
                    }
                }
            },
        );
    }
    fn logged_in(&self, profile: AccountProfile) {
        let text = format!("{} · B站账号", profile.name);
        self.account_button.set_label(&text);
        self.account_button
            .set_tooltip_text(Some(&format!("{} · UID {}", profile.name, profile.mid)));
        if let Some(l) = self.account_button.child().and_downcast::<gtk::Label>() {
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            l.set_max_width_chars(15);
        }
        *self.profile.borrow_mut() = Some(profile);
        self.status
            .set_text("已登录 B站。可读取收藏夹和历史，播放时自动同步记录。");
    }
    pub(super) fn account_dialog(self: &Rc<Self>) {
        if self.account.is_logged_in() {
            let dialog = gtk::Dialog::builder()
                .title("B站账号")
                .transient_for(&self.window)
                .modal(true)
                .default_width(390)
                .build();
            dialog.add_button("关闭", gtk::ResponseType::Close);
            let area = dialog.content_area();
            margins(&area, 24);
            area.set_spacing(15);
            let title = self
                .profile
                .borrow()
                .as_ref()
                .map_or("已保存登录会话".into(), |p| {
                    format!("{} · UID {}", p.name, p.mid)
                });
            area.append(&label(&title, "title-2"));
            let text = label(
                "登录凭据仅保存在本机，不包含在资料库备份中。\n退出会清除本机凭据，不影响 B站其他设备。",
                "muted",
            );
            text.set_wrap(true);
            area.append(&text);
            let folders = button("打开 B站收藏夹", "primary");
            let refresh = button("刷新账号状态", "");
            let logout = button("退出本机登录", "");
            area.append(&folders);
            area.append(&refresh);
            area.append(&logout);
            {
                let weak = Rc::downgrade(self);
                let d = dialog.clone();
                folders.connect_clicked(move |_| {
                    d.close();
                    if let Some(ui) = weak.upgrade() {
                        ui.navigate(Route::RemoteFavorites);
                    }
                });
            }
            {
                let weak = Rc::downgrade(self);
                let d = dialog.clone();
                refresh.connect_clicked(move |_| {
                    d.close();
                    if let Some(ui) = weak.upgrade() {
                        ui.refresh_account();
                    }
                });
            }
            {
                let weak = Rc::downgrade(self);
                let d = dialog.clone();
                logout.connect_clicked(move |_| {
                    if let Some(ui) = weak.upgrade() {
                        match ui.account.logout() {
                            Ok(()) => {
                                ui.play_generation.set(ui.play_generation.get() + 1);
                                ui.list_generation.set(ui.list_generation.get() + 1);
                                *ui.profile.borrow_mut() = None;
                                ui.account_button.set_label("扫码登录 B站");
                                ui.account_button.set_tooltip_text(None);
                                *ui.last_report.borrow_mut() = None;
                                ui.status.set_text("已退出 B站，保留本地收藏与歌单。");
                                d.close();
                            }
                            Err(error) => ui.status.set_text(&error),
                        }
                    }
                });
            }
            dialog.connect_response(|d, _| d.close());
            dialog.present();
        } else {
            self.login_dialog();
        }
    }
    fn login_dialog(self: &Rc<Self>) {
        let dialog = gtk::Dialog::builder()
            .title("扫码登录 B站")
            .transient_for(&self.window)
            .modal(true)
            .default_width(360)
            .build();
        dialog.add_button("取消", gtk::ResponseType::Cancel);
        let area = dialog.content_area();
        margins(&area, 24);
        area.set_spacing(16);
        area.append(&label("用哔哩哔哩客户端扫一扫", "title-2"));
        let drawing = gtk::DrawingArea::new();
        drawing.set_content_width(250);
        drawing.set_content_height(250);
        drawing.set_halign(gtk::Align::Center);
        type Matrix = Rc<RefCell<Option<(usize, Vec<Color>)>>>;
        let matrix: Matrix = Rc::new(RefCell::new(None));
        let draw_matrix = matrix.clone();
        drawing.set_draw_func(move |_, cr, w, h| {
            if let Some((size, colors)) = draw_matrix.borrow().as_ref() {
                cr.set_source_rgb(1., 1., 1.);
                let _ = cr.paint();
                let side = w.min(h) as f64;
                let cell = (side / (*size + 8) as f64).floor().max(1.);
                let left = (w as f64 - cell * (*size + 8) as f64) / 2.;
                let top = (h as f64 - cell * (*size + 8) as f64) / 2.;
                cr.set_source_rgb(0., 0., 0.);
                for y in 0..*size {
                    for x in 0..*size {
                        if colors[y * size + x] == Color::Dark {
                            cr.rectangle(
                                left + (x + 4) as f64 * cell,
                                top + (y + 4) as f64 * cell,
                                cell,
                                cell,
                            );
                        }
                    }
                }
                let _ = cr.fill();
            } else {
                cr.set_source_rgb(0.95, 0.95, 0.96);
                let _ = cr.paint();
                let side = w.min(h) as f64;
                let inset = (side * 0.08).max(10.);
                cr.set_source_rgb(0.78, 0.78, 0.81);
                cr.set_line_width(2.);
                cr.rectangle(
                    inset + 1.,
                    inset + 1.,
                    (side - inset * 2. - 2.).max(0.),
                    (side - inset * 2. - 2.).max(0.),
                );
                let _ = cr.stroke();

                let cell = ((side - inset * 3.) / 7.).max(4.);
                let left = (w as f64 - cell * 7.) / 2.;
                let top = (h as f64 - cell * 7.) / 2.;
                cr.set_source_rgb(0.80, 0.80, 0.83);
                for y in 0..7 {
                    for x in 0..7 {
                        let finder = (x < 2 && y < 2) || (x > 4 && y < 2) || (x < 2 && y > 4);
                        if finder || (x + y % 3) % 3 == 0 {
                            cr.rectangle(
                                left + x as f64 * cell + cell * 0.12,
                                top + y as f64 * cell + cell * 0.12,
                                cell * 0.76,
                                cell * 0.76,
                            );
                        }
                    }
                }
                let _ = cr.fill();
            }
        });
        area.append(&drawing);
        let message = label("正在向 B站申请二维码…", "muted");
        message.set_halign(gtk::Align::Center);
        message.set_wrap(true);
        area.append(&message);
        let hint = label(
            "请用哔哩哔哩客户端扫码。\n手机端确认登录后会自动完成。\n登录后可读写 B站收藏夹并同步播放历史。",
            "muted",
        );
        hint.set_wrap(true);
        hint.set_halign(gtk::Align::Center);
        hint.set_visible(false);
        area.append(&hint);
        let refresh = button("刷新二维码", "");
        refresh.set_sensitive(false);
        area.append(&refresh);
        let generation = Rc::new(Cell::new(0u64));
        let closed = Rc::new(Cell::new(false));
        let timer = Rc::new(RefCell::new(None::<glib::SourceId>));
        let start: Rc<dyn Fn()> = Rc::new({
            let account = self.account.clone();
            let weak = Rc::downgrade(self);
            let drawing = drawing.clone();
            let message = message.clone();
            let hint = hint.clone();
            let refresh = refresh.clone();
            let generation = generation.clone();
            let closed = closed.clone();
            let timer = timer.clone();
            let dialog = dialog.clone();
            let matrix = matrix.clone();
            move || {
                if let Some(id) = timer.borrow_mut().take() {
                    id.remove();
                }
                generation.set(generation.get() + 1);
                let request_id = generation.get();
                refresh.set_sensitive(false);
                message.set_text("正在向 B站申请二维码…");
                *matrix.borrow_mut() = None;
                drawing.queue_draw();
                hint.set_visible(false);
                let account = account.clone();
                let poll_account = account.clone();
                let weak = weak.clone();
                let drawing = drawing.clone();
                let message = message.clone();
                let hint = hint.clone();
                let refresh = refresh.clone();
                let generation = generation.clone();
                let closed = closed.clone();
                let timer = timer.clone();
                let dialog = dialog.clone();
                let matrix = matrix.clone();
                background(
                    move || account.generate_qr(),
                    move |result| {
                        if closed.get() || generation.get() != request_id {
                            return;
                        }
                        refresh.set_sensitive(true);
                        match result {
                            Err(error) => message.set_text(&format!(
                                "二维码生成失败：{error}\n请检查网络后点击“刷新二维码”。"
                            )),
                            Ok(qr) => {
                                match QrCode::new(qr.url.as_bytes()) {
                                    Ok(code) => {
                                        *matrix.borrow_mut() =
                                            Some((code.width(), code.to_colors()));
                                        drawing.queue_draw();
                                    }
                                    Err(_) => {
                                        message.set_text(
                                            "二维码内容无效，无法显示。\n请点击“刷新二维码”重试。",
                                        );
                                        return;
                                    }
                                }
                                message.set_text("等待扫码 · 二维码约 3 分钟内有效");
                                hint.set_visible(true);
                                let in_flight = Rc::new(Cell::new(false));
                                let deadline = Instant::now() + Duration::from_secs(180);
                                let timer_slot = timer.clone();
                                let source = glib::timeout_add_local(
                                    Duration::from_secs(3),
                                    move || {
                                        if closed.get() || generation.get() != request_id {
                                            return glib::ControlFlow::Break;
                                        }
                                        if Instant::now() >= deadline {
                                            generation.set(generation.get() + 1);
                                            refresh.set_sensitive(true);
                                            message.set_text(
                                                "二维码已过期。\n请点击“刷新二维码”重新生成。",
                                            );
                                            hint.set_visible(false);
                                            *matrix.borrow_mut() = None;
                                            drawing.queue_draw();
                                            if let Some(id) = timer_slot.borrow_mut().take() {
                                                id.remove();
                                            }
                                            return glib::ControlFlow::Break;
                                        }
                                        if in_flight.replace(true) {
                                            return glib::ControlFlow::Continue;
                                        }
                                        let account = poll_account.clone();
                                        let key = qr.key.clone();
                                        let in_flight = in_flight.clone();
                                        let message = message.clone();
                                        let hint = hint.clone();
                                        let closed = closed.clone();
                                        let generation = generation.clone();
                                        let weak = weak.clone();
                                        let dialog = dialog.clone();
                                        let timer = timer_slot.clone();
                                        let matrix = matrix.clone();
                                        let drawing = drawing.clone();
                                        let refresh = refresh.clone();
                                        let deadline = deadline;
                                        background(
                                            move || account.poll_qr(&key),
                                            move |result| {
                                                in_flight.set(false);
                                                if closed.get() || generation.get() != request_id {
                                                    return;
                                                }
                                                if Instant::now() >= deadline {
                                                    generation.set(generation.get() + 1);
                                                    refresh.set_sensitive(true);
                                                    message.set_text(
                                                        "二维码已过期。\n请点击“刷新二维码”重新生成。",
                                                    );
                                                    hint.set_visible(false);
                                                    *matrix.borrow_mut() = None;
                                                    drawing.queue_draw();
                                                    if let Some(id) = timer.borrow_mut().take() {
                                                        id.remove();
                                                    }
                                                    return;
                                                }
                                                match result {
                                                    Ok(LoginPoll::Waiting) => {
                                                        message.set_text("等待手机扫码…")
                                                    }
                                                    Ok(LoginPoll::Scanned) => message
                                                        .set_text("扫码成功，请在手机端确认登录。"),
                                                    Ok(LoginPoll::Expired) => {
                                                        generation.set(generation.get() + 1);
                                                        refresh.set_sensitive(true);
                                                        message.set_text(
                                                            "二维码已过期。\n请点击“刷新二维码”重新生成。",
                                                        );
                                                        hint.set_visible(false);
                                                        *matrix.borrow_mut() = None;
                                                        drawing.queue_draw();
                                                        if let Some(id) = timer.borrow_mut().take()
                                                        {
                                                            id.remove();
                                                        }
                                                    }
                                                    Ok(LoginPoll::Success(profile)) => {
                                                        if let Some(ui) = weak.upgrade() {
                                                            ui.logged_in(profile);
                                                        }
                                                        if let Some(id) = timer.borrow_mut().take()
                                                        {
                                                            id.remove();
                                                        }
                                                        dialog.close();
                                                    }
                                                    Err(error) => {
                                                        generation.set(generation.get() + 1);
                                                        refresh.set_sensitive(true);
                                                        message.set_text(&format!(
                                                            "登录状态验证失败：{error}\n请检查网络后点击“刷新二维码”。"
                                                        ));
                                                        hint.set_visible(false);
                                                        *matrix.borrow_mut() = None;
                                                        drawing.queue_draw();
                                                        if let Some(id) = timer.borrow_mut().take()
                                                        {
                                                            id.remove();
                                                        }
                                                    }
                                                }
                                            },
                                        );
                                        glib::ControlFlow::Continue
                                    },
                                );
                                *timer.borrow_mut() = Some(source);
                            }
                        }
                    },
                );
            }
        });
        {
            let start = start.clone();
            refresh.connect_clicked(move |_| start());
        }
        {
            let closed = closed.clone();
            let generation = generation.clone();
            let timer = timer.clone();
            dialog.connect_close_request(move |_| {
                closed.set(true);
                generation.set(generation.get() + 1);
                if let Some(id) = timer.borrow_mut().take() {
                    id.remove();
                }
                glib::Propagation::Proceed
            });
        }
        dialog.connect_response(|d, _| d.close());
        dialog.present();
        start();
    }
    pub(super) fn favorite_remote(self: &Rc<Self>) {
        let Some(track) = self.selected() else {
            self.status.set_text("请先选中要收藏的歌曲。");
            return;
        };
        self.open_remote_folders(Some(track));
    }
    pub(super) fn open_remote_folders(self: &Rc<Self>, favorite: Option<Track>) {
        if !self.account.is_logged_in() {
            self.login_dialog();
            return;
        }
        self.status.set_text("正在读取 B站收藏夹…");
        let account = self.account.clone();
        let weak = Rc::downgrade(self);
        background(
            move || account.folders(),
            move |result| {
                if let Some(ui) = weak.upgrade() {
                    match result {
                        Ok(folders) => ui.choose_remote_folder(folders, favorite),
                        Err(error) => ui.status.set_text(&error),
                    }
                }
            },
        );
    }
    fn choose_remote_folder(self: &Rc<Self>, folders: Vec<Folder>, favorite: Option<Track>) {
        if folders.is_empty() {
            self.status
                .set_text("这个账号还没有收藏夹，请先在 B站创建一个收藏夹。");
            return;
        }
        let dialog = gtk::Dialog::builder()
            .title(if favorite.is_some() {
                "收藏到 B站收藏夹"
            } else {
                "选择 B站收藏夹"
            })
            .transient_for(&self.window)
            .modal(true)
            .default_width(390)
            .build();
        dialog.add_button("取消", gtk::ResponseType::Cancel);
        dialog.add_button(
            if favorite.is_some() {
                "收藏"
            } else {
                "打开"
            },
            gtk::ResponseType::Accept,
        );
        let names: Vec<String> = folders
            .iter()
            .map(|f| format!("{} · {} 首", f.title, f.count))
            .collect();
        let refs: Vec<_> = names.iter().map(String::as_str).collect();
        let choice = gtk::DropDown::from_strings(&refs);
        margins(&choice, 24);
        dialog.content_area().append(&choice);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d, response| {
            if response == gtk::ResponseType::Accept
                && let (Some(ui), Some(folder)) =
                    (weak.upgrade(), folders.get(choice.selected() as usize))
            {
                if let Some(track) = favorite.clone() {
                    ui.status.set_text("正在收藏到 B站…");
                    let account = ui.account.clone();
                    let id = folder.id;
                    let name = folder.title.clone();
                    let weak = Rc::downgrade(&ui);
                    background(
                        move || account.favorite(&track.bvid, id),
                        move |result| {
                            if let Some(ui) = weak.upgrade() {
                                match result {
                                    Ok(()) => {
                                        ui.status.set_text(&format!("已收藏到 B站「{name}」。"))
                                    }
                                    Err(error) => ui.status.set_text(&error),
                                }
                            }
                        },
                    );
                } else {
                    ui.navigate(Route::RemoteFolder(folder.id, folder.title.clone()));
                }
            }
            d.close();
        });
        dialog.present();
    }
    pub(super) fn load_remote(self: &Rc<Self>, append: bool) {
        if !self.account.is_logged_in() {
            self.set_tracks(vec![]);
            self.status.set_text("请先扫码登录 B站。");
            self.login_dialog();
            return;
        }
        if self.loading.get() || self.active_requests.get() >= 3 {
            return;
        }
        self.active_requests.set(self.active_requests.get() + 1);
        self.loading.set(true);
        self.search_button.set_sensitive(false);
        self.status.set_text("正在同步 B站资料…");
        let page = if append { self.page.get() + 1 } else { 1 };
        self.page.set(page);
        let generation = self.list_generation.get() + 1;
        self.list_generation.set(generation);
        let route = self.route.borrow().clone();
        let account = self.account.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result: Result<RemoteTracks, String> = match route {
                Route::RemoteFolder(id, _) => account.folder_tracks(id, page),
                Route::RemoteHistory => account.history(page),
                _ => Err("无效的 B站资料页面".into()),
            };
            let result = result.map(|r| SearchResult {
                tracks: r.tracks,
                has_more: r.has_more,
            });
            let _ = tx.send_blocking(Event::List(generation, result, append));
        });
    }
    pub(super) fn sync_history(self: &Rc<Self>, force: bool) {
        if !self.account.is_logged_in()
            || !self.lib.borrow().sync_history
            || self.report_inflight.get()
        {
            return;
        }
        let Some(track) = self.current_track() else {
            return;
        };
        let position = self.position();
        if let Some((id, at, last_position)) = self.last_report.borrow().as_ref()
            && id == &track.id
            && ((!force && at.elapsed() < std::time::Duration::from_secs(30))
                || (force && position == *last_position))
        {
            return;
        }
        *self.last_report.borrow_mut() =
            Some((track.id.clone(), std::time::Instant::now(), position));
        self.report_inflight.set(true);
        let account = self.account.clone();
        let weak = Rc::downgrade(self);
        // Hold the application only while finishing a real account write. Closing
        // the window can therefore flush the final position without blocking GTK.
        let hold = self.window.application().map(|a| a.hold());
        background(
            move || account.report(&track.bvid, position),
            move |result| {
                if let Some(ui) = weak.upgrade() {
                    ui.report_inflight.set(false);
                    if let Err(error) = result {
                        ui.status
                            .set_text(&format!("本地记录已保存，B站同步失败：{error}"));
                    }
                }
                drop(hold);
            },
        );
    }
}
