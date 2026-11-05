use crate::api::{Track, USER_AGENT};
use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const API_ORIGIN: &str = "https://api.bilibili.com";
const PASSPORT_ORIGIN: &str = "https://passport.bilibili.com";
const WEB_REFERER: &str = "https://www.bilibili.com/";
const NETWORK_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SESSION_BYTES: usize = 64 * 1024;
const MAX_COOKIE_BYTES: usize = 32 * 1024;
const PAGE_SIZE: u32 = 20;
const MAX_PAGE: u32 = 50;
const MAX_TEXT_CHARS: usize = 256;
const MAX_QR_KEY_CHARS: usize = 512;
const COOKIE_WHITELIST: [&str; 8] = [
    "SESSDATA",
    "bili_jct",
    "DedeUserID",
    "DedeUserID__ckMd5",
    "sid",
    "buvid3",
    "buvid4",
    "b_nut",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountProfile {
    pub mid: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrCode {
    pub url: String,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginPoll {
    Waiting,
    Scanned,
    Expired,
    Success(AccountProfile),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub id: u64,
    pub title: String,
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteTracks {
    pub tracks: Vec<Track>,
    pub has_more: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Session {
    cookies: BTreeMap<String, String>,
    csrf_token: String,
}

#[derive(Clone, Copy)]
struct MediaMetadata {
    aid: u64,
    cid: u64,
    duration: u64,
}

#[derive(Clone)]
struct HistoryCursor {
    max: u64,
    view_at: u64,
    business: String,
}

#[derive(Default)]
struct HistoryCache {
    pages: HashMap<u32, RemoteTracks>,
    cursors: HashMap<u32, HistoryCursor>,
}

#[derive(Clone)]
pub struct AccountClient {
    session_path: Arc<PathBuf>,
    session: Arc<Mutex<Option<Session>>>,
    client: Client,
    api_base: String,
    passport_base: String,
    media_cache: Arc<Mutex<HashMap<String, MediaMetadata>>>,
    history_cache: Arc<Mutex<HistoryCache>>,
    history_lock: Arc<Mutex<()>>,
    report_lock: Arc<Mutex<()>>,
    generation: Arc<AtomicU64>,
    #[cfg(test)]
    test_cookie_host: Option<String>,
}

struct JsonResponse {
    payload: Value,
    set_cookies: Vec<String>,
}

impl AccountClient {
    pub fn new(session_path: PathBuf) -> Result<Self, String> {
        if session_path.as_os_str().is_empty() {
            return Err("账号会话路径不能为空".to_owned());
        }
        let session = load_session(&session_path).unwrap_or(None);
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(NETWORK_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| "无法初始化账号网络客户端".to_owned())?;
        Ok(Self {
            session_path: Arc::new(session_path),
            session: Arc::new(Mutex::new(session)),
            client,
            api_base: API_ORIGIN.to_owned(),
            passport_base: PASSPORT_ORIGIN.to_owned(),
            media_cache: Arc::new(Mutex::new(HashMap::new())),
            history_cache: Arc::new(Mutex::new(HistoryCache::default())),
            history_lock: Arc::new(Mutex::new(())),
            report_lock: Arc::new(Mutex::new(())),
            generation: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            test_cookie_host: None,
        })
    }

    pub fn profile(&self) -> Result<Option<AccountProfile>, String> {
        let generation = self.generation.load(Ordering::SeqCst);
        let Some(session) = self.session_snapshot()? else {
            return Ok(None);
        };
        let profile = self.nav_profile(&session)?;
        if profile.is_none() {
            self.clear_session_if_generation(generation)?;
        }
        Ok(profile)
    }

    pub fn generate_qr(&self) -> Result<QrCode, String> {
        let url = self.passport_url("/x/passport-login/web/qrcode/generate", &[])?;
        let response = self.get_json(&url, None)?;
        let data = checked_api_data(&response.payload)?;
        let login_url = data
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| "二维码登录地址缺失".to_owned())?;
        let url = validate_qr_url(login_url)?;
        let key = data
            .get("qrcode_key")
            .and_then(Value::as_str)
            .ok_or_else(|| "二维码登录密钥缺失".to_owned())?;
        let key = validate_qr_key(key)?;
        Ok(QrCode { url, key })
    }

    pub fn poll_qr(&self, key: &str) -> Result<LoginPoll, String> {
        let key = validate_qr_key(key)?;
        let generation = self.generation.load(Ordering::SeqCst);
        let url = self.passport_url(
            "/x/passport-login/web/qrcode/poll",
            &[("qrcode_key".to_owned(), key)],
        )?;
        let response = self.get_json(&url, None)?;
        checked_api_success(&response.payload)?;
        let data = response
            .payload
            .get("data")
            .and_then(Value::as_object)
            .ok_or_else(|| "二维码登录响应缺少数据".to_owned())?;
        let status = data.get("code").and_then(Value::as_i64).unwrap_or_default();
        match status {
            86101 => Ok(LoginPoll::Waiting),
            86090 => Ok(LoginPoll::Scanned),
            86038 => Ok(LoginPoll::Expired),
            0 => {
                let session = self.session_from_cookies(&response.set_cookies)?;
                let profile = self
                    .nav_profile(&session)?
                    .ok_or_else(|| "扫码登录成功，但账号会话校验失败".to_owned())?;
                let mut session = session;
                session
                    .cookies
                    .insert("DedeUserID".to_owned(), profile.mid.to_string());
                let mut current = self
                    .session
                    .lock()
                    .map_err(|_| "账号会话暂时不可用".to_owned())?;
                if self
                    .generation
                    .compare_exchange(
                        generation,
                        generation.wrapping_add(1),
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    )
                    .is_err()
                {
                    return Err("登录流程已取消，请重新生成二维码".to_owned());
                }
                self.persist_session(&session)?;
                *current = Some(session);
                Ok(LoginPoll::Success(profile))
            }
            _ => Err(format!("二维码登录状态异常：{status}")),
        }
    }

    pub fn logout(&self) -> Result<(), String> {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut session = self
            .session
            .lock()
            .map_err(|_| "账号会话暂时不可用".to_owned())?;
        *session = None;
        match fs::remove_file(self.session_path.as_ref()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("删除本机登录会话失败".to_owned()),
        }
    }

    pub fn is_logged_in(&self) -> bool {
        self.session
            .lock()
            .ok()
            .and_then(|session| session.as_ref().map(Session::has_login_cookies))
            .unwrap_or(false)
    }

    pub fn cookie_header(&self) -> Option<String> {
        self.session
            .lock()
            .ok()
            .and_then(|session| session.as_ref().and_then(Session::cookie_header))
    }

    pub fn folders(&self) -> Result<Vec<Folder>, String> {
        let session = self.require_session()?;
        let mid = if let Some(mid) = session.cookie_u64("DedeUserID") {
            mid
        } else {
            self.profile()?
                .ok_or_else(|| "无法确定当前登录账号".to_owned())?
                .mid
        };
        let url = self.api_url(
            "/x/v3/fav/folder/created/list-all",
            &[
                ("up_mid".to_owned(), mid.to_string()),
                ("type".to_owned(), "2".to_owned()),
            ],
        )?;
        let response = self.get_json(&url, Some(&session))?;
        let data = checked_api_data(&response.payload)?;
        let list = data
            .get("list")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        Ok(list.iter().filter_map(folder_from_item).collect())
    }

    pub fn folder_tracks(&self, folder_id: u64, page: u32) -> Result<RemoteTracks, String> {
        if folder_id == 0 {
            return Err("收藏夹 ID 无效".to_owned());
        }
        validate_page(page)?;
        let session = self.require_session()?;
        let url = self.api_url(
            "/x/v3/fav/resource/list",
            &[
                ("media_id".to_owned(), folder_id.to_string()),
                ("pn".to_owned(), page.to_string()),
                ("ps".to_owned(), PAGE_SIZE.to_string()),
                ("keyword".to_owned(), String::new()),
                ("order".to_owned(), "mtime".to_owned()),
                ("type".to_owned(), "0".to_owned()),
                ("tid".to_owned(), "0".to_owned()),
                ("platform".to_owned(), "web".to_owned()),
            ],
        )?;
        let response = self.get_json(&url, Some(&session))?;
        let data = checked_api_data(&response.payload)?;
        let tracks = data
            .get("medias")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(crate::api::track_from_item)
                    .collect()
            })
            .unwrap_or_default();
        Ok(RemoteTracks {
            tracks,
            has_more: data
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    pub fn history(&self, page: u32) -> Result<RemoteTracks, String> {
        validate_page(page)?;
        let _history_guard = self
            .history_lock
            .lock()
            .map_err(|_| "历史记录缓存暂时不可用".to_owned())?;
        if page == 1 {
            self.history_cache
                .lock()
                .map_err(|_| "历史记录缓存暂时不可用".to_owned())?
                .pages
                .clear();
            self.history_cache
                .lock()
                .map_err(|_| "历史记录缓存暂时不可用".to_owned())?
                .cursors
                .clear();
        }
        let session = self.require_session()?;

        for current_page in 1..=page {
            if let Some(cached) = self
                .history_cache
                .lock()
                .map_err(|_| "历史记录缓存暂时不可用".to_owned())?
                .pages
                .get(&current_page)
                .cloned()
            {
                if current_page == page {
                    return Ok(cached);
                }
                continue;
            }
            let cursor = if current_page == 1 {
                HistoryCursor {
                    max: 0,
                    view_at: 0,
                    business: "archive".to_owned(),
                }
            } else {
                let Some(cursor) = self
                    .history_cache
                    .lock()
                    .map_err(|_| "历史记录缓存暂时不可用".to_owned())?
                    .cursors
                    .get(&(current_page - 1))
                    .cloned()
                else {
                    return Ok(RemoteTracks {
                        tracks: Vec::new(),
                        has_more: false,
                    });
                };
                cursor
            };
            let url = self.api_url(
                "/x/web-interface/history/cursor",
                &[
                    ("max".to_owned(), cursor.max.to_string()),
                    ("view_at".to_owned(), cursor.view_at.to_string()),
                    ("business".to_owned(), cursor.business),
                    ("ps".to_owned(), PAGE_SIZE.to_string()),
                ],
            )?;
            let response = self.get_json(&url, Some(&session))?;
            let data = checked_api_data(&response.payload)?;
            let list = data
                .get("list")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let cursor = data.get("cursor").and_then(Value::as_object);
            let is_end = cursor
                .and_then(|cursor| cursor.get("is_end"))
                .and_then(Value::as_bool)
                .unwrap_or(list.len() < PAGE_SIZE as usize);
            let page_result = RemoteTracks {
                tracks: list
                    .iter()
                    .filter_map(crate::api::track_from_item)
                    .collect(),
                has_more: !is_end,
            };
            let next_cursor = cursor.map(|cursor| HistoryCursor {
                max: value_u64(cursor.get("max")),
                view_at: value_u64(cursor.get("view_at")),
                business: cursor
                    .get("business")
                    .and_then(Value::as_str)
                    .map(clean_text)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "archive".to_owned()),
            });
            {
                let mut cache = self
                    .history_cache
                    .lock()
                    .map_err(|_| "历史记录缓存暂时不可用".to_owned())?;
                cache.pages.insert(current_page, page_result.clone());
                if is_end {
                    cache.cursors.remove(&current_page);
                } else if let Some(next_cursor) = next_cursor.clone() {
                    cache.cursors.insert(current_page, next_cursor);
                }
            }
            if current_page == page {
                return Ok(page_result);
            }
            if is_end || next_cursor.is_none() {
                return Ok(RemoteTracks {
                    tracks: Vec::new(),
                    has_more: false,
                });
            }
        }
        Ok(RemoteTracks {
            tracks: Vec::new(),
            has_more: false,
        })
    }

    pub fn favorite(&self, bvid: &str, folder_id: u64) -> Result<(), String> {
        if !crate::api::valid_bvid(bvid) {
            return Err("BV 号格式不正确".to_owned());
        }
        if folder_id == 0 {
            return Err("收藏夹 ID 无效".to_owned());
        }
        let session = self.require_session()?;
        let csrf = session.csrf()?;
        let metadata = self.media_metadata(bvid, &session)?;
        let url = self.api_url("/x/v3/fav/resource/deal", &[])?;
        let response = self.post_form_json(
            &url,
            &session,
            &[
                ("rid".to_owned(), metadata.aid.to_string()),
                ("type".to_owned(), "2".to_owned()),
                ("add_media_ids".to_owned(), folder_id.to_string()),
                ("del_media_ids".to_owned(), String::new()),
                ("csrf".to_owned(), csrf),
            ],
        )?;
        checked_api_success(&response.payload)
    }

    pub fn report(&self, bvid: &str, position: u64) -> Result<(), String> {
        if !crate::api::valid_bvid(bvid) {
            return Err("BV 号格式不正确".to_owned());
        }
        let _report_guard = self
            .report_lock
            .lock()
            .map_err(|_| "历史上报队列暂时不可用".to_owned())?;
        let session = self.require_session()?;
        let csrf = session.csrf()?;
        let mid = if let Some(mid) = session.cookie_u64("DedeUserID") {
            mid
        } else {
            self.nav_profile(&session)?
                .ok_or_else(|| "无法确定当前登录账号".to_owned())?
                .mid
        };
        let metadata = self.media_metadata(bvid, &session)?;
        let played_time = position.min(metadata.duration);
        let url = self.api_url("/x/click-interface/web/heartbeat", &[])?;
        let response = self.post_form_json(
            &url,
            &session,
            &[
                ("aid".to_owned(), metadata.aid.to_string()),
                ("cid".to_owned(), metadata.cid.to_string()),
                ("mid".to_owned(), mid.to_string()),
                ("csrf".to_owned(), csrf),
                ("played_time".to_owned(), played_time.to_string()),
                ("type".to_owned(), "3".to_owned()),
                ("sub_type".to_owned(), "0".to_owned()),
                ("video_duration".to_owned(), metadata.duration.to_string()),
                ("is_visible".to_owned(), "1".to_owned()),
                ("play_type".to_owned(), "0".to_owned()),
            ],
        )?;
        checked_api_success(&response.payload)
    }

    fn media_metadata(&self, bvid: &str, session: &Session) -> Result<MediaMetadata, String> {
        if let Ok(cache) = self.media_cache.lock()
            && let Some(metadata) = cache.get(bvid)
        {
            return Ok(*metadata);
        }
        let url = self.api_url(
            "/x/web-interface/view",
            &[("bvid".to_owned(), bvid.to_owned())],
        )?;
        let response = self.get_json(&url, Some(session))?;
        let data = checked_api_data(&response.payload)?;
        let aid = value_u64(data.get("aid"));
        let cid = value_u64(data.get("cid")).max(
            data.get("pages")
                .and_then(Value::as_array)
                .and_then(|pages| pages.first())
                .and_then(|page| page.get("cid"))
                .map(|value| value_u64(Some(value)))
                .unwrap_or_default(),
        );
        let duration = value_u64(data.get("duration")).max(
            data.get("pages")
                .and_then(Value::as_array)
                .and_then(|pages| pages.first())
                .and_then(|page| page.get("duration"))
                .map(|value| value_u64(Some(value)))
                .unwrap_or_default(),
        );
        if aid == 0 || cid == 0 {
            return Err("视频信息缺少 aid 或 cid".to_owned());
        }
        let metadata = MediaMetadata { aid, cid, duration };
        if let Ok(mut cache) = self.media_cache.lock() {
            if cache.len() >= 512 {
                cache.clear();
            }
            cache.insert(bvid.to_owned(), metadata);
        }
        Ok(metadata)
    }

    fn nav_profile(&self, session: &Session) -> Result<Option<AccountProfile>, String> {
        let url = self.api_url("/x/web-interface/nav", &[])?;
        let response = self.get_json(&url, Some(session))?;
        let code = response
            .payload
            .get("code")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        if code == -101 {
            return Ok(None);
        }
        checked_api_success(&response.payload)?;
        let data = response
            .payload
            .get("data")
            .and_then(Value::as_object)
            .ok_or_else(|| "账号信息响应缺少数据".to_owned())?;
        if !data
            .get("isLogin")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(None);
        }
        let mid = value_u64(data.get("mid"));
        if mid == 0 {
            return Err("账号信息缺少用户 ID".to_owned());
        }
        let name = clean_text(
            data.get("uname")
                .and_then(Value::as_str)
                .unwrap_or("B站用户"),
        );
        Ok(Some(AccountProfile {
            mid,
            name: if name.is_empty() {
                "B站用户".to_owned()
            } else {
                name
            },
        }))
    }

    fn require_session(&self) -> Result<Session, String> {
        self.session_snapshot()?
            .filter(Session::has_login_cookies)
            .ok_or_else(|| "请先扫码登录 B 站账号".to_owned())
    }

    fn session_snapshot(&self) -> Result<Option<Session>, String> {
        self.session
            .lock()
            .map(|session| session.clone())
            .map_err(|_| "账号会话暂时不可用".to_owned())
    }

    fn clear_session_if_generation(&self, generation: u64) -> Result<bool, String> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| "账号会话暂时不可用".to_owned())?;
        if self.generation.load(Ordering::SeqCst) != generation {
            return Ok(false);
        }
        *session = None;
        match fs::remove_file(self.session_path.as_ref()) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(_) => Err("删除失效的本机会话失败".to_owned()),
        }
    }

    fn session_from_cookies(&self, raw_cookies: &[String]) -> Result<Session, String> {
        let mut cookies = BTreeMap::new();
        for raw in raw_cookies {
            let Some((name, value)) = raw.split_once(';').unwrap_or((raw, "")).0.split_once('=')
            else {
                continue;
            };
            let Some(canonical) = canonical_cookie_name(name) else {
                continue;
            };
            let value = value.trim();
            if !valid_cookie_value(value) {
                return Err("登录响应包含无效 Cookie".to_owned());
            }
            cookies.insert(canonical.to_owned(), value.to_owned());
        }
        let csrf = cookies
            .get("bili_jct")
            .cloned()
            .ok_or_else(|| "登录响应缺少 bili_jct".to_owned())?;
        if cookies.get("SESSDATA").is_none_or(|value| value.is_empty()) {
            return Err("登录响应缺少 SESSDATA".to_owned());
        }
        let session = Session {
            cookies,
            csrf_token: csrf,
        };
        if session
            .cookie_header()
            .is_none_or(|header| header.len() > MAX_COOKIE_BYTES)
        {
            return Err("账号 Cookie 大小超出限制".to_owned());
        }
        Ok(session)
    }

    fn persist_session(&self, session: &Session) -> Result<(), String> {
        let data = serde_json::to_vec(session).map_err(|_| "无法序列化本机会话".to_owned())?;
        if data.len() > MAX_SESSION_BYTES {
            return Err("本机会话数据过大".to_owned());
        }
        let path = self.session_path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| "无法创建账号会话目录".to_owned())?;
        }
        #[cfg(unix)]
        if fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
        {
            return Err("本机会话路径不是普通文件".to_owned());
        }
        let temp_path = path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            unix_timestamp_nanos()
        ));
        let result = (|| -> Result<(), String> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options
                .open(&temp_path)
                .map_err(|_| "无法创建本机会话文件".to_owned())?;
            #[cfg(unix)]
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| "无法保护本机会话临时文件权限".to_owned())?;
            file.write_all(&data)
                .map_err(|_| "无法写入本机会话文件".to_owned())?;
            file.sync_all()
                .map_err(|_| "无法同步本机会话文件".to_owned())?;
            drop(file);
            fs::rename(&temp_path, path).map_err(|_| "无法替换本机会话文件".to_owned())?;
            #[cfg(unix)]
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .map_err(|_| "无法保护本机会话文件权限".to_owned())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result
    }

    fn api_url(&self, path: &str, params: &[(String, String)]) -> Result<String, String> {
        build_url(&self.api_base, path, params)
    }

    fn passport_url(&self, path: &str, params: &[(String, String)]) -> Result<String, String> {
        build_url(&self.passport_base, path, params)
    }

    fn get_json(&self, url: &str, session: Option<&Session>) -> Result<JsonResponse, String> {
        let request = self
            .client
            .get(url)
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.7")
            .header("Referer", WEB_REFERER);
        self.send_json(self.with_cookie(request, url, session), session.is_some())
    }

    fn post_form_json(
        &self,
        url: &str,
        session: &Session,
        form: &[(String, String)],
    ) -> Result<JsonResponse, String> {
        let request = self
            .client
            .post(url)
            .header("Accept", "application/json, text/plain, */*")
            .header("Referer", WEB_REFERER)
            .header("Origin", "https://www.bilibili.com")
            .form(form);
        self.send_json(self.with_cookie(request, url, Some(session)), true)
    }

    fn with_cookie(
        &self,
        mut request: RequestBuilder,
        url: &str,
        session: Option<&Session>,
    ) -> RequestBuilder {
        if !self.cookie_target_allowed(url) {
            return request;
        }
        if let Some(cookie) = session.and_then(Session::cookie_header) {
            request = request.header("Cookie", cookie);
        }
        request
    }

    fn cookie_target_allowed(&self, value: &str) -> bool {
        let Ok(url) = Url::parse(value) else {
            return false;
        };
        let host = url.host_str().unwrap_or_default();
        #[cfg(test)]
        if self
            .test_cookie_host
            .as_deref()
            .is_some_and(|allowed| host == allowed)
        {
            return true;
        }
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none_or(|port| port == 443)
            && (host == "bilibili.com" || host.ends_with(".bilibili.com"))
    }

    fn send_json(
        &self,
        request: RequestBuilder,
        _used_session: bool,
    ) -> Result<JsonResponse, String> {
        let response = request.send().map_err(network_error)?;
        let status = response.status();
        if status == StatusCode::PRECONDITION_FAILED || status == StatusCode::TOO_MANY_REQUESTS {
            return Err("B 站暂时触发了访问限制，请稍后再试".to_owned());
        }
        let set_cookies = response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if let Some(length) = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            && length > MAX_RESPONSE_BYTES as u64
        {
            return Err("B 站响应过大，已停止读取".to_owned());
        }
        let mut limited = response.take((MAX_RESPONSE_BYTES + 1) as u64);
        let mut bytes = Vec::new();
        limited
            .read_to_end(&mut bytes)
            .map_err(|_| "读取 B 站账号数据失败，请稍后重试".to_owned())?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err("B 站响应过大，已停止读取".to_owned());
        }
        if !status.is_success() {
            let detail = String::from_utf8_lossy(&bytes);
            if is_risk_text(&detail) {
                return Err("B 站暂时触发了访问限制，请稍后再试".to_owned());
            }
            return Err(if status.is_server_error() {
                "B 站服务暂时不可用，请稍后重试".to_owned()
            } else {
                format!("B 站账号请求失败（HTTP {}）", status.as_u16())
            });
        }
        let payload = serde_json::from_slice(&bytes)
            .map_err(|_| "B 站返回了无法解析的账号数据".to_owned())?;
        Ok(JsonResponse {
            payload,
            set_cookies,
        })
    }
}

