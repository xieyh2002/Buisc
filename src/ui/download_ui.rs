use super::*;

fn audio_filename(title: &str) -> String {
    let name: String = title
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .take(80)
        .collect();
    let name = name.trim().trim_matches('.').trim();
    format!(
        "{}.m4a",
        if name.is_empty() {
            "Buisc 音频"
        } else {
            name
        }
    )
}

impl Ui {
    pub(super) fn download_audio(self: &Rc<Self>) {
        if self.downloading.get() {
            return;
        }
        let Some(track) = self.current_track().or_else(|| self.selected()) else {
            self.status.set_text("请先播放或选中一首歌曲。");
            return;
        };
        self.downloading.set(true);
        self.download_button.set_sensitive(false);
        let chooser = gtk::FileChooserNative::new(
            Some("保存音频"),
            Some(&self.window),
            gtk::FileChooserAction::Save,
            Some("下载"),
            Some("取消"),
        );
        chooser.set_current_name(&audio_filename(&track.title));
        if let Some(directory) = glib::user_special_dir(glib::UserDirectory::Downloads) {
            let _ = chooser.set_current_folder(Some(&gio::File::for_path(directory)));
        }
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("M4A 音频"));
        filter.add_pattern("*.m4a");
        chooser.add_filter(&filter);
        let weak = Rc::downgrade(self);
        chooser.connect_response(move |chooser, response| {
            if let Some(ui) = weak.upgrade() {
                let path = if response == gtk::ResponseType::Accept {
                    chooser.file().and_then(|file| file.path())
                } else {
                    None
                };
                if let Some(mut path) = path.filter(|_| !ui.quitting.get()) {
                    if path.extension().is_none() {
                        path.set_extension("m4a");
                    }
                    ui.status.set_text(&format!("正在下载：{}…", track.title));
                    ui.download_cancel
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    let cancel = ui.download_cancel.clone();
                    let api = ui.api.clone();
                    let bvid = track.bvid.clone();
                    let tx = ui.tx.clone();
                    std::thread::spawn(move || {
                        let result = api.resolve(&bvid).and_then(|stream| {
                            crate::download::save_audio(&stream.url, &path, &cancel)
                        });
                        let _ = tx.send_blocking(Event::Download(path, result));
                    });
                } else {
                    ui.downloading.set(false);
                    ui.download_button.set_sensitive(true);
                }
            }
            chooser.destroy();
        });
        chooser.show();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filename_stays_in_selected_directory() {
        assert_eq!(audio_filename("../歌/曲\\测试\n"), "歌曲测试.m4a");
        assert_eq!(audio_filename("..."), "Buisc 音频.m4a");
        assert!(audio_filename(&"音".repeat(200)).chars().count() <= 84);
    }
}
