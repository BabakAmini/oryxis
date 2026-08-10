<p align="center">
  <img src="resources/logo_128.png" width="120" alt="Oryxis logo">
</p>

<h1 align="center">Oryxis</h1>

<p align="center">
  完全以 Rust 打造的現代 SSH 用戶端。快速、加密、原生。
</p>

<p align="center">
  <a href="README.md">English</a> | <a href="README.zh-CN.md">简体中文</a> | 繁體中文 | <a href="README.ja.md">日本語</a> | <a href="README.ko.md">한국어</a> | <a href="README.fa.md">فارسی</a> | <a href="README.pt-BR.md">Português (BR)</a>
</p>

<p align="center">
  <a href="https://github.com/wilsonglasser/oryxis/releases/latest"><img src="https://img.shields.io/github/v/release/wilsonglasser/oryxis?color=green" alt="Release"></a>
  <img src="https://img.shields.io/badge/platforms-linux%20%7C%20macos%20%7C%20windows-blue" alt="Platforms">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0-blue" alt="License"></a>
  <a href="https://oryxis.app"><img src="https://img.shields.io/badge/website-oryxis.app-3CBBB1" alt="Website"></a>
</p>

<p align="center">
  <img src="resources/screen_1.gif" width="720" alt="Oryxis 實際操作：連線主機、執行程式碼片段、瀏覽 SFTP">
</p>

> 本文件譯自 v0.13.0 之後的英文 README（2026-08-10 同步），採用台灣慣用詞彙。
> 詳細文件（[功能總覽](docs/FEATURES.md)、[架構說明](docs/ARCHITECTURE.md)）為英文。

## Oryxis 是什麼？

