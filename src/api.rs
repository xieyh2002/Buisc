use crate::account::AccountClient;
use md5::compute as md5_digest;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use url::Url;
use url::form_urlencoded;

const API_ORIGIN: &str = "https://api.bilibili.com";
const MAX_QUERY_CHARS: usize = 100;
const MAX_PAGE: u32 = 50;
const PAGE_SIZE: u32 = 20;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const WBI_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const EVENTUAL_REFERER: &str = "https://www.bilibili.com/";

pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (X11; Linux x86_64) ",
    "AppleWebKit/537.36 (KHTML, like Gecko) ",
    "Chrome/131.0.0.0 Safari/537.36"
);

const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub id: String,
    pub bvid: String,
    pub title: String,
    pub artist: String,
    pub cover: String,
    pub duration: u64,
    pub play_count: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SearchResult {
    pub tracks: Vec<Track>,
    pub has_more: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct AudioStream {
    pub url: String,
    pub duration: u64,
    #[serde(default)]
    pub initialization_range: String,
    #[serde(default)]
    pub index_range: String,
    #[serde(default)]
    pub bandwidth: u64,
    #[serde(default)]
    pub codecs: String,
}

#[derive(Clone)]
pub struct BiliClient {
    client: Client,
    api_base: String,
    wbi_cache: Arc<Mutex<Option<WbiCache>>>,
    account: Option<AccountClient>,
}

#[derive(Clone)]
struct WbiCache {
    mixin_key: String,
    expires_at: Instant,
}

struct AudioCandidate {
    bandwidth: u64,
    id: u64,
    url: String,
    initialization_range: String,
    index_range: String,
    codecs: String,
}

#[derive(Debug)]
enum RequestError {
    Network(String),
    Http(String),
    Api(String),
    Data(String),
    Risk(String),
}

impl fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(message)
            | Self::Http(message)
            | Self::Api(message)
            | Self::Data(message)
            | Self::Risk(message) => formatter.write_str(message),
        }
    }
}

pub fn valid_bvid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 12
        && bytes.starts_with(b"BV")
        && bytes[2..].iter().all(u8::is_ascii_alphanumeric)
}

fn direct_bvid(value: &str) -> Option<String> {
    let value = value.trim();
    if valid_bvid(value) {
        return Some(value.to_owned());
    }
    if value.chars().any(char::is_control) {
        return None;
    }

    let parsed = Url::parse(value).ok()?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("www.bilibili.com")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some_and(|port| port != 443)
    {
        return None;
    }
    let segments = parsed
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();
    segments
        .windows(2)
        .find(|parts| parts[0] == "video" && valid_bvid(parts[1]))
        .map(|parts| parts[1].to_owned())
}

fn page_has_more(page: u32, num_pages: u64, total: u64, result_len: usize) -> bool {
    if page >= MAX_PAGE {
        return false;
    }
    if num_pages > 0 {
        u64::from(page) < num_pages
    } else if total > 0 {
        u64::from(page) * u64::from(PAGE_SIZE) < total
    } else {
        result_len >= PAGE_SIZE as usize
    }
}

