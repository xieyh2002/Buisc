use super::*;
use std::time::Duration;

impl Ui {
    pub(super) fn load_lyrics(&self, generation: u64, bvid: &str, cid: Option<u64>) {
        self.lyrics_generation.set(Some(generation));
        if self.lyrics_inflight.get() {
            *self.pending_lyrics.borrow_mut() = Some((generation, bvid.to_owned(), cid));
        } else {
            self.fetch_lyrics(generation, bvid.to_owned(), cid);
        }
    }

    fn fetch_lyrics(&self, generation: u64, bvid: String, cid: Option<u64>) {
        self.lyrics_inflight.set(true);
        let api = self.api.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let lyrics = match cid {
                Some(cid) => api.lyrics(&bvid, cid),
                None => api.lyrics_for_video(&bvid),
            }
            .unwrap_or_default();
            let _ = tx.send_blocking(Event::Lyrics(generation, lyrics));
        });
    }

    pub(super) fn receive_lyrics(self: &Rc<Self>, generation: u64, lyrics: Lyrics) {
        self.lyrics_inflight.set(false);
        let pending = self.pending_lyrics.borrow_mut().take();
        if let Some((next_generation, bvid, cid)) = pending {
            self.fetch_lyrics(next_generation, bvid, cid);
        }
        if self.lyrics_generation.get() != Some(generation) || lyrics.is_empty() {
            return;
        }
        *self.lyrics.borrow_mut() = lyrics;
        self.lyric_line.set_visible(true);
        self.update_lyric(self.lyric_position().unwrap_or_default());
        if self.playing() {
            self.ensure_lyrics_timer();
        }
    }

    pub(super) fn clear_lyrics(&self) {
        self.lyrics_generation.set(None);
        self.pending_lyrics.borrow_mut().take();
        *self.lyrics.borrow_mut() = Lyrics::default();
        self.lyric_line.set_text("");
        self.lyric_line.set_tooltip_text(None);
        self.lyric_line.set_visible(false);
    }

    fn lyric_position(&self) -> Option<u64> {
        self.audio.borrow().as_ref()?.position_ms()
    }

    pub(super) fn update_lyric(&self, position_ms: u64) {
        if self.lyric_line.is_visible() {
            let lyrics = self.lyrics.borrow();
            let line = lyrics.line_at(position_ms);
            let text = line.unwrap_or(" ");
            if self.lyric_line.text() != text {
                self.lyric_line.set_text(text);
                self.lyric_line.set_tooltip_text(line);
            }
        }
    }

    pub(super) fn ensure_lyrics_timer(self: &Rc<Self>) {
        if !self.lyric_line.is_visible() || self.lyrics_timer.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(ui) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if let Some(position) = ui.lyric_position() {
                ui.update_lyric(position);
            }
            glib::ControlFlow::Continue
        });
        *self.lyrics_timer.borrow_mut() = Some(timer);
    }

    pub(super) fn remove_lyrics_timer(&self) {
        if let Some(timer) = self.lyrics_timer.borrow_mut().take() {
            timer.remove();
        }
    }
}
