use std::cell::Cell;
use std::rc::Rc;

use gst::glib::{ControlFlow, FlagsClass};
use gst::prelude::*;
use gstreamer as gst;

use crate::api;

const BUFFER_SIZE: i32 = 2 * 1024 * 1024;
const HTTP_TIMEOUT_SECONDS: u32 = 15;
const REFERER: &str = "https://www.bilibili.com/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioEvent {
    Ended,
    Error(String),
    Playing(bool),
    Duration(u64),
}

pub struct AudioPlayer {
    playbin: gst::Element,
    desired_playing: Rc<Cell<bool>>,
    _bus_watch: Option<gst::bus::BusWatchGuard>,
}

impl AudioPlayer {
    pub fn new(on_event: impl Fn(AudioEvent) + 'static) -> Result<Self, String> {
        gst::init().map_err(|error| format!("GStreamer 初始化失败：{error}"))?;

        let playbin = gst::ElementFactory::make("playbin")
            .name("bili-audio-player")
            .build()
            .map_err(|error| format!("无法创建 GStreamer playbin：{error}"))?;

        configure_audio_only(&playbin)?;
        configure_http_headers(&playbin);

        let desired_playing = Rc::new(Cell::new(false));
        let bus_desired_playing = Rc::clone(&desired_playing);
        let bus = playbin
            .bus()
            .ok_or_else(|| "GStreamer playbin 没有消息总线".to_string())?;
        let bus_playbin = playbin.clone();
        let mut was_playing = false;
        let bus_watch = bus
            .add_watch_local(move |_, message| {
                match message.view() {
                    gst::MessageView::Error(error) => {
                        bus_desired_playing.set(false);
                        was_playing = false;
                        let _ = bus_playbin.set_state(gst::State::Null);
                        on_event(AudioEvent::Playing(false));

                        let mut text = error.error().message().to_string();
                        if let Some(debug) = error.debug().filter(|debug| !debug.is_empty()) {
                            text.push('：');
                            text.push_str(&debug);
                        }
                        on_event(AudioEvent::Error(text.chars().take(600).collect()));
                    }
                    gst::MessageView::Eos(_) => {
                        bus_desired_playing.set(false);
                        was_playing = false;
                        on_event(AudioEvent::Ended);
                    }
                    gst::MessageView::Buffering(buffering) => {
                        if buffering.percent() < 100 {
                            if bus_desired_playing.get() {
                                let _ = bus_playbin.set_state(gst::State::Paused);
                            }
                        } else if bus_desired_playing.get() {
                            let _ = bus_playbin.set_state(gst::State::Playing);
                        }
                    }
                    gst::MessageView::StateChanged(changed)
                        if message.src() == Some(bus_playbin.upcast_ref::<gst::Object>()) =>
                    {
                        let playing = changed.current() == gst::State::Playing;
                        if playing != was_playing {
                            was_playing = playing;
                            on_event(AudioEvent::Playing(playing));
                        }
                    }
                    gst::MessageView::DurationChanged(_) => {
                        let duration = bus_playbin
                            .query_duration::<gst::ClockTime>()
                            .map_or(0, gst::ClockTime::seconds);
                        on_event(AudioEvent::Duration(duration));
                    }
                    _ => {}
                }
                ControlFlow::Continue
            })
            .map_err(|error| format!("无法监听 GStreamer 消息总线：{error}"))?;

        Ok(Self {
            playbin,
            desired_playing,
            _bus_watch: Some(bus_watch),
        })
    }

