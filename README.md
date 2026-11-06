<div align="center">
  <img src="assets/buisc-logo.png" width="112" height="112" alt="Buisc 粉蓝小电视图标">
  <h1>Buisc</h1>
  <p><strong>把哔哩哔哩当作你的曲库。</strong><br>轻量原生 Linux 音乐播放器，免登录在线听，也支持扫码同步收藏和播放记录。</p>

  <p>
    <a href="https://github.com/xieyh2002/Buisc/releases/latest"><img src="https://img.shields.io/github/v/release/xieyh2002/Buisc?label=release&amp;style=flat-square&amp;color=fb7299" alt="最新版本"></a>
    <a href="https://github.com/xieyh2002/Buisc/releases"><img src="https://img.shields.io/github/downloads/xieyh2002/Buisc/total?label=downloads&amp;style=flat-square" alt="总下载量"></a>
    <a href="https://github.com/xieyh2002/Buisc/actions/workflows/release.yml"><img src="https://github.com/xieyh2002/Buisc/actions/workflows/release.yml/badge.svg" alt="发布构建"></a>
    <a href="LICENSE"><img src="https://img.shields.io/github/license/xieyh2002/Buisc?style=flat-square" alt="MIT 许可证"></a>
    <a href="https://github.com/xieyh2002/Buisc/stargazers"><img src="https://img.shields.io/github/stars/xieyh2002/Buisc?label=stars&amp;style=flat-square" alt="GitHub Stars"></a>
  </p>
  <p>
    <img src="https://img.shields.io/badge/Linux-amd64%20%7C%20arm64-007fae?style=flat-square&amp;logo=linux&amp;logoColor=white" alt="Linux amd64 和 arm64">
    <img src="https://img.shields.io/badge/Rust-1.92%2B-de6538?style=flat-square&amp;logo=rust&amp;logoColor=white" alt="Rust 1.92+">
    <img src="https://img.shields.io/badge/GTK-4.8%2B-4a86cf?style=flat-square&amp;logo=gtk&amp;logoColor=white" alt="GTK 4.8+">
    <img src="https://img.shields.io/badge/GStreamer-audio-5c6692?style=flat-square" alt="GStreamer 音频播放">
  </p>
  <p><strong><a href="https://github.com/xieyh2002/Buisc/releases/latest">⬇ 下载最新版</a> · <a href="#功能">功能</a> · <a href="#安装">安装</a> · <a href="CONTRIBUTING.md">参与贡献</a> · <a href="CHANGELOG.md">更新日志</a></strong></p>
  <img src="docs/screenshot.png" width="900" alt="Buisc 音乐热榜界面与底部播放条">
</div>

## 功能

- 无需登录即可在线播放；支持 B 站扫码登录、会话保存、失效检测与退出。
- 热榜封面首页、平滑滚动和实时毛玻璃播放条。默认由 GTK 选择渲染器，可在「设置与备份 → 界面渲染」切换 CPU 兼容模式，重开后生效。
- 读取账号收藏夹与 B站历史，选择收藏夹后收藏曲目。
- 播放时每 30 秒同步历史进度，暂停或切歌时补报，可在设置中关闭。
- B 站音乐区热榜、自动附加“音乐”的关键词搜索、滚动自动翻页、BV 号或视频链接直达。
- AAC 音频流播放、暂停、上一首 / 下一首、拖动进度、音量。
- 播放队列、顺序播放 / 单曲循环 / 列表循环 / 随机播放。
- 当前视频封面、原始 AAC/M4A 音频下载。
- 收藏、最近 200 首记录、本地歌单、JSON 资料库导入与导出。
- MPRIS 系统媒体控制，可在 KDE/GNOME 的媒体面板控制播放。
- 可选关闭到系统托盘，托盘菜单可显示窗口、控制播放或彻底退出；默认关闭。
- 单实例；网络任务在后台执行，搜索和切歌通过请求序号屏蔽过期响应。
- 资料库原子写入，读入异常时保留原文件并另存恢复资料库。

## 安装