Oryxis 是 [Termius](https://termius.com/) 的開源替代品：一款桌面 SSH
用戶端，擁有現代化介面與保存憑證的本機加密保險庫，整個流程沒有任何
雲端帳號。沒有 Electron、沒有 webview、沒有廠商伺服器，只有一個原生
二進位檔。

|  | Oryxis | Termius | PuTTY | Tabby |
|--|--------|---------|-------|-------|
| 介面技術 | 原生 Rust（iced + wgpu） | Electron | 原生 | Electron |
| 授權條款 | AGPL-3.0，開源 | 專有 | MIT | MIT |
| 憑證儲存 | 本機加密保險庫 | 廠商雲端帳號 | 無 | 本機設定檔 |
| 多裝置同步 | P2P 端對端加密，可自架中繼 | 廠商雲端（訂閱制） | 無 | 透過 Tabby Web |
| SFTP 圖形介面 | 內建雙欄 | 付費方案 | 僅命令列 | 基本面板 |
| 價格 | 免費 | 免費版 + 訂閱 | 免費 | 免費 |

## 安裝

**Windows**

[![從 Microsoft Store 取得](https://get.microsoft.com/images/zh-tw%20dark.svg)](https://apps.microsoft.com/detail/9NTKPPSHBTG2)

或使用終端機：

```powershell
winget install WilsonGlasser.Oryxis
```

**Arch Linux (AUR)**

```bash
yay -S oryxis-bin
```

**直接下載**：前往[最新版本頁](https://github.com/wilsonglasser/oryxis/releases/latest)，
提供 Linux（`.tar.gz` / `.deb` / `.AppImage`，x86_64 與 ARM64）、
macOS（Apple Silicon `.dmg`）和 Windows（系統層級與使用者層級安裝程式、
可攜版 `.zip`，x86_64 與 ARM64）。Windows 二進位檔已完成 Authenticode
簽章。

### 字型與編碼

首次將介面語言切換為繁體中文時，會自動下載 Noto Sans TC 字型
（按需下載，不會增加安裝程式的體積）。連線舊式裝置（網路設備、
工控主機等）時，可在主機編輯器中為個別主機選擇 Big5 等傳統編碼。

介面的繁體中文翻譯以台灣慣用詞彙撰寫（伺服器、檔案、網路、連接埠），
而不是簡體字的機械轉換。

## 亮點

- **原生且快速**：純 Rust、GPU 加速的 [iced](https://iced.rs) 介面、
  單一二進位檔。沒有 Electron、沒有 webview。
- **本機加密保險庫**：Argon2id + ChaCha20-Poly1305 欄位級加密、可選
  主密碼、生物辨識解鎖（Windows Hello / Touch ID / Linux 金鑰環）、
  閒置自動鎖定、TOTP 兩步驟驗證自動填入，以及在 `sudo` 提示時
  提供保險庫密碼（絕不自動送出）。
- **完整的 SSH 能力**：自動驗證、多層跳板機、SOCKS / HTTP / 命令
  代理、Agent 轉發、獨立的 `-L`/`-R`/`-D` 連接埠轉送、面向選單式
  跳板機（JumpServer 等）的 expect/send 登入指令碼、一鍵匯入
  `~/.ssh/config`。
- **不只 SSH**：Telnet 與序列埠主控台、ZMODEM 傳輸、本機 Shell，
  以及透過 SSH 隧道一鍵開啟 RDP/VNC。
- **真正的終端機**：以 alacritty 為基礎的模擬器、分割窗格、工作階段
  群組、依主機套用主題、內建 Nerd 字型外加可下載字型包
  （JetBrains Mono、Fira Code、MesloLGS 等）、標示長時間執行指令的
  智慧分頁、依主機保存的指令歷史。
- **檔案無所不在**：雙欄 SFTP 支援拖放、就地編輯、伺服器對伺服器
  複製；每個 SSH 分頁還有跟隨 Shell 工作目錄的檔案側欄。
- **工作階段錄製**：靜態加密儲存；可匯出 asciinema `.cast`（內嵌
  主題）或純文字逐字稿，設計上僅錄製輸出。
- **雲端帳號**：AWS、Google Cloud、Azure 與 Kubernetes 的資源探索
  與連線（EC2、SSM、ECS Exec、GKE、AKS、`kubectl`），以簽章外掛
  按需下載。
- **AI 隨侍在側**：每個分頁的 AI 助手（自備金鑰：Anthropic、OpenAI、
  Gemini 或相容服務），多層自動執行安全控管，另有
  [MCP 伺服器](docs/FEATURES.md#mcp-server)可將主機開放給
  Claude Code 等 AI 用戶端。
- **P2P 同步，無雲端**：端對端加密（X25519 + XChaCha20-Poly1305），
  基於 QUIC；區域網路內以 mDNS 探索，跨網路可[自架](SELF_HOSTING.md)
  信令/中繼。沒有帳號，沒有廠商伺服器。
- **鍵盤優先**：`user@host` 快速連線（Ctrl+K）、最近使用分頁切換、
  涵蓋到最後一個開關的完整鍵盤導覽、所有快速鍵皆可重新綁定。
- **隱私至上**：沒有任何遙測、隱私模式遮罩、貼上前讓你確認內容的
  貼上防護，以及含完整 RTL 支援的
  [23 種語言](docs/FEATURES.md#themes--internationalization)：English、
  Português、Español、Français、Deutsch、Italiano、简体中文、繁體中文、
  日本語、Русский、فارسی、العربية、עברית、한국어、Polski、Türkçe、
  Bahasa Indonesia、Tiếng Việt、Українська、ไทย、हिन्दी、Čeština、Ελληνικά。

完整功能清單見英文[功能總覽](docs/FEATURES.md)。
在用 tmux？**[tmux 下的日誌與命令歷史](docs/TMUX.md)**（英文）說明了哪些功能開箱即用、哪些需要自行安裝。
想讓檔案瀏覽器精確跟隨 shell 的目錄？**[跟隨 shell 的目錄](docs/CWD.md)**（英文）提供了程式碼片段。

## 快速上手

1. **首次啟動**：設定主密碼，或先跳過（之後可在設定中開啟，並啟用
   生物辨識解鎖）。
2. **新增主機**：點擊 `+ HOST`，或直接輸入 `user@host`（Ctrl+K）
   免儲存連線。`~/.ssh/config` 一鍵匯入。
3. **連線**：點擊主機卡片。分割窗格、檔案側欄、SFTP 和程式碼片段
   都只有一個按鍵的距離。
4. **可選擴充**：AI 聊天（設定 > AI）、MCP 伺服器（設定 > 安全性）、
   裝置間 P2P 同步（設定 > 同步）。

有問題？看看 [FAQ](https://github.com/wilsonglasser/oryxis/discussions/66)，
或發起[討論](https://github.com/wilsonglasser/oryxis/discussions)。

## 安全性

所有敏感資料均以欄位級加密儲存（Argon2id + ChaCha20-Poly1305），主機
金鑰採 TOFU 釘選，同步資料端對端加密，外掛在執行前經過 Ed25519 簽章
驗證，而且沒有任何遙測。

完整的安全模型與弱點揭露政策見 [SECURITY.md](SECURITY.md)。請透過
私密管道回報安全弱點。

## 開發藍圖

Oryxis 以大約每週一次的節奏持續發布，功能就緒即上線。最新穩定版為
**v0.13.0**；完整歷史見 [CHANGELOG.md](CHANGELOG.md)，互動式藍圖見
[藍圖討論](https://github.com/wilsonglasser/oryxis/discussions/67)。
正在推進的方向包括：原生 FIDO2（透過 USB / NFC 直接與安全金鑰通訊）、
原生 Mosh 用戶端、多保險庫，
以及東亞全形寬度選項。社群呼聲很高的側邊欄 tmux 工作階段管理
器和主機樹狀檢視已在本版本推出。

## 參與貢獻

歡迎貢獻。**可以直接用中文開 issue 或參與討論**，維護者會閱讀並盡力
回覆；程式碼、commit 訊息與程式註解請使用英文。開發環境、品質門檻與
專案慣例見 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 授權條款

Copyright (C) 2026 Wilson Glasser。依
[AGPL-3.0-or-later](LICENSE) 授權發布：任何人都可以使用、修改與散布
Oryxis，但透過網路提供的修改版本必須以相同授權公開其原始碼。詳見
[NOTICE](NOTICE)。

---

<p align="center">
  以 Rust 打造，獻給以終端機為家的人。
</p>