pub fn valid_audio_url(value: &str) -> bool {
    if value.is_empty() || value.chars().any(char::is_control) {
        return false;
    }
    let Ok(parsed) = Url::parse(value) else {
        return false;
    };
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some_and(|port| port != 443)
    {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    allowed_host(host, &["bilivideo.com", "bilivideo.cn"])
}

fn allowed_host(host: &str, suffixes: &[&str]) -> bool {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    suffixes
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

fn safe_wbi_image_url(value: Option<&Value>) -> Result<String, RequestError> {
    let raw = value
        .and_then(Value::as_str)
        .ok_or_else(|| RequestError::Data("B 站 WBI 签名地址无效".to_owned()))?;
    let normalized = if let Some(stripped) = raw.strip_prefix("//") {
        format!("https:{stripped}")
    } else {
        raw.to_owned()
    };
    let mut parsed = Url::parse(&normalized)
        .map_err(|_| RequestError::Data("B 站 WBI 签名地址无效".to_owned()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some_and(|port| port != 80 && port != 443)
    {
        return Err(RequestError::Data("B 站 WBI 签名地址无效".to_owned()));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| RequestError::Data("B 站 WBI 签名地址无效".to_owned()))?;
    if !allowed_host(host, &["hdslb.com", "biliimg.com", "bilibili.com"]) {
        return Err(RequestError::Data(
            "B 站 WBI 签名地址不在允许的图片域名内".to_owned(),
        ));
    }
    parsed
        .set_scheme("https")
        .map_err(|_| RequestError::Data("B 站 WBI 签名地址无效".to_owned()))?;
    Ok(parsed.into())
}

fn wbi_key_from_url(value: &str) -> Result<String, RequestError> {
    let parsed = Url::parse(value)
        .map_err(|_| RequestError::Data("B 站 WBI 签名密钥格式无效".to_owned()))?;
    let file = parsed
        .path_segments()
        .and_then(|mut segments| segments.rfind(|part| !part.is_empty()))
        .ok_or_else(|| RequestError::Data("B 站 WBI 签名密钥格式无效".to_owned()))?;
    let key = file.split('.').next().unwrap_or_default();
    if key.len() != 32 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RequestError::Data("B 站 WBI 签名密钥格式无效".to_owned()));
    }
    Ok(key.to_owned())
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn build_wbi_query(
    params: &[(String, String)],
    mixin_key: &str,
    timestamp: u64,
) -> (String, String) {
    let mut values = params.to_vec();
    values.push(("wts".to_owned(), timestamp.to_string()));
    values.sort_by(|left, right| left.0.cmp(&right.0));
    values = values
        .into_iter()
        .map(|(key, value)| {
            let cleaned = value
                .chars()
                .filter(|character| !matches!(character, '!' | '\'' | '(' | ')' | '*'))
                .collect::<String>();
            (key, cleaned)
        })
        .collect();

    let query = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(
            values
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .finish();
    let signature_input = format!("{query}{mixin_key}");
    let signature = format!("{:x}", md5_digest(signature_input.as_bytes()));
    (query, signature)
}

pub(crate) fn track_from_item(item: &Value) -> Option<Track> {
    let object = item.as_object()?;
    let bvid = object
        .get("bvid")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            object
                .get("history")
                .and_then(|history| history.get("bvid"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            object
                .get("uri")
                .and_then(Value::as_str)
                .and_then(direct_bvid)
        })
        .unwrap_or_default();
    let bvid = bvid.trim().to_owned();
    if !valid_bvid(&bvid) {
        return None;
    }

    let title = {
        let title = clean_html_text(
            object
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        if title.is_empty() {
            "未知标题".to_owned()
        } else {
            truncate_chars(title, 512)
        }
    };
    let artist = {
        let mut artist = object
            .get("author")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if artist.is_empty() {
            artist = object
                .get("owner")
                .and_then(|owner| owner.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
        }
        if artist.is_empty() {
            artist = object
                .get("upper")
                .and_then(|upper| upper.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
        }
        if artist.is_empty() {
            artist = object
                .get("author_name")
                .and_then(Value::as_str)
                .unwrap_or_default();
        }
        let artist = clean_html_text(artist);
        if artist.is_empty() {
            "未知UP主".to_owned()
        } else {
            truncate_chars(artist, 256)
        }
    };
    let cover = object
        .get("pic")
        .or_else(|| object.get("cover"))
        .and_then(Value::as_str)
        .map(normalize_cover)
        .unwrap_or_default();
    let duration = object
        .get("duration")
        .map(value_to_duration)
        .unwrap_or_default();
    let play_count = object
        .get("play")
        .or_else(|| object.get("stat").and_then(|stat| stat.get("view")))
        .or_else(|| object.get("cnt_info").and_then(|counts| counts.get("play")))
        .map(|value| value_to_u64(Some(value)))
        .unwrap_or_default();

    Some(Track {
        id: bvid.clone(),
        bvid,
        title,
        artist,
        cover,
        duration,
        play_count,
    })
}

fn truncate_chars(value: String, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value;
    }
    value.chars().take(limit).collect()
}

fn audio_codecs(item: &Value) -> String {
    let Some(object) = item.as_object() else {
        return String::new();
    };
    object
        .get("codecs")
        .or_else(|| object.get("codec"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(64)
        .collect()
}

fn audio_segment_ranges(item: &Value) -> (String, String) {
    let Some(segment) = item
        .get("SegmentBase")
        .or_else(|| item.get("segment_base"))
        .and_then(Value::as_object)
    else {
        return (String::new(), String::new());
    };
    let initialization = segment
        .get("Initialization")
        .or_else(|| segment.get("initialization"))
        .and_then(Value::as_str)
        .map(clean_byte_range)
        .unwrap_or_default();
    let index = segment
        .get("indexRange")
        .or_else(|| segment.get("index_range"))
        .and_then(Value::as_str)
        .map(clean_byte_range)
        .unwrap_or_default();
    (initialization, index)
}

fn clean_byte_range(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|character| character.is_ascii_digit() || *character == '-')
        .take(64)
        .collect()
}

fn is_aac_audio(item: &Value) -> bool {
    let Some(object) = item.as_object() else {
        return false;
    };
    let codecs = audio_codecs(item).to_ascii_lowercase();
    let mime_type = object
        .get("mimeType")
        .or_else(|| object.get("mime_type"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    codecs.starts_with("mp4a") || (codecs.is_empty() && mime_type == "audio/mp4")
}

fn candidate_audio_url(item: &Value) -> Option<String> {
    let object = item.as_object()?;
    let mut candidates = Vec::new();
    for key in ["baseUrl", "base_url"] {
        if let Some(value) = object.get(key).and_then(Value::as_str) {
            candidates.push(value);
        }
    }
    for key in ["backupUrl", "backup_url"] {
        if let Some(values) = object.get(key).and_then(Value::as_array) {
            candidates.extend(values.iter().filter_map(Value::as_str));
        }
    }

    candidates.into_iter().find_map(|candidate| {
        let normalized = if let Some(rest) = candidate.strip_prefix("http://") {
            format!("https://{rest}")
        } else {
            candidate.to_owned()
        };
        valid_audio_url(&normalized).then_some(normalized)
    })
}

fn value_to_u64(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(number)) => number
            .as_u64()
            .or_else(|| {
                number
                    .as_i64()
                    .and_then(|number| u64::try_from(number).ok())
            })
            .or_else(|| {
                number.as_f64().and_then(|number| {
                    (number.is_finite() && number >= 0.0).then_some(number as u64)
                })
            })
            .unwrap_or_default(),
        Some(Value::String(text)) => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite() && *number >= 0.0)
            .map(|number| number as u64)
            .unwrap_or_default(),
        _ => 0,
    }
}

fn value_to_duration(value: &Value) -> u64 {
    match value {
        Value::String(text) => parse_duration_seconds(text),
        _ => value_to_u64(Some(value)),
    }
}

fn parse_duration_seconds(value: &str) -> u64 {
    let value = value.trim();
    if value.is_empty() {
        return 0;
    }
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse().unwrap_or_default();
    }
    if !value.contains(':') {
        return 0;
    }
    let parts = value.split(':').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len()) {
        return 0;
    }
    let mut total = 0_u64;
    for part in parts {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return 0;
        }
        let Ok(number) = part.parse::<u64>() else {
            return 0;
        };
        let Some(next) = total
            .checked_mul(60)
            .and_then(|total| total.checked_add(number))
        else {
            return 0;
        };
        total = next;
    }
    total
}

fn normalize_cover(value: &str) -> String {
    let cleaned = clean_html_text(value);
    if let Some(rest) = cleaned.strip_prefix("//") {
        format!("https://{rest}")
    } else if let Some(rest) = cleaned.strip_prefix("http://") {
        format!("https://{rest}")
    } else {
        cleaned
    }
}

fn music_search_keyword(query: &str) -> String {
    let query = query.trim();
    if query.contains("音乐") {
        query.to_owned()
    } else {
        format!("{query} 音乐")
    }
}

fn cover_thumbnail_url(value: &str) -> Result<Url, String> {
    let mut url = Url::parse(&normalize_cover(value)).map_err(|_| "封面地址无效")?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !(host == "hdslb.com" || host.ends_with(".hdslb.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return Err("封面地址无效".into());
    }
    let original = url.path().split('@').next().unwrap_or_default();
    url.set_path(&format!("{original}@160w_160h_1c.jpg"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn clean_html_text(value: &str) -> String {
    let unescaped = decode_html_entities(value);
    let without_tags = strip_html_tags(&unescaped);
    without_tags
        .chars()
        .filter(|character| {
            *character != '\0' && (!character.is_control() || character.is_whitespace())
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_html_tags(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < value.len() {
        let Some(relative_start) = value[cursor..].find('<') else {
            output.push_str(&value[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let Some(relative_end) = value[start..].find('>') else {
            output.push_str(&value[start..]);
            break;
        };
        cursor = start + relative_end + 1;
    }
    output
}

fn decode_html_entities(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < value.len() {
        let Some(relative_amp) = value[cursor..].find('&') else {
            output.push_str(&value[cursor..]);
            break;
        };
        let amp = cursor + relative_amp;
        output.push_str(&value[cursor..amp]);
        let Some(relative_semicolon) = value[amp + 1..].find(';') else {
            output.push('&');
            cursor = amp + 1;
            continue;
        };
        if relative_semicolon > 32 {
            output.push('&');
            cursor = amp + 1;
            continue;
        }
        let semicolon = amp + 1 + relative_semicolon;
        let entity = &value[amp + 1..semicolon];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => {
                entity[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        if let Some(character) = decoded {
            output.push(character);
        } else {
            output.push_str(&value[amp..=semicolon]);
        }
        cursor = semicolon + 1;
    }
    output
}

fn is_risk_text(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    [
        "风控",
        "访问过于频繁",
        "请求过于频繁",
        "请求被拦截",
        "risk control",
    ]
    .iter()
    .any(|word| lowered.contains(word))
}

impl RequestError {
    fn allows_legacy_search(&self) -> bool {
        matches!(self, Self::Api(_) | Self::Data(_))
    }

    fn into_message(self) -> String {
        self.to_string()
    }

    fn risk() -> Self {
        Self::Risk("B 站暂时触发了访问限制，请稍后再试".to_owned())
    }
}

impl BiliClient {
    pub fn new() -> Result<Self, String> {
        Self::with_api_base(API_ORIGIN.to_owned())
    }

    fn with_api_base(api_base: String) -> Result<Self, String> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| "无法初始化 B 站网络客户端".to_owned())?;

        Ok(Self {
            client,
            api_base,
            wbi_cache: Arc::new(Mutex::new(None)),
            account: None,
        })
    }

    pub fn set_account(&mut self, account: AccountClient) {
        self.account = Some(account);
    }

    pub fn cover_thumbnail(&self, source: &str) -> Result<Vec<u8>, String> {
        let url = cover_thumbnail_url(source)?;
        // Public CDN only: no account cookies, disk cache or full-resolution images.
        let response = self
            .client
            .get(url)
            .header("Referer", EVENTUAL_REFERER)
            .send()
            .map_err(|_| "无法加载封面")?
            .error_for_status()
            .map_err(|_| "无法加载封面")?;
        const LIMIT: usize = 512 * 1024;
        if response
            .content_length()
            .is_some_and(|size| size > LIMIT as u64)
        {
            return Err("封面过大".into());
        }
        let mut bytes = Vec::new();
        response
            .take((LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "无法读取封面")?;
        if bytes.len() > LIMIT {
            return Err("封面过大".into());
        }
        Ok(bytes)
    }

    pub fn search(&self, query: &str, page: u32) -> Result<SearchResult, String> {
        let query = query.trim();
        if query.is_empty() {
            return Err("搜索关键词不能为空".to_owned());
        }
        if !(1..=MAX_PAGE).contains(&page) {
            return Err(format!("页码必须是 1 到 {MAX_PAGE} 的整数"));
        }
        if let Some(bvid) = direct_bvid(query) {
            return Ok(SearchResult {
                tracks: vec![self.load_view_track(&bvid)?],
                has_more: false,
            });
        }
        if query.chars().any(char::is_control) {
            return Err("搜索关键词包含不允许的控制字符".to_owned());
        }
        if query.chars().count() > MAX_QUERY_CHARS {
            return Err(format!("搜索关键词不能超过 {MAX_QUERY_CHARS} 个字符"));
        }

        let params = vec![
            ("keyword".to_owned(), music_search_keyword(query)),
            ("search_type".to_owned(), "video".to_owned()),
            ("page".to_owned(), page.to_string()),
            ("page_size".to_owned(), PAGE_SIZE.to_string()),
            ("order".to_owned(), "totalrank".to_owned()),
        ];
        let payload = match self.signed_search_url(&params) {
            Ok(url) => self.get_json_url(&url, &[]),
            Err(error) if error.allows_legacy_search() => {
                let legacy_url = self
                    .api_url("/x/web-interface/search/type", &params)
                    .map_err(RequestError::into_message)?;
                self.get_json_url(&legacy_url, &[])
            }
            Err(error) => Err(error),
        }
        .map_err(RequestError::into_message)?;

        let data = payload
            .get("data")
            .and_then(Value::as_object)
            .ok_or_else(|| "B 站搜索响应缺少数据".to_owned())?;
        let raw_tracks = data
            .get("result")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let tracks = raw_tracks
            .iter()
            .filter_map(track_from_item)
            .collect::<Vec<_>>();

        let total = value_to_u64(data.get("numResults"));
        let num_pages = value_to_u64(data.get("numPages"));
        let has_more = page_has_more(page, num_pages, total, raw_tracks.len());

        Ok(SearchResult { tracks, has_more })
    }

    fn load_view_track(&self, bvid: &str) -> Result<Track, String> {
        let url = self
            .api_url(
                "/x/web-interface/view",
                &[("bvid".to_owned(), bvid.to_owned())],
            )
            .map_err(RequestError::into_message)?;
        let payload = self
            .get_json_url(&url, &[])
            .map_err(RequestError::into_message)?;
        let data = payload
            .get("data")
            .ok_or_else(|| "B 站视频信息响应缺少数据".to_owned())?;
        let track = track_from_item(data).ok_or_else(|| "B 站视频信息缺少有效 BV 号".to_owned())?;
        if track.bvid != bvid {
            return Err("B 站返回的 BV 号与请求不一致".to_owned());
        }
        Ok(track)
    }

    pub fn recommend(&self) -> Result<Vec<Track>, String> {
        let params = [
            ("rid".to_owned(), "1003".to_owned()),
            ("type".to_owned(), "all".to_owned()),
        ];
        let url = self
            .api_url("/x/web-interface/ranking/v2", &params)
            .map_err(RequestError::into_message)?;
        let payload = self
            .get_json_url(&url, &[])
            .map_err(RequestError::into_message)?;
        let raw_tracks = payload
            .get("data")
            .and_then(|data| data.get("list"))
            .and_then(Value::as_array)
            .ok_or_else(|| "B 站音乐榜单响应缺少数据".to_owned())?;

        let mut seen = HashSet::new();
        let mut tracks = Vec::new();
        for item in raw_tracks {
            if let Some(track) = track_from_item(item)
                && seen.insert(track.bvid.clone())
            {
                tracks.push(track);
            }
        }
        Ok(tracks)
    }

    pub fn resolve(&self, bvid: &str) -> Result<AudioStream, String> {
        if !valid_bvid(bvid) {
            return Err("BV 号格式不正确".to_owned());
        }

        let view_url = self
            .api_url(
                "/x/web-interface/view",
                &[("bvid".to_owned(), bvid.to_owned())],
            )
            .map_err(RequestError::into_message)?;
        let view = self
            .get_json_url(&view_url, &[])
            .map_err(RequestError::into_message)?;
        let view_data = view
            .get("data")
            .and_then(Value::as_object)
            .ok_or_else(|| "B 站视频信息响应缺少数据".to_owned())?;
        let first_page = view_data
            .get("pages")
            .and_then(Value::as_array)
            .and_then(|pages| pages.first())
            .and_then(Value::as_object);

        let cid = view_data
            .get("cid")
            .map(|value| value_to_u64(Some(value)))
            .filter(|value| *value > 0)
            .or_else(|| {
                first_page
                    .and_then(|page| page.get("cid"))
                    .map(|value| value_to_u64(Some(value)))
            })
            .unwrap_or_default();
        if cid == 0 {
            return Err("该视频缺少可播放的分P信息".to_owned());
        }

        let duration = first_page
            .and_then(|page| page.get("duration"))
            .map(value_to_duration)
            .or_else(|| view_data.get("duration").map(value_to_duration))
            .unwrap_or_default();
        let play_url = self
            .api_url(
                "/x/player/playurl",
                &[
                    ("bvid".to_owned(), bvid.to_owned()),
                    ("cid".to_owned(), cid.to_string()),
                    ("fnval".to_owned(), "4048".to_owned()),
                    ("fourk".to_owned(), "1".to_owned()),
                ],
            )
            .map_err(RequestError::into_message)?;
        let play = self
            .get_json_url(&play_url, &[])
            .map_err(RequestError::into_message)?;
        let audio_items = play
            .get("data")
            .and_then(|data| data.get("dash"))
            .and_then(|dash| dash.get("audio"))
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty())
            .ok_or_else(|| "该视频没有可用的普通 DASH AAC 音频".to_owned())?;

        let mut candidates = audio_items
            .iter()
            .filter(|item| is_aac_audio(item))
            .filter_map(|item| {
                let url = candidate_audio_url(item)?;
                let (initialization_range, index_range) = audio_segment_ranges(item);
                Some(AudioCandidate {
                    bandwidth: value_to_u64(item.get("bandwidth")),
                    id: value_to_u64(item.get("id")),
                    url,
                    initialization_range,
                    index_range,
                    codecs: audio_codecs(item),
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .bandwidth
                .cmp(&left.bandwidth)
                .then_with(|| right.id.cmp(&left.id))
        });
        let selected = candidates
            .into_iter()
            .next()
            .ok_or_else(|| "该视频没有可播放的 B 站 AAC 音频源".to_owned())?;
        Ok(AudioStream {
            url: selected.url,
            duration,
            initialization_range: selected.initialization_range,
            index_range: selected.index_range,
            bandwidth: selected.bandwidth,
            codecs: selected.codecs,
        })
    }

    fn api_url(&self, path: &str, params: &[(String, String)]) -> Result<String, RequestError> {
        let raw = format!("{}{}", self.api_base.trim_end_matches('/'), path);
        let mut url = Url::parse(&raw)
            .map_err(|_| RequestError::Data("B 站接口地址格式不正确".to_owned()))?;
        if !params.is_empty() {
            url.query_pairs_mut().extend_pairs(
                params
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            );
        }
        Ok(url.into())
    }

    fn signed_search_url(&self, params: &[(String, String)]) -> Result<String, RequestError> {
        let mixin_key = self.wbi_mixin_key()?;
        let values = params.to_vec();
        let (query, signature) = build_wbi_query(&values, &mixin_key, unix_timestamp());
        let mut url = Url::parse(&self.api_url("/x/web-interface/wbi/search/type", &[])?)
            .map_err(|_| RequestError::Data("B 站搜索地址格式不正确".to_owned()))?;
        url.set_query(Some(&format!("{query}&w_rid={signature}")));
        Ok(url.into())
    }

    fn wbi_mixin_key(&self) -> Result<String, RequestError> {
        {
            let cache = self
                .wbi_cache
                .lock()
                .map_err(|_| RequestError::Data("B 站 WBI 缓存暂时不可用".to_owned()))?;
            if let Some(cached) = cache.as_ref()
                && Instant::now() < cached.expires_at
            {
                return Ok(cached.mixin_key.clone());
            }
        }

        let nav_url = self.api_url("/x/web-interface/nav", &[])?;
        let nav = self.get_json_url(&nav_url, &[-101])?;
        let wbi_img = nav
            .get("data")
            .and_then(|data| data.get("wbi_img"))
            .and_then(Value::as_object)
            .ok_or_else(|| RequestError::Data("无法获取 B 站 WBI 签名信息".to_owned()))?;
        let img_url = safe_wbi_image_url(wbi_img.get("img_url"))?;
        let sub_url = safe_wbi_image_url(wbi_img.get("sub_url"))?;
        let img_key = wbi_key_from_url(&img_url)?;
        let sub_key = wbi_key_from_url(&sub_url)?;
        let raw_key = format!("{img_key}{sub_key}");
        if raw_key.len() < 64 {
            return Err(RequestError::Data("B 站 WBI 签名密钥无效".to_owned()));
        }

        let mixin_key = MIXIN_KEY_ENC_TAB
            .iter()
            .filter_map(|index| raw_key.as_bytes().get(*index).copied())
            .map(char::from)
            .take(32)
            .collect::<String>();
        if mixin_key.len() != 32 {
            return Err(RequestError::Data("B 站 WBI 混合密钥无效".to_owned()));
        }

        let mut cache = self
            .wbi_cache
            .lock()
            .map_err(|_| RequestError::Data("B 站 WBI 缓存暂时不可用".to_owned()))?;
        *cache = Some(WbiCache {
            mixin_key: mixin_key.clone(),
            expires_at: Instant::now() + WBI_CACHE_TTL,
        });
        Ok(mixin_key)
    }

    fn get_json_url(&self, url: &str, allowed_api_codes: &[i64]) -> Result<Value, RequestError> {
        let bytes = self.get_bytes(url)?;
        let payload: Value = serde_json::from_slice(&bytes)
            .map_err(|_| RequestError::Data("B 站返回了无法解析的数据".to_owned()))?;
        let object = payload
            .as_object()
            .ok_or_else(|| RequestError::Data("B 站返回的数据格式不正确".to_owned()))?;

        let code = object
            .get("code")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        if code != 0 && !allowed_api_codes.contains(&code) {
            let message = clean_html_text(
                object
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("未知错误"),
            );
            if matches!(code, -352 | -412 | -799) || is_risk_text(&message) {
                return Err(RequestError::risk());
            }
            return Err(RequestError::Api(format!("B 站接口返回错误：{message}")));
        }
        Ok(payload)
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, RequestError> {
        let may_send_cookie = Url::parse(url).ok().is_some_and(|parsed| {
            parsed.scheme() == "https"
                && parsed.host_str() == Some("api.bilibili.com")
                && parsed.username().is_empty()
                && parsed.port().is_none_or(|port| port == 443)
        });
        let cookie = if may_send_cookie {
            self.account.as_ref().and_then(AccountClient::cookie_header)
        } else {
            None
        };
        let mut request = self
            .client
            .get(url)
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.7")
            .header("Referer", EVENTUAL_REFERER);
        if let Some(cookie) = cookie {
            request = request.header("Cookie", cookie);
        }
        let response = request.send().map_err(|error| {
            if error.is_timeout() {
                RequestError::Network("连接 B 站超时，请检查网络后重试".to_owned())
            } else if error.is_connect() {
                RequestError::Network("无法连接 B 站，请检查网络后重试".to_owned())
            } else {
                RequestError::Network("网络请求失败，请稍后重试".to_owned())
            }
        })?;

        let status = response.status();
        if status == StatusCode::PRECONDITION_FAILED || status == StatusCode::TOO_MANY_REQUESTS {
            return Err(RequestError::risk());
        }
        if let Some(length) = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            && length > MAX_RESPONSE_BYTES as u64
        {
            return Err(RequestError::Data("B 站响应过大，已停止读取".to_owned()));
        }

        let mut limited = response.take((MAX_RESPONSE_BYTES + 1) as u64);
        let mut bytes = Vec::new();
        limited.read_to_end(&mut bytes).map_err(|error| {
            if error.kind() == std::io::ErrorKind::TimedOut {
                RequestError::Network("读取 B 站数据超时，请稍后重试".to_owned())
            } else {
                RequestError::Network("读取 B 站数据失败，请稍后重试".to_owned())
            }
        })?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(RequestError::Data("B 站响应过大，已停止读取".to_owned()));
        }

        if !status.is_success() {
            let detail = String::from_utf8_lossy(&bytes);
            if is_risk_text(&detail) {
                return Err(RequestError::risk());
            }
            let code = status.as_u16();
            let message = if status.is_server_error() {
                "B 站服务暂时不可用，请稍后重试".to_owned()
            } else {
                format!("B 站请求失败（HTTP {code}），请稍后重试")
            };
            return Err(RequestError::Http(message));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    fn mocked_json_sequence(
        bodies: Vec<&'static str>,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        assert!(!bodies.is_empty());
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let address = listener.local_addr().expect("mock server address");
        let (request_tx, request_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            for body in bodies {
                let (mut stream, _) = listener.accept().expect("accept test request");
                let mut reader = BufReader::new(stream.try_clone().expect("clone test stream"));
                let mut request_line = String::new();
                reader
                    .read_line(&mut request_line)
                    .expect("read request line");
                request_tx.send(request_line).expect("send request line");
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).expect("read request header");
                    if header == "\r\n" || header.is_empty() {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write mock response");
            }
        });

        (format!("http://{address}"), request_rx, handle)
    }

    fn mocked_json_server(
        body: &'static str,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        mocked_json_sequence(vec![body])
    }

    fn mocked_status_server(
        status: u16,
        reason: &'static str,
        body: &'static str,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let address = listener.local_addr().expect("mock server address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut reader = BufReader::new(stream.try_clone().expect("clone test stream"));
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).expect("read request header");
                if header == "\r\n" || header.is_empty() {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write mock response");
        });
        (format!("http://{address}"), handle)
    }

    fn mocked_oversized_response_server(
        declared_length: usize,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let address = listener.local_addr().expect("mock server address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut reader = BufReader::new(stream.try_clone().expect("clone test stream"));
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).expect("read request header");
                if header == "\r\n" || header.is_empty() {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {declared_length}\r\nConnection: close\r\n\r\n"
            );
            stream
                .write_all(response.as_bytes())
                .expect("write oversized response header");
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn validates_bvid_and_audio_urls() {
        assert!(valid_bvid("BV1xx411c7mD"));
        assert!(!valid_bvid("BV1xx411c7m"));
        assert!(!valid_bvid("https://www.bilibili.com/video/BV1xx411c7mD"));
        assert_eq!(
            direct_bvid("  BV1xx411c7mD  "),
            Some("BV1xx411c7mD".to_owned())
        );
        assert_eq!(
            direct_bvid("https://www.bilibili.com/video/BV1xx411c7mD/?p=1&vd_source=x"),
            Some("BV1xx411c7mD".to_owned())
        );
        assert_eq!(direct_bvid("https://evil.example/video/BV1xx411c7mD"), None);

        assert!(valid_audio_url(
            "https://upos-sz-mirrorcos.bilivideo.com/audio.m4s?token=1"
        ));
        assert!(valid_audio_url(
            "https://xy12x34.mcdn.bilivideo.cn/audio.m4s"
        ));
        assert!(!valid_audio_url(
            "http://upos-sz-mirrorcos.bilivideo.com/audio.m4s"
        ));
        assert!(!valid_audio_url("https://evil.example/audio.m4s"));
        assert!(!valid_audio_url(
            "https://upos-sz-mirrorcos.bilivideo.com.evil.example/audio.m4s"
        ));
    }

    #[test]
    fn cleans_html_controls_and_clock_duration() {
        assert_eq!(clean_html_text("<em>夜</em>空\u{0} &amp; 星"), "夜空 & 星");
        assert_eq!(parse_duration_seconds("02:05"), 125);
        assert_eq!(parse_duration_seconds("1:02:03"), 3723);
        assert_eq!(parse_duration_seconds("bad"), 0);
        assert!(!page_has_more(50, 100, 2000, 20));
        assert!(page_has_more(49, 100, 2000, 20));
    }

    #[test]
    fn direct_video_url_uses_view_and_returns_one_track() {
        let (base, request_rx, handle) = mocked_json_server(
            r#"{
                "code": 0,
                "data": {
                    "bvid": "BV1xx411c7mD",
                    "title": "直达音乐",
                    "owner": {"name": "视频UP"},
                    "pic": "//i0.hdslb.com/direct.jpg",
                    "duration": 212,
                    "stat": {"view": 123}
                }
            }"#,
        );
        let client = BiliClient::with_api_base(base).expect("create test client");

        let result = client
            .search("https://www.bilibili.com/video/BV1xx411c7mD?p=1", 1)
            .expect("resolve direct URL");

        assert_eq!(
            result,
            SearchResult {
                tracks: vec![Track {
                    id: "BV1xx411c7mD".to_owned(),
                    bvid: "BV1xx411c7mD".to_owned(),
                    title: "直达音乐".to_owned(),
                    artist: "视频UP".to_owned(),
                    cover: "https://i0.hdslb.com/direct.jpg".to_owned(),
                    duration: 212,
                    play_count: 123,
                }],
                has_more: false,
            }
        );
        let request = request_rx.recv().expect("request line");
        assert!(request.contains("/x/web-interface/view"));
        assert!(!request.contains("/search/type"));
        handle.join().expect("mock server should finish");
    }

    #[test]
    fn recommend_uses_standard_library_mock_server() {
        let (base, request_rx, handle) = mocked_json_server(
            r#"{
                "code": 0,
                "data": {
                    "list": [{
                        "bvid": "BV1xx411c7mD",
                        "title": "A\u0000<em>B</em> &amp; C",
                        "owner": {"name": "音乐UP"},
                        "pic": "http://i0.hdslb.com/cover.jpg",
                        "duration": "03:04",
                        "stat": {"view": 9876}
                    }]
                }
            }"#,
        );
        let client = BiliClient::with_api_base(base).expect("create test client");

        let tracks = client.recommend().expect("recommend from mock");

        assert_eq!(
            tracks,
            vec![Track {
                id: "BV1xx411c7mD".to_owned(),
                bvid: "BV1xx411c7mD".to_owned(),
                title: "AB & C".to_owned(),
                artist: "音乐UP".to_owned(),
                cover: "https://i0.hdslb.com/cover.jpg".to_owned(),
                duration: 184,
                play_count: 9876,
            }]
        );
        assert!(
            request_rx
                .recv()
                .expect("request line")
                .contains("rid=1003")
        );
        handle.join().expect("mock server should finish");
    }

    #[test]
    fn keyword_search_adds_music_filter_on_every_page() {
        for page in [1, 2] {
            let (base, request_rx, handle) =
                mocked_json_server(r#"{"code":0,"data":{"result":[],"numResults":0}}"#);
            let client = BiliClient::with_api_base(base).unwrap();
            *client.wbi_cache.lock().unwrap() = Some(WbiCache {
                mixin_key: "a".repeat(32),
                expires_at: Instant::now() + Duration::from_secs(60),
            });
            client.search("周杰伦", page).unwrap();
            let request = request_rx.recv().unwrap();
            let path = request.split_whitespace().nth(1).unwrap();
            let url = Url::parse(&format!("https://api.bilibili.com{path}")).unwrap();
            let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(pairs["keyword"], "周杰伦 音乐");
            assert_eq!(pairs["page"], page.to_string());
            handle.join().unwrap();
        }
        assert_eq!(music_search_keyword("  纯音乐  "), "纯音乐");
        assert_eq!(music_search_keyword("摇滚 音乐"), "摇滚 音乐");
    }

    #[test]
    fn signed_search_url_contains_encoded_parameters_and_w_rid() {
        let client = BiliClient::with_api_base("https://api.bilibili.com".to_owned())
            .expect("create client");
        {
            let mut cache = client.wbi_cache.lock().expect("lock WBI cache");
            *cache = Some(WbiCache {
                mixin_key: "a".repeat(32),
                expires_at: Instant::now() + Duration::from_secs(60),
            });
        }
        let params = vec![
            ("keyword".to_owned(), "中文 & 音乐".to_owned()),
            ("page".to_owned(), "2".to_owned()),
        ];

        let signed = client.signed_search_url(&params).expect("sign search URL");
        let parsed = Url::parse(&signed).expect("parse signed URL");
        let pairs = parsed
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(
            pairs.get("keyword").map(String::as_str),
            Some("中文 & 音乐")
        );
        assert_eq!(pairs.get("page").map(String::as_str), Some("2"));
        assert!(pairs.contains_key("wts"));
        assert_eq!(pairs.get("w_rid").map(|value| value.len()), Some(32));
        assert!(!signed.contains("query="));
    }

    #[test]
    fn wbi_signature_matches_known_vector() {
        let params = vec![
            ("keyword".to_owned(), "音乐".to_owned()),
            ("search_type".to_owned(), "video".to_owned()),
            ("page".to_owned(), "1".to_owned()),
            ("page_size".to_owned(), "20".to_owned()),
            ("order".to_owned(), "totalrank".to_owned()),
        ];

        let (query, signature) = build_wbi_query(&params, &"a".repeat(32), 1_700_000_000);

        assert_eq!(
            query,
            "keyword=%E9%9F%B3%E4%B9%90&order=totalrank&page=1&\
             page_size=20&search_type=video&wts=1700000000"
        );
        assert_eq!(signature, "03117f46fd77fc1a5c9fbf9c57925c03");
    }

    #[test]
    fn resolve_uses_first_page_cid_and_highest_valid_ordinary_aac() {
        let (base, requests, handle) = mocked_json_sequence(vec![
            r#"{
                "code": 0,
                "data": {
                    "bvid": "BV1xx411c7mD",
                    "duration": 999,
                    "pages": [
                        {"cid": 101, "duration": 321},
                        {"cid": 202, "duration": 654}
                    ]
                }
            }"#,
            r#"{
                "code": 0,
                "data": {
                    "dash": {
                        "audio": [
                            {
                                "id": 30216,
                                "bandwidth": 64000,
                                "codecs": "mp4a.40.2",
                                "baseUrl": "https://upos-sz-mirrorcos.bilivideo.com/low.m4s"
                            },
                            {
                                "id": 30280,
                                "bandwidth": 192000,
                                "codecs": "mp4a.40.2",
                                "baseUrl": "https://upos-sz-mirrorcos.bilivideo.com/selected.m4s",
                                "SegmentBase": {
                                    "Initialization": "0-767",
                                    "indexRange": "768-1023"
                                }
                            },
                            {
                                "id": 30250,
                                "bandwidth": 400000,
                                "codecs": "ec-3",
                                "baseUrl": "https://upos-sz-mirrorcos.bilivideo.com/dolby.m4s"
                            },
                            {
                                "id": 30290,
                                "bandwidth": 900000,
                                "codecs": "mp4a.40.2",
                                "baseUrl": "https://evil.example/illegal.m4s"
                            }
                        ],
                        "flac": {
                            "audio": {
                                "id": 30251,
                                "bandwidth": 999999,
                                "codecs": "fLaC",
                                "baseUrl": "https://upos-sz-mirrorcos.bilivideo.com/flac.m4s"
                            }
                        }
                    }
                }
            }"#,
        ]);
        let client = BiliClient::with_api_base(base).expect("create test client");

        let stream = client
            .resolve("BV1xx411c7mD")
            .expect("resolve mocked video");

        assert_eq!(
            stream,
            AudioStream {
                url: "https://upos-sz-mirrorcos.bilivideo.com/selected.m4s".to_owned(),
                duration: 321,
                initialization_range: "0-767".to_owned(),
                index_range: "768-1023".to_owned(),
                bandwidth: 192000,
                codecs: "mp4a.40.2".to_owned(),
            }
        );
        let view_request = requests.recv().expect("view request line");
        let play_request = requests.recv().expect("playurl request line");
        assert!(view_request.contains("/x/web-interface/view"));
        assert!(view_request.contains("bvid=BV1xx411c7mD"));
        assert!(play_request.contains("/x/player/playurl"));
        assert!(play_request.contains("cid=101"));
        assert!(!play_request.contains("cid=202"));
        assert!(play_request.contains("fnval=4048"));
        handle.join().expect("mock server should finish");
    }

    #[test]
    fn http_412_and_429_return_friendly_risk_errors() {
        for (status, reason) in [
            (412_u16, "Precondition Failed"),
            (429_u16, "Too Many Requests"),
        ] {
            let (base, handle) = mocked_status_server(status, reason, "blocked");
            let client = BiliClient::with_api_base(base).expect("create test client");

            let error = client.recommend().expect_err("HTTP risk status must fail");

            assert!(error.contains("访问限制"), "unexpected error: {error}");
            assert!(error.contains("稍后再试"), "unexpected error: {error}");
            handle.join().expect("mock server should finish");
        }
    }

    #[test]
    fn business_risk_code_returns_friendly_error() {
        let (base, _requests, handle) =
            mocked_json_server(r#"{"code":-352,"message":"触发风控，请求被拦截","data":null}"#);
        let client = BiliClient::with_api_base(base).expect("create test client");

        let error = client
            .recommend()
            .expect_err("business risk code must fail");

        assert!(error.contains("访问限制"), "unexpected error: {error}");
        assert!(error.contains("稍后再试"), "unexpected error: {error}");
        handle.join().expect("mock server should finish");
    }

    #[test]
    fn invalid_json_returns_friendly_data_error() {
        let (base, _requests, handle) = mocked_json_server(r#"{"code":"#);
        let client = BiliClient::with_api_base(base).expect("create test client");

        let error = client.recommend().expect_err("invalid JSON must fail");

        assert!(error.contains("无法解析"), "unexpected error: {error}");
        handle.join().expect("mock server should finish");
    }

    #[test]
    fn oversized_json_response_is_rejected_before_body_read() {
        let (base, handle) = mocked_oversized_response_server(MAX_RESPONSE_BYTES + 1);
        let client = BiliClient::with_api_base(base).expect("create test client");

        let error = client
            .recommend()
            .expect_err("oversized response must fail");

        assert!(error.contains("响应过大"), "unexpected error: {error}");
        handle.join().expect("mock server should finish");
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;
    #[test]
    fn thumbnail_uses_small_public_cdn_image() {
        assert_eq!(
            cover_thumbnail_url("//i0.hdslb.com/bfs/archive/a.jpg@640w.jpg?x=1")
                .unwrap()
                .as_str(),
            "https://i0.hdslb.com/bfs/archive/a.jpg@160w_160h_1c.jpg"
        );
        for source in [
            "https://hdslb.com.evil.test/a.jpg",
            "https://localhost/a.jpg",
            "https://user@i0.hdslb.com/a.jpg",
            "https://i0.hdslb.com:8443/a.jpg",
        ] {
            assert!(cover_thumbnail_url(source).is_err());
        }
    }
}