impl Session {
    fn has_login_cookies(&self) -> bool {
        self.cookies
            .get("SESSDATA")
            .is_some_and(|value| !value.is_empty())
            && self.csrf().is_ok()
    }

    fn cookie_header(&self) -> Option<String> {
        if self
            .cookies
            .get("SESSDATA")
            .is_none_or(|value| value.is_empty())
        {
            return None;
        }
        let header = self
            .cookies
            .iter()
            .filter(|(name, value)| {
                canonical_cookie_name(name).is_some() && valid_cookie_value(value)
            })
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        (!header.is_empty() && header.len() <= MAX_COOKIE_BYTES).then_some(header)
    }

    fn cookie_u64(&self, name: &str) -> Option<u64> {
        self.cookies.get(name)?.trim().parse().ok()
    }

    fn csrf(&self) -> Result<String, String> {
        let csrf = self
            .cookies
            .get("bili_jct")
            .cloned()
            .or_else(|| (!self.csrf_token.is_empty()).then(|| self.csrf_token.clone()))
            .filter(|token| valid_cookie_value(token))
            .ok_or_else(|| "账号缺少 CSRF 令牌".to_owned())?;
        Ok(csrf)
    }
}

fn load_session(path: &PathBuf) -> Result<Option<Session>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取本机会话文件".to_owned()),
    };
    if !metadata.is_file() || metadata.len() > MAX_SESSION_BYTES as u64 {
        return Err("本机会话文件无效或过大".to_owned());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|_| "无法修正本机会话文件权限".to_owned())?;
    }
    let bytes = fs::read(path).map_err(|_| "无法读取本机会话文件".to_owned())?;
    if bytes.len() > MAX_SESSION_BYTES {
        return Err("本机会话文件过大".to_owned());
    }
    let raw: Session =
        serde_json::from_slice(&bytes).map_err(|_| "本机会话文件格式无效".to_owned())?;
    let mut cookies = BTreeMap::new();
    for (name, value) in raw.cookies {
        let Some(canonical) = canonical_cookie_name(&name) else {
            continue;
        };
        if !valid_cookie_value(&value) {
            return Err("本机会话包含无效 Cookie".to_owned());
        }
        cookies.insert(canonical.to_owned(), value);
    }
    let csrf_token = valid_cookie_value(&raw.csrf_token)
        .then_some(raw.csrf_token)
        .or_else(|| cookies.get("bili_jct").cloned())
        .unwrap_or_default();
    let session = Session {
        cookies,
        csrf_token,
    };
    if !session.has_login_cookies() && !session.cookies.is_empty() {
        return Err("本机会话缺少登录 Cookie".to_owned());
    }
    Ok(Some(session))
}