在 [GitHub Releases](https://github.com/xieyh2002/Buisc/releases) 下载对应架构安装包：普通 Intel/AMD 电脑选择 `amd64`（RPM 文件名为 `x86_64`），ARM 电脑选择 `arm64`（RPM 文件名为 `aarch64`）。

发布包基于 Ubuntu 24.04 构建：DEB 面向 Ubuntu 24.04+ / Debian 13+；RPM 面向 Fedora 41+。需要 glibc 2.39+、GLib 2.80+、GTK 4.8+ 与 GStreamer 插件。其他发行版建议从源码构建。ARM64 包由原生 ARM runner 构建并运行单元测试，图形界面仍需在目标硬件确认。

```sh
# Debian / Ubuntu
sudo apt install ./buisc_0.2.0_amd64.deb
# Fedora（下载文件名中的 x86_64 对应 amd64）
sudo dnf install ./buisc-0.2.0-1.x86_64.rpm
```

## 编译与运行

需要 Rust 1.92+、GTK 4.8+、GStreamer 1.x，以及 HTTPS、AAC 和音频输出插件。

Fedora：

```sh
sudo dnf install gcc pkg-config gtk4-devel gstreamer1-devel \
  gstreamer1-plugins-base-devel gstreamer1-plugins-good gstreamer1-libav
cargo build --release --locked -j 2
./target/release/bili-music
```

Debian / Ubuntu：

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-good \
  gstreamer1.0-libav gstreamer1.0-pulseaudio
cargo build --release --locked -j 2
```

安装到当前用户的应用菜单，无需管理员权限：

```sh
./scripts/install.sh
```

卸载：`./scripts/uninstall.sh`，会保留个人资料库。默认关闭窗口会退出。可在“设置与备份”勾选“关闭窗口后在托盘后台运行”，音乐继续播放，点击托盘图标或重新启动应用即可恢复窗口。托盘菜单“退出 Buisc”、设置中的“退出 Buisc”或 `Ctrl+Q` 始终彻底退出；最后一笔历史同步完成或超时后结束进程。桌面不支持系统托盘时，关闭窗口仍正常退出，不会隐藏到无法找回。

## 登录与账号同步

点击侧栏“扫码登录 B站”，用 B站手机客户端扫描二维码并确认。程序不要求账号密码，也不读取浏览器 Cookie。登录凭据单独保存在资料目录的 `account.json`（权限 `0600`），不会包含在资料库导出的 JSON 中；“退出本机登录”会删除本机会话。

侧栏“B站收藏夹”读取当前账号的收藏夹，打开后可直接播放；选中曲目，点击“B站收藏”并选择目标收藏夹即可保存到账号。“B站历史”读取平台历史。播放记录默认同步到 B站，设置中可关闭。当前只处理视频类历史，默认播放视频第一 P。

“本地收藏”、本地歌单与账号收藏夹分别保存。离线时可查看本地资料库，但在线播放需要网络。二维码登录、会话过期和风控由 B站决定；如失效，请重新扫码。

## 使用

双击曲目即可播放。选中后使用工具栏收藏、加入队列或加入歌单；侧栏 `+` 创建歌单。点击“播放全部”将当前列表设为队列。顺序模式到队列末尾停止。

- `Ctrl+空格`：播放 / 暂停。
- `Ctrl+←`、`Ctrl+→`：上一首 / 下一首。
- `Ctrl+F`：搜索。
- `Ctrl+Q`：彻底退出（包括已启用托盘后台模式）。
- 系统媒体键由桌面环境通过 MPRIS 分发。

资料库默认保存于 `~/.local/share/bili-music/library.json`，遵循 `XDG_DATA_HOME`。导入备份将替换现有收藏、歌单和队列，导入前可先导出备份。

## 验证

```sh
cargo test --release --locked -j 2
cargo clippy --all-targets --locked -j 2 -- -D warnings
./scripts/test-tray.sh
./scripts/test-live.sh
```

托盘测试需要桌面提供 StatusNotifier 系统托盘，不访问网络。最后一项需要图形桌面和网络，使用独立的测试 MPRIS 名称与临时资料目录验证真实榜单、搜索、链接直达、播放、进度跳转、暂停、收藏、歌单和备份，不读取个人资料库。测试静音，并生成 `docs/screenshot.png`，输出空闲 / 播放 / 暂停状态的 RSS、PSS 与 CPU 采样。

## 当前范围

支持游客与扫码登录，只选择普通 AAC 音频，不处理无损音轨。多 P 视频当前播放第一 P。音频 URL 在播放前重新解析，不持久化临时签名。网络不可用或平台风控时会显示错误，不自动无限重试。

播放栏只加载当前视频的封面；首页分批加载有限数量的热榜缩略图。封面不写入磁盘，加载失败时显示默认图标。暂未加入歌词或多 P 连播。源码和 release 二进制依赖系统 GTK / GStreamer 动态库，当前没有制作跨发行版 AppImage。开发与安装脚本使用 Python 3，应用运行本身不需要 Python。

本项目与 Bilibili 无官方关联。音视频内容来自平台，不随项目分发。

关键词搜索自动附加“音乐”，减少无关内容；已含“音乐”的关键词不会重复追加，BV 号和视频链接仍直接打开原视频。搜索结果及账号列表滚动到底时自动加载下一页。

播放栏的模式按钮可切换顺序、单曲循环、列表循环与随机播放。下载按钮保存当前歌曲；未播放时保存选中曲目。音频以 M4A 文件保存，不转码。

## 致谢与许可

项目参考了 [HanversionOvO/BiliMusic](https://github.com/HanversionOvO/BiliMusic) 与 [Jmiao11/bili-music](https://github.com/Jmiao11/bili-music) 的使用场景，独立实现 Linux 版本。

按 [MIT 许可证](LICENSE) 发布。

## 维护者打包

先构建目标架构的 release 二进制，再运行：

```sh
scripts/package.sh --format all --arch amd64 --binary target/release/bili-music --output dist
```

打包需要 `dpkg-deb` 和 `rpmbuild`。不能仅修改 `--arch` 代替真正的目标架构编译。推送与 Cargo 版本匹配的 `v*` 标签后，GitHub Actions 会在原生 amd64 / arm64 runner 上构建两种安装包并发布 SHA-256 校验文件。
