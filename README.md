# httphunter

一个基于 Rust 的本地 HTTP/HTTPS 抓包代理。支持 HTTP/1.1 转发、HTTPS MITM、浏览器抓包、Web 控制台和 HAR 导出。

> 仅在你拥有或已获得授权的设备、服务和流量上使用。HTTPS MITM 可读取 Cookie、Token 和请求正文等敏感信息。

## 快速开始

### 1. 安装 Rust（MacOS）

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustc --version
```

如提示缺少编译工具，先运行：

```bash
xcode-select --install
```

### 2. 初始化项目

```bash
git clone <repository-url> httphunter
cd httphunter
cp config.example.toml config.toml
cargo check
```

`cargo` 会自动下载并编译所需依赖；请保留仓库中的 `Cargo.lock`。

### 3. 生成 HTTPS MITM 证书

```bash
cargo run -- ca generate
```

证书默认位于：

```text
~/Library/Application Support/httphunter/ca.crt
```

### 4. 安装并信任证书

打开证书：

```bash
open "$HOME/Library/Application Support/httphunter/ca.crt"
```

在“钥匙串访问”中将 `httphunter local CA` 添加到“登录”钥匙串，双击该证书，在“信任”中把“使用此证书时”设置为“始终信任”。随后完全退出并重新打开浏览器（`Command + Q`）。

### 5. 启动代理和 Web UI

```bash
RUST_LOG=info cargo run -- --config config.toml proxy
```

默认地址：

```text
代理：   http://127.0.0.1:8080
Web UI： http://127.0.0.1:9090/
```

打开 Web UI 后点击顶部 **Start**：httphunter 会开始保存新会话，并将 macOS 当前 `network_service` 的 HTTP/HTTPS 系统代理设置为 `127.0.0.1:8080`。点击 **Stop** 会暂停保存会话，并关闭这两项系统代理。首次使用前，请确认 `config.toml` 中的 `system_proxy.network_service` 与 macOS 网络服务名称一致（通常是 `Wi-Fi`）。

若不使用 Web UI 的 **Start** 开关，浏览器或 macOS 系统代理的 HTTP 和 HTTPS 代理都应设置为 `127.0.0.1:8080`。代理绕过列表应包含：

```text
localhost,127.0.0.1,::1
```

若使用 Clash TUN，保持 TUN 模式开启即可；httphunter 的外部连接会由 TUN 接管。

使用 Web UI 顶部的 **Start** 时，无需手动设置系统 HTTP/HTTPS 代理；按钮会自动设置它们。结束使用前点击 **Stop**，以关闭系统代理并暂停抓包。

## Windows 快速开始

1. 从 <https://rustup.rs/> 下载并运行 `rustup-init.exe`，安装完成后重新打开 PowerShell。
2. 克隆项目并初始化：

```powershell
git clone <repository-url> httphunter
cd httphunter
Copy-Item config.example.toml config.toml
cargo check
```

3. 生成本地 CA：

```powershell
cargo run -- ca generate
```

证书通常位于：

```text
%LOCALAPPDATA%\httphunter\ca.crt
```

4. 按 `Win + R`，输入 `certmgr.msc`；在“受信任的根证书颁发机构 → 证书”中导入 `ca.crt`。完全退出并重新启动 Chrome/Edge。
5. 启动：

```powershell
$env:RUST_LOG = "info"
cargo run -- --config config.toml proxy
```

6. 将 Windows 系统的 HTTP 和 HTTPS 代理设置为 `127.0.0.1:8080`，并在代理绕过列表加入 `localhost;127.0.0.1;::1`。Web UI 地址仍是 <http://127.0.0.1:9090/>。

Windows 上使用 Clash TUN 时，保持 TUN 启用即可；部分 Clash 客户端可能需要管理员权限安装 TUN 驱动。

## 使用 Web UI

打开 <http://127.0.0.1:9090/>。

- 左侧查看请求列表，可按 Host、方法、状态码过滤。
- 勾选 **Hide static** 隐藏图片、CSS、JS 等静态资源；勾选 **Group host** 按域名分组。
- 点击请求，在右侧查看概览、请求/响应 Header、Body 和原始 JSON。
- 点击 **Download HAR** 下载当前内存中的抓包记录。

会话仅保存在内存中，重启进程或点击 **Clear** 后会清空。

## 开发时自动重启

首次安装文件监控工具：

```bash
cargo install cargo-watch
```

之后开发时只需执行：

```bash
make dev
```

保存 `src/`、`Cargo.toml` 或 `config.toml` 后，代理会自动重新编译并重启；不需要再手动执行 `cargo check` 或 `cargo run`。重启会清空内存中的抓包会话，浏览器中的 Web UI 需要刷新。

## 桌面应用开发

项目正在迁移到 Tauri 2 桌面应用。现有的 CLI 代理和浏览器 Web UI 保持可用；新的桌面代码位于 `desktop/`，将逐步替换内嵌 Web UI。

桌面开发需要 Node.js 20 或 22 LTS。推荐通过 nvm 安装 Node 22：

```bash
nvm install 22
nvm use 22
```

首次安装桌面前端依赖：

```bash
make web-install
```

仅启动 React UI：

```bash
make web-dev
```

启动 Tauri 桌面窗口：

```bash
make desktop-dev
```

当前桌面窗口用于验证 React、Tauri 和 Rust core 的通信链路。代理会话列表、证书管理和系统代理开关将在后续迁移中接入。macOS 打包会生成 `.app`/`.dmg`，Windows 打包会生成 `.msi` 等安装包；发布前仍需要分别处理代码签名和证书安装权限。

## 配置

`config.toml` 基于 `config.example.toml` 创建。`mitm_exclude` 中的域名会使用普通 CONNECT 隧道：网站仍可访问，但该域名的 HTTPS 内容不会被解密或抓取。

```toml
mitm_exclude = ["bilibili.com"]
```

这会匹配 `bilibili.com` 及其子域名，如 `t.bilibili.com`、`api.bilibili.com`。

## 命令行测试

HTTP：

```bash
curl --proxy http://127.0.0.1:8080 http://example.com/
```

HTTPS：

```bash
curl --proxy http://127.0.0.1:8080 \
  --cacert "$HOME/Library/Application Support/httphunter/ca.crt" \
  https://example.com/
```

查看会话：

```bash
curl -s http://127.0.0.1:9090/sessions | jq
```

导出 HAR：

```bash
curl http://127.0.0.1:9090/export/har -o capture.har
```

## 当前限制

- HTTPS MITM 仅支持 HTTP/1.1；暂不支持 HTTP/2、HTTP/3/QUIC 和 WebSocket 深度解析。
- 请求和响应 Body 当前保存在内存中；不适合抓取大文件或长时间流式响应。
- `mitm_exclude` 域名只做隧道转发，不记录 HTTPS 内部请求。