fn build_url(base: &str, path: &str, params: &[(String, String)]) -> Result<String, String> {
    let mut url = Url::parse(&format!("{}{}", base.trim_end_matches('/'), path))
        .map_err(|_| "B 站账号接口地址格式不正确".to_owned())?;
    if !params.is_empty() {
        url.query_pairs_mut().extend_pairs(
            params
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
    }
    Ok(url.into())
}

fn validate_qr_url(value: &str) -> Result<String, String> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err("二维码登录地址无效".to_owned());
    }
    let parsed = Url::parse(value).map_err(|_| "二维码登录地址无效".to_owned())?;
    if parsed.scheme() != "https"
        || !matches!(
            parsed.host_str(),
            Some("passport.bilibili.com" | "account.bilibili.com")
        )
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some_and(|port| port != 443)
    {
        return Err("二维码登录地址不是允许的 B 站登录域名".to_owned());
    }
    Ok(parsed.into())
}

fn validate_qr_key(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_QR_KEY_CHARS
        || value.chars().any(char::is_control)
    {
        return Err("二维码登录密钥无效".to_owned());
    }
    Ok(value.to_owned())
}

fn canonical_cookie_name(name: &str) -> Option<&'static str> {
    COOKIE_WHITELIST
        .iter()
        .copied()
        .find(|allowed| allowed.eq_ignore_ascii_case(name.trim()))
}

