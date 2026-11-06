# 参与贡献

欢迎提交问题、功能建议和 Pull Request。请先查看 [现有 Issues](https://github.com/xieyh2002/Buisc/issues)，避免重复提交。

## 提交问题

请注明 Buisc 版本、Linux 发行版和架构（amd64 或 arm64），并描述复现步骤、预期结果和实际结果。如果问题与显示有关，请一并注明 Wayland 或 X11、GPU 型号和「设置与备份」里显示的渲染器。附上截图前请遮住个人信息，日志中请勿包含 Cookie、二维码或本地账号文件。

## 提交代码

1. 按 [README](README.md#编译与运行) 准备 Rust、GTK4 和 GStreamer 依赖。
2. 从 `main` 开分支，尽量将每个 PR 限定在一个问题。
3. 提交前运行以下检查：

   ```sh
   cargo fmt --all --check
   cargo clippy --all-targets --locked -- -D warnings
   cargo test --release --locked
   ```

4. 在 PR 中说明问题、改动和验证方式。涉及界面时，附上截图；涉及桌面托盘时，可运行 `./scripts/test-tray.sh`。
