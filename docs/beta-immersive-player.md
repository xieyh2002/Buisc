# Beta：沉浸播放页

这个分支用于验证 `0.3.0` 之后的播放器交互，不改变资料库格式，也不改变音源解析和下载逻辑。

## 本轮内容

- 播放栏新增沉浸播放入口，在主窗口内全屏切换，不再创建额外窗口。
- 点击首页歌曲封面、播放条封面或沉浸按钮进入；背景使用封面高斯模糊，中间显示大封面、标题、UP 主和音质标签。
- 有字幕歌词时右侧以无边框悬浮文字显示滚动歌词和当前行高亮；没有歌词时右侧完全收起。
- 底部律动条读取 GStreamer `spectrum` 元素输出的实时频谱幅值；频谱插件不可用时自动降级为静态波形，不影响播放。
- 播放队列通过底部队列按钮按需展开。
- 播放 / 暂停、上一首 / 下一首、进度拖动、收藏、下载。
- 队列列表点播，当前曲目高亮。
- `Esc` 或右上角下箭头返回主界面。
- `Ctrl+Shift+P` 可以直接打开沉浸播放页。
- 歌词时间轴新增活动行定位单元测试。

## 验证方式

```sh
cargo fmt --check
cargo test --release --locked -j 2
cargo clippy --all-targets --locked -j 2 -- -D warnings
```

桌面视觉检查：

```sh
XDG_DATA_HOME="$(mktemp -d /tmp/bili-test-XXXXXX)" \
BILI_MUSIC_TEST=1 \
BILI_MUSIC_SCREENSHOT=/tmp/buisc-beta.png \
cargo test --release --locked native_layout_preview -- --ignored --nocapture
```

## 已知边界

- 歌词仍然只来自 B 站视频字幕，没有接入外部 LRC 或歌词搜索。
- 沉浸页作为主窗口内覆盖层，不改变窗口尺寸，也不改变 MPRIS 播放状态。
- 当前仍只播放视频第一 P，下载队列和多 P 连播留到后续分支。