fn valid_cookie_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !value
            .chars()
            .any(|character| character == '\0' || character == '\r' || character == '\n')
}

fn checked_api_success(payload: &Value) -> Result<(), String> {
    let code = payload
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    if code == 0 {
        return Ok(());
    }
    let message = clean_text(
        payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("未知错误"),
    );
    if matches!(code, -352 | -412 | -799) || is_risk_text(&message) {
        return Err("B 站暂时触发了访问限制，请稍后再试".to_owned());
    }
    if code == -101 {
        return Err("B 站账号登录已失效，请重新扫码登录".to_owned());
    }
    Err(format!("B 站账号接口返回错误：{message}"))
}

fn checked_api_data(payload: &Value) -> Result<&serde_json::Map<String, Value>, String> {
    checked_api_success(payload)?;
    payload
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| "B 站账号响应缺少数据".to_owned())
}

fn folder_from_item(item: &Value) -> Option<Folder> {
    let object = item.as_object()?;
    let id = value_u64(object.get("id")).max(value_u64(object.get("media_id")));
    if id == 0 {
        return None;
    }
    let title = clean_text(
        object
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("未命名收藏夹"),
    );
    Some(Folder {
        id,
        title: if title.is_empty() {
            "未命名收藏夹".to_owned()
        } else {
            title
        },
        count: value_u64(object.get("media_count")).max(value_u64(object.get("count"))),
    })
}

