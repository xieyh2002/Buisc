use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REFERER: &str = "https://www.bilibili.com/";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DOWNLOAD_BUFFER_SIZE: usize = 64 * 1024;
const TEMP_FILE_ATTEMPTS: usize = 32;

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempFile {
    path: PathBuf,
    committed: bool,
}

impl TempFile {
    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn save_audio(url: &str, destination: &Path, cancel: &AtomicBool) -> Result<(), String> {
    if !crate::api::valid_audio_url(url) {
        return Err("音频地址无效或不是 B 站官方 CDN".to_owned());
    }
    download_audio(url, destination, cancel)
}

fn download_audio(url: &str, destination: &Path, cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("下载已取消".to_owned());
    }
    if destination.file_name().is_none() {
        return Err("音频保存路径无效".to_owned());
    }

    let parent = normalized_parent(destination);
    fs::create_dir_all(parent).map_err(|_| "无法创建音频保存目录".to_owned())?;
    let (mut file, mut temp_file) = create_temp_file(destination)?;

    let transfer_result = (|| {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TOTAL_TIMEOUT)
            .redirect(Policy::none())
            .build()
            .map_err(|_| "无法创建音频下载客户端".to_owned())?;

        if cancel.load(Ordering::Relaxed) {
            return Err("下载已取消".to_owned());
        }

        let mut response = client
            .get(url)
            .header(reqwest::header::USER_AGENT, crate::api::USER_AGENT)
            .header(reqwest::header::REFERER, REFERER)
            .send()
            .map_err(|_| "音频下载连接失败或超时".to_owned())?;

        if !response.status().is_success() {
            return Err(format!(
                "音频下载失败（HTTP {}）",
                response.status().as_u16()
            ));
        }

        let mut buffer = [0_u8; DOWNLOAD_BUFFER_SIZE];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("下载已取消".to_owned());
            }

            let read = match response.read(&mut buffer) {
                Ok(read) => read,
                Err(_) if cancel.load(Ordering::Relaxed) => {
                    return Err("下载已取消".to_owned());
                }
                Err(_) => return Err("读取音频数据失败或超时".to_owned()),
            };
            if read == 0 {
                if cancel.load(Ordering::Relaxed) {
                    return Err("下载已取消".to_owned());
                }
                break;
            }
            if cancel.load(Ordering::Relaxed) {
                return Err("下载已取消".to_owned());
            }
            file.write_all(&buffer[..read])
                .map_err(|_| "写入音频临时文件失败".to_owned())?;
        }

        file.sync_all()
            .map_err(|_| "同步音频临时文件失败".to_owned())
    })();

    drop(file);
    transfer_result?;
    if cancel.load(Ordering::Relaxed) {
        return Err("下载已取消".to_owned());
    }

    fs::rename(&temp_file.path, destination).map_err(|_| "无法将音频保存到目标位置".to_owned())?;
    temp_file.commit();
    Ok(())
}

fn normalized_parent(destination: &Path) -> &Path {
    destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn create_temp_file(destination: &Path) -> Result<(File, TempFile), String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());

    for _ in 0..TEMP_FILE_ATTEMPTS {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut file_name = OsString::from(".bili-music-audio-");
        file_name.push(std::process::id().to_string());
        file_name.push(format!("-{timestamp}-{sequence}.tmp"));
        let path = destination.with_file_name(file_name);

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        match options.open(&path) {
            Ok(file) => {
                return Ok((
                    file,
                    TempFile {
                        path,
                        committed: false,
                    },
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err("无法创建音频临时文件".to_owned()),
        }
    }

    Err("无法创建唯一的音频临时文件".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "bili-download-{name}-{}-{}",
                std::process::id(),
                TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read_request_headers<T: Read>(stream: &mut T) -> String {
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            headers.push_str(&line);
        }
        headers
    }

    fn mocked_audio_server(
        status: u16,
        reason: &'static str,
        body: Vec<u8>,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (headers_tx, headers_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let headers = read_request_headers(&mut stream);
            headers_tx.send(headers).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\n\
                 Content-Type: audio/mp4\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });

        (format!("http://{address}/audio.m4s"), headers_rx, handle)
    }

    fn assert_no_temp_files(directory: &Path) {
        let names = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert!(
            names
                .iter()
                .all(|name| !name.to_string_lossy().ends_with(".tmp")),
            "临时文件未清理：{names:?}"
        );
    }

    #[test]
    fn saves_complete_audio_and_sends_bilibili_headers() {
        let directory = TestDir::new("complete");
        let destination = directory.path().join("track.m4a");
        let body = (0..(DOWNLOAD_BUFFER_SIZE * 2 + 17))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let (url, headers_rx, server) = mocked_audio_server(200, "OK", body.clone());

        download_audio(&url, &destination, &AtomicBool::new(false)).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), body);
        let headers = headers_rx.recv().unwrap();
        assert!(headers.contains(crate::api::USER_AGENT));
        assert!(
            headers
                .to_ascii_lowercase()
                .contains(&format!("referer: {REFERER}"))
        );
        server.join().unwrap();
        assert_no_temp_files(directory.path());
    }

    #[test]
    fn failed_download_keeps_existing_destination() {
        let directory = TestDir::new("failure");
        let destination = directory.path().join("track.m4a");
        fs::write(&destination, b"existing audio").unwrap();
        let (url, _headers_rx, server) =
            mocked_audio_server(503, "Service Unavailable", b"retry".to_vec());

        let error = download_audio(&url, &destination, &AtomicBool::new(false)).unwrap_err();

        assert!(error.contains("HTTP 503"));
        assert_eq!(fs::read(&destination).unwrap(), b"existing audio");
        server.join().unwrap();
        assert_no_temp_files(directory.path());
    }

    #[test]
    fn cancellation_cleans_temp_file_and_keeps_existing_destination() {
        let directory = TestDir::new("cancel");
        let destination = directory.path().join("track.m4a");
        fs::write(&destination, b"existing audio").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request_headers(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\
                 Content-Type: audio/mp4\r\nConnection: close\r\n\r\n",
                DOWNLOAD_BUFFER_SIZE * 4
            )
            .unwrap();
            stream.write_all(&vec![7_u8; DOWNLOAD_BUFFER_SIZE]).unwrap();
            started_tx.send(()).unwrap();
            thread::sleep(Duration::from_millis(100));
        });

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_worker = Arc::clone(&cancel);
        let canceller = thread::spawn(move || {
            started_rx.recv().unwrap();
            cancel_for_worker.store(true, Ordering::Relaxed);
        });

        let error = download_audio(
            &format!("http://{address}/audio.m4s"),
            &destination,
            cancel.as_ref(),
        )
        .unwrap_err();

        assert_eq!(error, "下载已取消");
        assert_eq!(fs::read(&destination).unwrap(), b"existing audio");
        canceller.join().unwrap();
        server.join().unwrap();
        assert_no_temp_files(directory.path());
    }

    #[test]
    fn public_entry_rejects_non_bilibili_url_without_writing() {
        let directory = TestDir::new("validation");
        let destination = directory.path().join("track.m4a");

        let error = save_audio(
            "http://127.0.0.1/audio.m4s",
            &destination,
            &AtomicBool::new(false),
        )
        .unwrap_err();

        assert!(error.contains("B 站官方 CDN"));
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
