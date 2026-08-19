# httphunter

一个基于 Rust 的本地 HTTP/HTTPS 抓包工具，当前桌面端使用 Tauri 2、React 和 TypeScript 构建。

它在本机启动 `127.0.0.1:8080` 代理，通过本地 CA 解密 HTTPS 流量，并在桌面窗口中查看请求、响应、Header 和 Body。

> 仅用于你拥有或已获得明确授权的设备、服务和流量。HTTPS MITM 能看到 Cookie、Token、表单和响应正文等敏感数据；当前版本不适合作为默认开启抓包的正式公开发布版本。

## 当前能力

- HTTP/1.1 HTTP/HTTPS 代理与 HTTPS MITM。
- 请求列表、详情页、请求/响应 Header 与 Body、JSON 格式化显示。
- Host、方法、状态码和静态资源过滤。
- macOS 一键开启/关闭系统 HTTP 和 HTTPS 代理。
- 会话只保存在内存，关闭应用或点击 Clear 后清空。

当前不支持 HTTP/2、HTTP/3/QUIC、WebSocket 深度解析或会话持久化。某些启用证书绑定或反爬策略的网站可能不能通过 MITM 正常访问。

## 首次运行（macOS）

### 1. 安装依赖

需要 Rust、Node.js 22 和 Xcode Command Line Tools：