fn value_u64(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(number)) => number
            .as_u64()
            .or_else(|| {
                number
                    .as_i64()
                    .and_then(|number| u64::try_from(number).ok())
            })
            .unwrap_or_default(),
        Some(Value::String(text)) => text.trim().parse().unwrap_or_default(),
        _ => 0,
    }
}

fn clean_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            *character != '\0' && (!character.is_control() || character.is_whitespace())
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TEXT_CHARS)
        .collect()
}

fn validate_page(page: u32) -> Result<(), String> {
    if (1..=MAX_PAGE).contains(&page) {
        Ok(())
    } else {
        Err(format!("页码必须是 1 到 {MAX_PAGE} 的整数"))
    }
}

fn network_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "连接 B 站超时，请检查网络后重试".to_owned()
    } else if error.is_connect() {
        "无法连接 B 站，请检查网络后重试".to_owned()
    } else {
        "B 站账号网络请求失败，请稍后重试".to_owned()
    }
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

fn unix_timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    struct MockReply {
        status: u16,
        reason: &'static str,
        body: String,
        set_cookies: Vec<String>,
        delay: Duration,
    }

    struct CapturedRequest {
        line: String,
        headers: String,
        body: String,
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bili-account-test-{}-{}",
                std::process::id(),
                unix_timestamp_nanos()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self { path }
        }

        fn session_path(&self) -> PathBuf {
            self.path.join("account-session.json")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn json_reply(value: Value) -> MockReply {
        MockReply {
            status: 200,
            reason: "OK",
            body: value.to_string(),
            set_cookies: Vec::new(),
            delay: Duration::ZERO,
        }
    }

    fn mock_server(
        replies: Vec<MockReply>,
    ) -> (
        String,
        mpsc::Receiver<CapturedRequest>,
        thread::JoinHandle<()>,
    ) {
        assert!(!replies.is_empty());
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let address = listener.local_addr().expect("mock server address");
        let (request_tx, request_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            for reply in replies {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut reader = BufReader::new(stream.try_clone().expect("clone request stream"));
                let mut line = String::new();
                reader.read_line(&mut line).expect("read request line");
                let mut headers = String::new();
                let mut content_length = 0_usize;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).expect("read request header");
                    if header == "\r\n" || header.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        content_length = value.trim().parse().unwrap_or_default();
                    }
                    headers.push_str(&header);
                }
                let mut body = vec![0_u8; content_length];
                if content_length > 0 {
                    reader.read_exact(&mut body).expect("read request body");
                }
                request_tx
                    .send(CapturedRequest {
                        line,
                        headers,
                        body: String::from_utf8(body).expect("UTF-8 request body"),
                    })
                    .expect("send captured request");

                let mut response = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reply.reason,
                    reply.body.len()
                );
                for cookie in reply.set_cookies {
                    response.push_str(&format!("Set-Cookie: {cookie}\r\n"));
                }
                response.push_str("\r\n");
                response.push_str(&reply.body);
                if !reply.delay.is_zero() {
                    thread::sleep(reply.delay);
                }
                stream
                    .write_all(response.as_bytes())
                    .expect("write mock response");
            }
        });
        (format!("http://{address}"), request_rx, handle)
    }

    fn test_client(base: &str, directory: &TestDirectory) -> AccountClient {
        let mut client =
            AccountClient::new(directory.session_path()).expect("create account client");
        client.api_base = base.to_owned();
        client.passport_base = base.to_owned();
        client.test_cookie_host = Some("127.0.0.1".to_owned());
        client
    }

    fn test_session() -> Session {
        Session {
            cookies: BTreeMap::from([
                ("SESSDATA".to_owned(), "session-value".to_owned()),
                ("bili_jct".to_owned(), "csrf-value".to_owned()),
                ("DedeUserID".to_owned(), "42".to_owned()),
            ]),
            csrf_token: "csrf-value".to_owned(),
        }
    }

    #[test]
    fn qr_generate_accepts_current_and_legacy_official_domains() {
        for login_url in [
            "https://account.bilibili.com/h5/account-h5/auth/scan-web?navhide=1&qrcode_key=test-key",
            "https://passport.bilibili.com/h5-app/passport/login/scan?qrcode_key=test-key",
        ] {
            let directory = TestDirectory::new();
            let (base, requests, handle) = mock_server(vec![json_reply(json!({
                "code":0,"data":{"url":login_url,"qrcode_key":"test-key"}
            }))]);
            let client = test_client(&base, &directory);
            let qr = client
                .generate_qr()
                .expect("accept official QR login origin");
            assert_eq!(qr.url, login_url);
            assert_eq!(qr.key, "test-key");
            let request = requests.recv().unwrap();
            assert!(
                request
                    .line
                    .starts_with("GET /x/passport-login/web/qrcode/generate")
            );
            assert!(!request.headers.to_ascii_lowercase().contains("cookie:"));
            assert!(!client.is_logged_in());
            assert!(!directory.session_path().exists());
            handle.join().unwrap();
        }
    }

    #[test]
    fn qr_url_rejects_lookalike_domains_and_unsafe_origins() {
        for invalid in [
            "https://account.bilibili.com.evil.example/scan",
            "https://passport.bilibili.com.evil.example/scan",
            "https://evil-bilibili.com/scan",
            "https://evil.example/?next=https://account.bilibili.com",
            "https://account.bilibili.com@evil.example/scan",
            "https://user@account.bilibili.com/scan",
            "http://account.bilibili.com/scan",
            "https://account.bilibili.com:8443/scan",
            "https://127.0.0.1/scan",
            "",
            "https://account.bilibili.com/scan\n",
        ] {
            assert!(
                validate_qr_url(invalid).is_err(),
                "accepted unsafe origin: {invalid}"
            );
        }
    }

    #[test]
    #[ignore = "requires live Bilibili QR endpoints; does not confirm login or persist credentials"]
    fn live_qr_generate_and_wait_for_scan() {
        let directory = TestDirectory::new();
        let client = AccountClient::new(directory.session_path()).unwrap();
        let qr = client
            .generate_qr()
            .expect("generate real official login QR");
        let host = Url::parse(&qr.url).unwrap().host_str().unwrap().to_owned();
        let code =
            qrcode::QrCode::new(qr.url.as_bytes()).expect("render generated URL into QR modules");
        assert!(code.width() > 0);
        assert_eq!(
            client.poll_qr(&qr.key).expect("check unscanned QR"),
            LoginPoll::Waiting
        );
        assert!(!client.is_logged_in());
        assert!(!directory.session_path().exists());
        println!("official QR host={host}; QR rendering and unscanned polling passed");
    }

    #[test]
    fn qr_statuses_are_mapped_without_cookie_or_confirmation() {
        for (code, expected) in [
            (86101, LoginPoll::Waiting),
            (86090, LoginPoll::Scanned),
            (86038, LoginPoll::Expired),
        ] {
            let directory = TestDirectory::new();
            let (base, requests, handle) = mock_server(vec![json_reply(json!({
                "code": 0,
                "data": {"code": code, "message": "mock status"}
            }))]);
            let client = test_client(&base, &directory);

            assert_eq!(
                client.poll_qr("poll-key").expect("poll QR status"),
                expected
            );
            let request = requests.recv().expect("poll request");
            assert!(
                request
                    .line
                    .starts_with("GET /x/passport-login/web/qrcode/poll")
            );
            assert!(!request.headers.to_ascii_lowercase().contains("cookie:"));
            handle.join().expect("mock server should finish");
        }
    }

    #[test]
    fn qr_success_whitelists_cookies_and_secures_session_file() {
        let directory = TestDirectory::new();
        let (base, requests, handle) = mock_server(vec![
            MockReply {
                status: 200,
                reason: "OK",
                body: json!({
                    "code": 0,
                    "data": {"code": 0, "url": "https://www.bilibili.com/"}
                })
                .to_string(),
                set_cookies: vec![
                    "SESSDATA=session-value; Path=/; HttpOnly".to_owned(),
                    "bili_jct=csrf-value; Path=/; HttpOnly".to_owned(),
                    "DedeUserID=42; Path=/".to_owned(),
                    "tracking=must-not-survive; Path=/".to_owned(),
                ],
                delay: Duration::ZERO,
            },
            json_reply(json!({
                "code": 0,
                "data": {"isLogin": true, "mid": 42, "uname": "测试用户"}
            })),
        ]);
        let client = test_client(&base, &directory);

        assert_eq!(
            client.poll_qr("poll-key").expect("complete mock login"),
            LoginPoll::Success(AccountProfile {
                mid: 42,
                name: "测试用户".to_owned(),
            })
        );
        let poll_request = requests.recv().expect("poll request");
        let nav_request = requests.recv().expect("nav request");
        assert!(
            !poll_request
                .headers
                .to_ascii_lowercase()
                .contains("cookie:")
        );
        let nav_headers = nav_request.headers.to_ascii_lowercase();
        assert!(nav_headers.contains("cookie:"));
        assert!(nav_headers.contains("sessdata=session-value"));
        assert!(nav_headers.contains("bili_jct=csrf-value"));
        assert!(nav_headers.contains("dedeuserid=42"));
        assert!(!nav_headers.contains("tracking="));
        assert!(client.is_logged_in());
        let cookie_header = client.cookie_header().expect("cookie header");
        assert!(cookie_header.contains("SESSDATA=session-value"));
        assert!(cookie_header.contains("bili_jct=csrf-value"));
        assert!(cookie_header.contains("DedeUserID=42"));
        assert!(!cookie_header.contains("tracking="));

        let path = directory.session_path();
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path)
                .expect("session metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let reloaded = AccountClient::new(path).expect("reload persisted session");
        assert!(reloaded.is_logged_in());
        handle.join().expect("mock server should finish");
    }

    #[test]
    fn favorite_posts_aid_folder_and_csrf_to_api_host() {
        let directory = TestDirectory::new();
        let (base, requests, handle) = mock_server(vec![
            json_reply(json!({
                "code": 0,
                "data": {"aid": 987654, "cid": 123, "duration": 200}
            })),
            json_reply(json!({"code": 0, "data": {"prompt": false}})),
        ]);
        let client = test_client(&base, &directory);
        let session = test_session();
        client
            .persist_session(&session)
            .expect("persist test session");
        *client.session.lock().expect("lock test session") = Some(session);

        client
            .favorite("BV1xx411c7mD", 31415)
            .expect("favorite through mock");

        let view_request = requests.recv().expect("view request");
        let favorite_request = requests.recv().expect("favorite request");
        assert!(view_request.line.contains("/x/web-interface/view"));
        assert!(
            favorite_request
                .line
                .starts_with("POST /x/v3/fav/resource/deal")
        );
        let favorite_headers = favorite_request.headers.to_ascii_lowercase();
        assert!(favorite_headers.contains("cookie:"));
        assert!(favorite_headers.contains("sessdata=session-value"));
        let form = url::form_urlencoded::parse(favorite_request.body.as_bytes())
            .into_owned()
            .collect::<HashMap<_, _>>();
        assert_eq!(form.get("rid").map(String::as_str), Some("987654"));
        assert_eq!(form.get("type").map(String::as_str), Some("2"));
        assert_eq!(form.get("add_media_ids").map(String::as_str), Some("31415"));
        assert_eq!(form.get("csrf").map(String::as_str), Some("csrf-value"));
        handle.join().expect("mock server should finish");
    }

    #[test]
    fn report_posts_aid_cid_progress_and_csrf() {
        let directory = TestDirectory::new();
        let (base, requests, handle) = mock_server(vec![
            json_reply(json!({
                "code": 0,
                "data": {"aid": 987654, "cid": 123, "duration": 200}
            })),
            json_reply(json!({"code": 0, "data": {}})),
        ]);
        let client = test_client(&base, &directory);
        let session = test_session();
        client
            .persist_session(&session)
            .expect("persist test session");
        *client.session.lock().expect("lock test session") = Some(session);

        client
            .report("BV1xx411c7mD", 75)
            .expect("report through mock");

        let _view_request = requests.recv().expect("view request");
        let report_request = requests.recv().expect("report request");
        assert!(
            report_request
                .line
                .starts_with("POST /x/click-interface/web/heartbeat")
        );
        let report_headers = report_request.headers.to_ascii_lowercase();
        assert!(report_headers.contains("cookie:"));
        assert!(report_headers.contains("sessdata=session-value"));
        let form = url::form_urlencoded::parse(report_request.body.as_bytes())
            .into_owned()
            .collect::<HashMap<_, _>>();
        assert_eq!(form.get("aid").map(String::as_str), Some("987654"));
        assert_eq!(form.get("cid").map(String::as_str), Some("123"));
        assert_eq!(form.get("played_time").map(String::as_str), Some("75"));
        assert_eq!(form.get("video_duration").map(String::as_str), Some("200"));
        assert_eq!(form.get("csrf").map(String::as_str), Some("csrf-value"));
        assert!(!form.contains_key("real_played_time"));
        assert!(!form.contains_key("realtime"));
        assert!(!form.contains_key("start_ts"));
        handle.join().expect("mock server should finish");
    }

    #[test]
    fn session_permissions_are_tightened_and_logout_is_local() {
        let directory = TestDirectory::new();
        let path = directory.session_path();
        let client = AccountClient::new(path.clone()).expect("create account client");
        client
            .persist_session(&test_session())
            .expect("persist session");
        #[cfg(unix)]
        {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
                .expect("make test permissions broad");
            let reloaded = AccountClient::new(path.clone()).expect("reload and tighten session");
            assert!(reloaded.is_logged_in());
            assert_eq!(
                fs::metadata(&path)
                    .expect("session metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            reloaded.logout().expect("local logout");
        }
        assert!(!path.exists());
    }

    #[test]
    fn history_cursor_cache_makes_consecutive_pages_single_request_each() {
        let directory = TestDirectory::new();
        let page = |max: u64, view_at: u64| {
            json_reply(json!({
                "code": 0,
                "data": {
                    "list": [],
                    "cursor": {
                        "max": max,
                        "view_at": view_at,
                        "business": "archive",
                        "is_end": false
                    }
                }
            }))
        };
        let (base, requests, handle) =
            mock_server(vec![page(101, 11), page(202, 22), page(303, 33)]);
        let client = test_client(&base, &directory);
        let session = test_session();
        client.persist_session(&session).expect("persist session");
        *client.session.lock().expect("lock session") = Some(session);

        assert!(client.history(1).expect("first page").has_more);
        client.history(2).expect("second page");
        client.history(1).expect("refresh first page");

        let first = requests.recv().expect("page 1 request");
        let second = requests.recv().expect("page 2 request");
        let refreshed = requests.recv().expect("refresh request");
        assert!(first.line.contains("max=0"));
        assert!(second.line.contains("max=101"));
        assert!(refreshed.line.contains("max=0"));
        handle.join().expect("mock server should finish");
    }

    #[test]
    fn logged_out_nav_response_clears_local_session() {
        let directory = TestDirectory::new();
        let (base, _requests, handle) = mock_server(vec![json_reply(json!({
            "code": 0,
            "data": {"isLogin": false, "mid": 0, "uname": ""}
        }))]);
        let client = test_client(&base, &directory);
        let session = test_session();
        client.persist_session(&session).expect("persist session");
        *client.session.lock().expect("lock session") = Some(session);

        assert_eq!(client.profile().expect("query invalid profile"), None);

        assert!(!client.is_logged_in());
        assert!(client.cookie_header().is_none());
        assert!(!directory.session_path().exists());
        handle.join().expect("mock server should finish");
    }

    #[test]
    fn corrupt_session_degrades_to_logged_out_and_preserves_file() {
        let directory = TestDirectory::new();
        let path = directory.session_path();
        fs::write(&path, b"{not-json").expect("write corrupt session");
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("set private test permissions");

        let client = AccountClient::new(path.clone()).expect("degrade corrupt session");

        assert!(!client.is_logged_in());
        assert!(path.exists());
    }

    #[test]
    fn late_poll_success_cannot_restore_after_logout() {
        let directory = TestDirectory::new();
        let (base, requests, handle) = mock_server(vec![
            MockReply {
                status: 200,
                reason: "OK",
                body: json!({
                    "code": 0,
                    "data": {"code": 0, "url": "https://www.bilibili.com/"}
                })
                .to_string(),
                set_cookies: vec![
                    "SESSDATA=session-value; Path=/".to_owned(),
                    "bili_jct=csrf-value; Path=/".to_owned(),
                    "DedeUserID=42; Path=/".to_owned(),
                ],
                delay: Duration::ZERO,
            },
            MockReply {
                status: 200,
                reason: "OK",
                body: json!({
                    "code": 0,
                    "data": {"isLogin": true, "mid": 42, "uname": "旧登录"}
                })
                .to_string(),
                set_cookies: Vec::new(),
                delay: Duration::from_millis(200),
            },
        ]);
        let client = test_client(&base, &directory);
        let poll_client = client.clone();
        let poll = thread::spawn(move || poll_client.poll_qr("poll-key"));

        requests.recv().expect("poll request");
        client.logout().expect("logout while poll is running");
        let result = poll.join().expect("poll thread should finish");

        assert!(result.is_err());
        assert!(!client.is_logged_in());
        assert!(!directory.session_path().exists());
        handle.join().expect("mock server should finish");
    }

    #[test]
    fn logout_waits_for_session_lock_and_wins_after_commit() {
        let directory = TestDirectory::new();
        let client = AccountClient::new(directory.session_path()).expect("create client");
        let session = test_session();
        client.persist_session(&session).expect("persist session");
        let mut guard = client.session.lock().expect("hold session lock");
        *guard = Some(session);

        let logout_client = client.clone();
        let logout = thread::spawn(move || logout_client.logout());
        while client.generation.load(Ordering::SeqCst) == 0 {
            thread::yield_now();
        }
        assert!(!logout.is_finished());

        drop(guard);
        logout
            .join()
            .expect("logout thread should finish")
            .expect("logout should succeed");

        assert!(!client.is_logged_in());
        assert!(!directory.session_path().exists());
    }

    #[test]
    fn stale_profile_none_cannot_clear_newer_login() {
        let directory = TestDirectory::new();
        let (base, requests, handle) = mock_server(vec![MockReply {
            status: 200,
            reason: "OK",
            body: json!({
                "code": 0,
                "data": {"isLogin": false, "mid": 0, "uname": ""}
            })
            .to_string(),
            set_cookies: Vec::new(),
            delay: Duration::from_millis(150),
        }]);
        let client = test_client(&base, &directory);
        let old_session = test_session();
        client
            .persist_session(&old_session)
            .expect("persist old session");
        *client.session.lock().expect("lock session") = Some(old_session);

        let profile_client = client.clone();
        let profile = thread::spawn(move || profile_client.profile());
        requests.recv().expect("old profile request");

        let mut new_session = test_session();
        new_session
            .cookies
            .insert("SESSDATA".to_owned(), "new-session-value".to_owned());
        {
            let mut current = client.session.lock().expect("lock session");
            client.generation.fetch_add(1, Ordering::SeqCst);
            client
                .persist_session(&new_session)
                .expect("persist newer session");
            *current = Some(new_session);
        }

        assert_eq!(
            profile.join().expect("profile thread should finish"),
            Ok(None)
        );
        let cookie_header = client.cookie_header().expect("new session cookie");
        assert!(cookie_header.contains("SESSDATA=new-session-value"));
        assert!(client.is_logged_in());
        assert!(directory.session_path().exists());
        handle.join().expect("mock server should finish");
    }
}