    pub fn load(&self, url: &str) -> Result<(), String> {
        if !api::valid_audio_url(url) {
            return Err("音频地址无效".to_string());
        }
        self.start_uri(url)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn load_dash(
        &self,
        url: &str,
        duration: u64,
        initialization_range: &str,
        index_range: &str,
        bandwidth: u64,
        codecs: &str,
    ) -> Result<(), String> {
        let mpd = build_dash_manifest(
            url,
            duration,
            initialization_range,
            index_range,
            bandwidth,
            codecs,
        )?;
        self.start_uri(&data_uri(&mpd))
    }

    fn start_uri(&self, uri: &str) -> Result<(), String> {
        self.desired_playing.set(true);
        if let Err(error) = self.playbin.set_state(gst::State::Ready) {
            self.desired_playing.set(false);
            let _ = self.playbin.set_state(gst::State::Null);
            return Err(format!("无法重置音频播放器：{error}"));
        }
        self.playbin.set_property("uri", uri);
        if let Err(error) = self.playbin.set_state(gst::State::Playing) {
            self.desired_playing.set(false);
            let _ = self.playbin.set_state(gst::State::Null);
            return Err(format!("无法开始音频播放：{error}"));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn load_test_file(&self, path: &std::path::Path) -> Result<(), String> {
        let uri = gtk::gio::prelude::FileExt::uri(&gtk::gio::File::for_path(path));
        self.start_uri(uri.as_str())
    }

    pub fn toggle(&self) {
        if self.desired_playing.get() {
            self.pause();
        } else {
            self.play();
        }
    }

    pub fn pause(&self) {
        self.desired_playing.set(false);
        let _ = self.playbin.set_state(gst::State::Paused);
    }

    pub fn play(&self) {
        self.desired_playing.set(true);
        if self.playbin.set_state(gst::State::Playing).is_err() {
            self.desired_playing.set(false);
        }
    }

    pub fn stop(&self) {
        self.desired_playing.set(false);
        let _ = self.playbin.set_state(gst::State::Null);
    }

    pub fn seek(&self, seconds: u64) -> Result<(), String> {
        let seconds = seconds.min(gst::ClockTime::MAX.seconds());
        let position = gst::ClockTime::from_seconds(seconds);
        self.playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, position)
            .map_err(|_| "当前音频流暂不支持跳转".to_string())
    }

    pub fn set_volume(&self, volume: f64) {
        if volume.is_finite() {
            self.playbin.set_property("volume", volume.clamp(0.0, 1.0));
        }
    }

    pub fn position(&self) -> u64 {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map_or(0, gst::ClockTime::seconds)
    }

    pub fn duration(&self) -> u64 {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map_or(0, gst::ClockTime::seconds)
    }

    pub fn is_playing(&self) -> bool {
        self.playbin.current_state() == gst::State::Playing
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.desired_playing.set(false);
        let _ = self.playbin.set_state(gst::State::Null);
        self._bus_watch.take();
    }
}

fn configure_audio_only(playbin: &gst::Element) -> Result<(), String> {
    if !playbin.has_property("flags") || !playbin.has_property("buffer-size") {
        return Err("当前 GStreamer playbin 不支持所需的音频配置".to_string());
    }

    let flags = playbin.property_value("flags");
    let flags_class = FlagsClass::with_type(flags.type_())
        .ok_or_else(|| "GStreamer playbin flags 类型无效".to_string())?;
    let audio_flags = flags_class
        .builder()
        .set_by_nick("audio")
        .set_by_nick("buffering")
        .set_by_nick("soft-volume")
        .build()
        .ok_or_else(|| "当前 GStreamer playbin 不支持音频专用 flags".to_string())?;

    playbin.set_property_from_value("flags", &audio_flags);
    playbin.set_property("buffer-size", BUFFER_SIZE);
    Ok(())
}

fn configure_http_headers(playbin: &gst::Element) {
    playbin.connect("source-setup", false, |values| {
        if let Some(source) = values
            .get(1)
            .and_then(|value| value.get::<gst::Element>().ok())
        {
            configure_http_element(&source);
        }
        None
    });
    playbin.connect("element-setup", false, |values| {
        if let Some(element) = values
            .get(1)
            .and_then(|value| value.get::<gst::Element>().ok())
        {
            configure_http_element(&element);
        }
        None
    });
}

fn configure_http_element(element: &gst::Element) {
    if element.has_property("user-agent") {
        element.set_property("user-agent", api::USER_AGENT.to_string());
    }
    if element.has_property("extra-headers") {
        element.set_property(
            "extra-headers",
            gst::Structure::builder("extra-headers")
                .field("Referer", REFERER)
                .build(),
        );
    }
    if element.has_property("timeout") {
        element.set_property("timeout", HTTP_TIMEOUT_SECONDS);
    }
}

fn build_dash_manifest(
    url: &str,
    duration: u64,
    initialization_range: &str,
    index_range: &str,
    bandwidth: u64,
    codecs: &str,
) -> Result<String, String> {
    if !api::valid_audio_url(url) {
        return Err("音频地址无效".to_string());
    }
    if duration == 0 {
        return Err("音频时长无效，无法建立可跳转的 DASH 索引".to_string());
    }
    let initialization_range = normalize_byte_range(initialization_range)?;
    let index_range = normalize_byte_range(index_range)?;
    let codecs = if codecs.trim().is_empty() {
        "mp4a.40.2"
    } else {
        codecs.trim()
    };
    let codecs = xml_escape(codecs);
    let url = xml_escape(url);
    let bandwidth = bandwidth.max(1);

    Ok(format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
            "<MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\" type=\"static\" ",
            "profiles=\"urn:mpeg:dash:profile:isoff-on-demand:2011\" ",
            "mediaPresentationDuration=\"PT{duration}S\" minBufferTime=\"PT2S\">",
            "<Period id=\"0\" duration=\"PT{duration}S\">",
            "<AdaptationSet id=\"0\" contentType=\"audio\" mimeType=\"audio/mp4\" ",
            "segmentAlignment=\"true\" startWithSAP=\"1\">",
            "<Representation id=\"0\" bandwidth=\"{bandwidth}\" codecs=\"{codecs}\">",
            "<BaseURL>{url}</BaseURL>",
            "<SegmentBase indexRange=\"{index_range}\" indexRangeExact=\"true\">",
            "<Initialization range=\"{initialization_range}\"/>",
            "</SegmentBase></Representation></AdaptationSet></Period></MPD>"
        ),
        duration = duration,
        bandwidth = bandwidth,
        codecs = codecs,
        url = url,
        index_range = index_range,
        initialization_range = initialization_range
    ))
}