```bash
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

通过 nvm 安装并启用 Node.js 22：

```bash
nvm install 22
nvm use 22
node --version
rustc --version
```

### 2. 获取项目并安装前端依赖

```bash
git clone <repository-url> httphunter
cd httphunter
make web-install
```

`make web-install` 根据 `package-lock.json` 安装 React、Vite 和 Tauri 前端依赖。Rust 依赖会在第一次启动时由 Cargo 自动下载并编译。

### 3. 启动桌面应用

```bash
make desktop-dev
```

首次编译时间较长，之后会打开 `httphunter` 桌面窗口。开发中，前端修改会由 Vite 热更新；Rust/Tauri 修改会触发重新编译和应用重启。停止运行使用终端中的 `Control + C`。

若提示 `failed to bind proxy listener at 127.0.0.1:8080`，说明已有 httphunter 进程或其他程序占用了 8080 端口。先在原来的终端按 `Control + C`；找不到终端时可执行：

```bash
lsof -nP -iTCP:8080 -sTCP:LISTEN
kill <PID>
```

其中 `<PID>` 是第一条命令输出中的进程号。确认该进程确实是旧的 httphunter 后再结束它。

## 信任本地 HTTPS 证书

桌面应用首次打开时会提示生成 Local CA。点击 **Generate local CA** 后，证书位于：

```text
~/Library/Application Support/httphunter/ca.crt
```

在 Finder 或终端中打开它：

```bash
open "$HOME/Library/Application Support/httphunter/ca.crt"
```

在“钥匙串访问”中将 `httphunter local CA` 添加到“登录”钥匙串。双击该证书，展开“信任”，把“使用此证书时”设为“始终信任”。完成后完全退出并重新打开 Chrome 或其他浏览器（`Command + Q`）。

没有信任该证书时，浏览器会显示 `ERR_CERT_AUTHORITY_INVALID`，这是 HTTPS MITM 的预期保护行为，不应绕过证书警告继续登录或提交敏感信息。

## 使用桌面应用

1. 打开桌面窗口后，先生成并信任 Local CA。
2. 点击顶部的 **Stopped**，它会变为绿色的 **Capturing**，开始抓包。
3. 在 macOS 上，此开关同时将当前网络服务的 HTTP 和 HTTPS 系统代理设为 `127.0.0.1:8080`。浏览器流量会自动进入 httphunter。
4. 左侧列表按最新请求在上方排序。使用 **Filters** 按 Host、Method、Status 过滤，或隐藏静态资源。
5. 点击一个请求，在右侧查看 Overview、请求/响应 Header、Body 和原始 JSON。
6. 结束时再次点击 **Capturing**，使其回到 **Stopped**。这会停止抓包并关闭 httphunter 设置的系统代理。

桌面端当前使用固定的 `Wi-Fi` 网络服务和 `127.0.0.1:8080`。开始抓包会覆盖该网络服务原有的 HTTP/HTTPS 代理配置，停止时只会关闭 httphunter 代理，**不会恢复原有代理地址**。使用 Clash、公司代理或其他系统代理前，请先记录原配置；异常退出时也应在“系统设置 -> 网络 -> Wi-Fi -> 详情 -> 代理”中检查并关闭 HTTP/HTTPS 代理。

Clash TUN 可以保持开启：httphunter 接收浏览器的本地代理流量，其后续外连通常会经过 TUN。若遇到无法访问、证书固定或风控页面，应先停止抓包，或将该域名排除在 HTTPS MITM 之外（目前此项通过 CLI 配置）。

## Windows

桌面 UI 可以在 Windows 构建和运行，但当前“开始/停止抓包”自动控制系统代理仅实现了 macOS。因此 Windows 上需要手动生成并信任证书、手动设置系统代理。

前置条件：安装 [Rust](https://rustup.rs/)、Node.js 22 LTS、Microsoft C++ Build Tools 和 Microsoft Edge WebView2 Runtime（Windows 11 通常已自带）。PowerShell 中首次运行：

```powershell
git clone <repository-url> httphunter
cd httphunter
npm --prefix desktop/web install
cd desktop/src-tauri
../web/node_modules/.bin/tauri.cmd dev
```

在应用中生成证书后，可按 `Win + R`，输入 `certmgr.msc`，将 `ca.crt` 导入“受信任的根证书颁发机构 -> 证书”。证书通常在：

```text
%LOCALAPPDATA%\httphunter\ca.crt
```

然后在 Windows 系统代理中手动将 HTTP 和 HTTPS 代理设为 `127.0.0.1:8080`，绕过地址加入 `localhost;127.0.0.1;::1`。停止抓包后手动关闭这些代理。

## 构建安装包

### 本机构建

先在本机验证当前平台的安装包：

```bash
make desktop-build
```

Tauri 会先构建前端，再生成当前平台的安装包。产物位于：

```text
desktop/src-tauri/target/release/bundle/
```

在 macOS 上会生成 `.dmg`，Windows 上会生成 `.msi` 和 `.exe` 安装包。Tauri 不能可靠地在一个平台上交叉构建另一个平台的安装包，因此 macOS 包应在 macOS 构建，Windows 包应在 Windows 构建。

### 发布到 GitHub Releases

仓库提供 [release.yml](.github/workflows/release.yml)：向 GitHub 推送 `v` 开头的版本标签后，GitHub Actions 会在 macOS 和 Windows 云端分别构建，再自动创建 Release 并上传 `.dmg`、`.msi` 与 `.exe`。

发布前必须让以下三个版本号一致：

- `desktop/src-tauri/tauri.conf.json`
- `desktop/src-tauri/Cargo.toml`
- `desktop/web/package.json`

例如发布 `0.1.0`：

```bash
git status
git add <本次发布需要提交的文件>
git commit -m "release: v0.1.0"
git push origin main
git tag v0.1.0
git push origin v0.1.0
```

在 GitHub 的 **Actions -> Release desktop application** 查看构建进度。两个平台构建成功后，安装包会出现在 [Releases](https://github.com/valderer/httphunter/releases)。若需要从 Actions 手动发布，在目标提交或分支上选择 **Run workflow**，输入版本标签如 `v0.1.0`；工作流会在该提交创建或更新对应的标签和 Release。

当前 CI 构建的是未签名安装包。macOS 用户可能会看到 Gatekeeper 提示，Windows 用户可能会看到 SmartScreen 提示。面向普通用户正式分发前，应配置 Apple Developer 签名和公证，以及 Windows 代码签名证书。不要将本机生成的 `ca.crt`、`ca.key` 或包含抓包内容的文件打包、上传或提交到仓库。

## CLI 兼容模式

旧的命令行代理和浏览器 Web UI 仍保留，适合调试核心代理。复制配置并启动：

```bash
cp config.example.toml config.toml
cargo run -- ca generate
RUST_LOG=info cargo run -- --config config.toml proxy
```

默认代理为 `127.0.0.1:8080`，旧 Web UI 为 <http://127.0.0.1:9090/>。`config.toml` 中的 `mitm_exclude` 会使指定域名及其子域名使用普通 CONNECT 隧道：网站可访问，但其 HTTPS 内容不会被解密或捕获。

```toml
[proxy]
mitm_exclude = ["bilibili.com"]
```

开发旧 CLI 时可安装 `cargo-watch` 后运行 `make dev`：

```bash
cargo install cargo-watch
make dev
```

## 项目结构

```text
crates/hunter-core/   共享代理、HTTPS MITM、证书和会话核心
desktop/web/          React + TypeScript + Vite 界面
desktop/src-tauri/    Tauri 桌面壳和 Rust 命令
src/                  旧 CLI 与 Axum Web UI
```

## 安全说明

- 代理只监听 `127.0.0.1`，不会直接暴露到局域网。
- 会话目前只放在内存中，但抓包期间任何能访问应用窗口的本机用户都可看到其内容。
- HTTPS MITM 的本地 CA 私钥位于应用数据目录。不要分享该私钥，也不要在不可信设备上信任该 CA。
- 当前配置中的 Header 脱敏和 Body 大小限制尚未完整接入所有抓包路径。使用时应假定敏感 Header 与 Body 可能被完整保存在内存中。
- 不要在生产账号、支付、身份认证等敏感流程上长时间开启抓包。完成调试后停止抓包并移除对本地 CA 的信任。