fn normalize_byte_range(value: &str) -> Result<String, String> {
    let (start, end) = value
        .trim()
        .split_once('-')
        .ok_or_else(|| "DASH 字节范围格式无效".to_string())?;
    let start = start
        .parse::<u64>()
        .map_err(|_| "DASH 字节范围起点无效".to_string())?;
    let end = end
        .parse::<u64>()
        .map_err(|_| "DASH 字节范围终点无效".to_string())?;
    if end < start {
        return Err("DASH 字节范围终点早于起点".to_string());
    }
    Ok(format!("{start}-{end}"))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn data_uri(value: &str) -> String {
    let mut encoded = String::from("data:application/dash+xml;charset=utf-8,");
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playbin_is_audio_only_and_volume_is_bounded() {
        gst::init().unwrap();
        let playbin = gst::ElementFactory::make("playbin").build().unwrap();
        configure_audio_only(&playbin).unwrap();

        let flags = playbin.property_value("flags");
        let flags_class = FlagsClass::with_type(flags.type_()).unwrap();
        assert!(flags_class.is_set_by_nick(&flags, "audio"));
        assert!(flags_class.is_set_by_nick(&flags, "buffering"));
        assert!(flags_class.is_set_by_nick(&flags, "soft-volume"));
        assert!(!flags_class.is_set_by_nick(&flags, "video"));
        assert!(!flags_class.is_set_by_nick(&flags, "text"));
        assert!(!flags_class.is_set_by_nick(&flags, "vis"));
        assert!(!flags_class.is_set_by_nick(&flags, "download"));
        assert_eq!(playbin.property::<i32>("buffer-size"), BUFFER_SIZE);

        let player = AudioPlayer::new(|_| {}).unwrap();
        player.set_volume(-1.0);
        assert_eq!(player.playbin.property::<f64>("volume"), 0.0);
        player.set_volume(2.0);
        assert_eq!(player.playbin.property::<f64>("volume"), 1.0);
        player.set_volume(0.42);
        assert!((player.playbin.property::<f64>("volume") - 0.42).abs() < f64::EPSILON);
    }

    #[test]
    fn dash_manifest_contains_exact_segment_ranges() {
        let mpd = build_dash_manifest(
            "https://cdn.bilivideo.com/audio.m4s?a=1&b=2",
            212,
            "0-817",
            "818-1365",
            184_067,
            "mp4a.40.2",
        )
        .unwrap();
        assert!(mpd.contains("<BaseURL>https://cdn.bilivideo.com/audio.m4s?a=1&amp;b=2</BaseURL>"));
        assert!(mpd.contains("<Initialization range=\"0-817\"/>"));
        assert!(mpd.contains("indexRange=\"818-1365\""));
        assert!(mpd.contains("bandwidth=\"184067\""));
        assert!(data_uri(&mpd).starts_with("data:application/dash+xml"));
    }

    #[test]
    fn soup_source_gets_headers_and_timeout() {
        gst::init().unwrap();
        let source = gst::ElementFactory::make("souphttpsrc").build().unwrap();
        configure_http_element(&source);

        assert_eq!(source.property::<String>("user-agent"), api::USER_AGENT);
        assert_eq!(source.property::<u32>("timeout"), HTTP_TIMEOUT_SECONDS);
        let headers = source.property::<gst::Structure>("extra-headers");
        assert_eq!(headers.get::<&str>("Referer").unwrap(), REFERER);
    }

    #[test]
    fn local_audiotestsrc_pipeline_reaches_eos() {
        gst::init().unwrap();
        let pipeline = gst::Pipeline::new();
        let source = gst::ElementFactory::make("audiotestsrc")
            .property("num-buffers", 2i32)
            .build()
            .unwrap();
        let sink = gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .property("async", false)
            .build()
            .unwrap();
        pipeline.add_many([&source, &sink]).unwrap();
        source.link(&sink).unwrap();
        let bus = pipeline.bus().unwrap();

        pipeline.set_state(gst::State::Playing).unwrap();
        let message = bus
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(2),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .expect("本地 audiotestsrc 应在超时前结束");
        assert!(
            matches!(message.view(), gst::MessageView::Eos(_)),
            "unexpected message: {message:?}"
        );
        pipeline.set_state(gst::State::Null).unwrap();
    }
}
