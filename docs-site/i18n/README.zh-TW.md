<p align="center">
  <img src="../static/img/nyaterm.svg" alt="NyaTerm" width="128" height="128">
</p>

<h1 align="center">NyaTerm</h1>

<p align="center">
  <em>/ˈnjaː tɜːrm/</em>
</p>

<p align="center">
  <strong>以 GPUI 打造的現代原生遠端終端機工作區。</strong><br/>
  <a href="https://nyaterm.app"><strong>nyaterm.app</strong></a> ·
  <a href="https://nyaterm.app/docs/"><strong>線上文件</strong></a>
</p>

<p align="center">
  在同一個桌面用戶端中使用 SSH、本機 Shell、Telnet、序列埠、RDP、VNC、SFTP、通道、OTP、AI 輔助與加密同步。
</p>

<p align="center">
  <a href="https://discord.gg/Tgb9ugMKdM" target="_blank"><img src="https://dcbadge.limes.pink/api/server/Tgb9ugMKdM" alt="Discord" /></a>
  &nbsp;
  <a href="https://docs.qq.com/doc/DYW5TUkZnakxOWUFW" target="_blank"><img src="https://img.shields.io/badge/WeChatGroup-Join-07C160?style=for-the-badge&logo=wechat&logoColor=white" alt="微信交流群" /></a>
  <br/>
  <a href="https://linux.do" target="_blank" rel="noopener noreferrer">
    <img src="https://img.shields.io/badge/LINUX%20DO-Forum-F0B003?style=for-the-badge&logo=discourse&logoColor=white" alt="LINUX DO" />
  </a>
  <br/>
  <a href="https://nyaterm.app"><img alt="版本" src="https://img.shields.io/github/v/release/nyakang/nyaterm?style=flat-square&logo=github&color=0EA5E9&labelColor=334155"></a>
  &nbsp;
  <a href="https://github.com/nyakang/nyaterm/releases"><img alt="GitHub 下載次數" src="https://img.shields.io/github/downloads/nyakang/nyaterm/total?style=flat-square&logo=github&color=0EA5E9&label=Downloads&labelColor=334155"></a>
  &nbsp;
  <a href="#"><img alt="平台" src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-0EA5E9?style=flat-square&logo=linux&labelColor=334155"></a>
  &nbsp;
  <a href="../../LICENSE"><img alt="授權條款" src="https://img.shields.io/badge/License-Apache%202.0-0EA5E9?style=flat-square&logo=readthedocs&labelColor=334155"></a>
</p>

<p align="center">
  <a href="../../README.md">English</a> · <a href="./README.zh-CN.md">简体中文</a> · <strong>繁體中文</strong> · <a href="./README.ja.md">日本語</a> · <a href="./README.ko.md">한국어</a> · <a href="./README.fr.md">Français</a> · <a href="./README.pt-BR.md">Português (Brasil)</a>
</p>

---

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/product-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/product-light.png">
    <img alt="NyaTerm 主工作區" src="../static/img/home/product-light.png">
  </picture>
</p>

---

<a name="ai-assistant"></a>
# AI 助理

NyaTerm 內建 AI 助理面板，可用於產生命令、解釋終端機輸出、分析錯誤，以及執行多步驟的終端機工作流程。

## 功能介紹

- **Ask 模式**：提供單次協助，例如產生命令、解釋選取的輸出及分析錯誤
- **Agent 模式**：針對目前使用中的終端機工作階段，透過「觀察、決策、執行」循環完成多步驟任務
- **最近輸出操作**：直接請 AI 解釋最新的終端機輸出，無須手動複製
- **結構化命令卡片**：顯示風險等級與執行控制，並可儲存為快捷命令
- **經核准的命令執行工具**：支援 Agent 工作流程，在終端機任務完成後另行提供最終回答
- **行內終端機擷取**：在 Agent 執行期間擷取輸出，並可透過 `Terminal Output Lines` 設定行數
- **工作階段提及**：使用 `@` 將其他終端機工作階段納入 AI 上下文
- **供應商管理**：支援內建供應商、手動新增模型、憑證群組及自訂 OpenAI 相容端點
- **風險控制**：對高影響命令提供核准機制與更安全的替代方案

---

<a name="what-is-nyaterm"></a>
# NyaTerm 是什麼

**NyaTerm** 是以 SSH 維運與混合終端機工作流程為核心的原生 GPUI 桌面用戶端。它透過 Rust 工作區，將桌面介面、終端機繪製、傳輸協定、資料持久化、驗證、網路工具、AI 輔助終端機操作、工作階段匯入／匯出、診斷及加密同步／備份整合於同一個應用程式。

- **NyaTerm 是**面向開發者、系統管理員與 DevOps 工程師的 SSH 用戶端
- **NyaTerm 是**支援分頁、水平與垂直分割的終端機工作區
- **NyaTerm 是**提供傳輸佇列與「本機編輯後上傳」流程的 SFTP 檔案瀏覽器
- **NyaTerm 支援** SSH、本機終端機、Telnet、序列埠、RDP 與 VNC 工作階段

---

<a name="why-nyaterm"></a>
# 為什麼選擇 NyaTerm

NyaTerm 適合每天在伺服器、本機命令、裝置與設定檔之間切換的使用者。

- **以工作區為核心**：透過分頁、分割面板、側邊面板與子視窗，集中管理相關的終端機
- **在工作階段中處理遠端操作**：瀏覽 SFTP 檔案、跟隨終端機路徑、執行傳輸及編輯遠端檔案
- **兼顧安全的工作流程**：管理憑證、金鑰、已知主機、OTP、鎖定畫面及受主密碼保護的儲存空間
- **可攜式設定**：從既有工具匯入資料、匯出加密 `.nya` 備份，並透過 WebDAV 或 S3 相容儲存空間同步加密快照
- **實用的 AI 輔助**：根據目前終端機的上下文產生命令、檢查輸出，並執行經核准的多步驟操作

---

<a name="features"></a>
# 功能

## 工作階段與工作區

- 支援 SSH、本機終端機、Telnet、序列埠、RDP 與 VNC 工作階段
- 多分頁工作區，支援水平與垂直分割面板、拖曳分頁停駐及還原版面配置
- RDP 與 VNC 遠端桌面面板；VNC 目前支援直接 TCP 連線、None／傳統 VNC Auth 驗證、Raw／ZRLE／Tight／Tight JPEG 畫面緩衝區更新、視窗縮放、有限次數的重新連線，以及 Latin-1 文字的剪貼簿交換
- 已儲存連線支援資料夾、圖示、中繼資料、複製連線、鍵盤複製、重新連線與匯入流程
- 命令選擇區與工作階段快速切換器，可尋找操作、已開啟工作階段、已儲存連線及新增工作階段的入口
- 主視窗可自訂 `Background Image`，提供 `cover`／`contain`／`stretch`／`tile` 尺寸模式，並可調整 `Background Content Opacity`
- 左右活動列提供檔案總管、網路、安全／驗證、同步與備份、AI 助理、筆記、已儲存連線、使用中的工作階段、命令歷史、資產監控、資源監控、GPU 監控、程序管理員與 Docker 管理員
- 筆記面板與編輯器支援樹狀導覽、快顯功能表、自動儲存、工具列控制，以及納入同步／備份的資料持久化
- 資產監控工作區支援分組連線檢視、麵包屑導覽、表格／卡片版面，以及資源／GPU／NPU 狀態整合
- SSH 工作階段的遠端主機監控面板：資源監控、NVIDIA GPU 與 Ascend NPU 總覽、程序管理員（signal／renice）及 Docker 管理員（容器、映像檔、磁碟區、網路、Compose）
- 工作階段輸入同步群組，可將鍵入內容與傳送的命令同時廣播至多個工作階段
- 貼上 `ssh://` URL 或 `ssh` 命令即可建立臨時 SSH 連線，無須儲存連線設定
- 設定、新增工作階段、快捷命令編輯、遠端檔案編輯及自動上傳提示使用獨立子視窗
- 支援系統匣，可選擇最小化至系統匣及隱藏主視窗

## 終端機體驗

- 終端機搜尋支援結果導覽、搜尋歷史、複製／貼上、快顯功能表及選取文字操作
- 命令歷史提供模糊建議、可設定的長度篩選，並會在互動式程式中停用建議
- Unicode 字素繪製支援 emoji、組合附加符號與 ZWJ 序列
- 可選擇顯示行號與時間戳記邊欄
- 可為 IPv4 位址、`host:port` 及封存檔名啟用操作連結
- 可選擇啟用關鍵字醒目提示，提供擴充的內建預設、自訂規則與 JSON 匯入
- 終端機縮放、工作區內距、字型粗細控制、原生輸入法支援、圖片路徑貼上處理及清除終端機操作
- AI 快捷操作可解釋最近的輸出，並支援行內顯示 Agent 命令輸出，行數可由 `Terminal Output Lines` 設定
- 大量輸出保護、可設定的回捲緩衝區、SSH 連線保活及工作階段錄製
- 對選取的終端機文字進行線上搜尋與翻譯
- 可直接從終端機使用 Zmodem 傳輸檔案，並在傳輸佇列中顯示
- 關閉所有工作階段前顯示確認對話方塊
- 可自訂終端機與介面操作的鍵盤快捷鍵，並為 Telnet 與序列埠工作階段選擇 `Backspace Mode`

## SFTP 與檔案工作流程

- SSH 工作階段內建 SFTP 檔案總管
- 支援上傳、下載、重新命名、移動、刪除、內容檢視、新增檔案／資料夾，以及 OpenSSH 相容的符號連結操作
- 資料夾上傳／下載、多資料夾上傳選取、多選、可編輯路徑列，以及與終端機目前工作目錄手動／自動同步
- 傳輸佇列支援速度顯示、暫停、繼續、取消、重試、重複目標處理、時間戳記保留及可設定的並行數
- 強化 SCP 與目錄傳輸處理，並在可行時保留原始屬性
- 在本機編輯器開啟遠端檔案，透過檔案監看驅動的自動上傳流程回傳已儲存的變更；使用內容指紋，只有實際內容變更才會觸發重新上傳
- 多分頁唯讀遠端檔案預覽，支援文字、JSON、Markdown、CSV／TSV、圖片及 PDF，提供各分頁的工作階段標籤、縮放／旋轉／適應視窗控制與分頁 PDF 繪製
- 預覽載入與解碼在繪製執行緒之外執行，並為各格式設定大小限制；檔案過大或格式不支援時會顯示清楚訊息，避免阻塞介面
- 限制 SFTP 通道並行數，並在暫時性的通道開啟失敗時自動重試
- Windows 支援從外部拖放檔案上傳
- Zmodem 傳輸會追蹤本機路徑，並可在系統檔案管理員中顯示已完成的檔案

## 安全、驗證與網路

- 密碼驗證、私密金鑰、主機金鑰驗證及加密的本機資料持久化
- 憑證管理支援以正規表示式為基礎的終端機自動填入
- OTP 管理支援 TOTP／HOTP、QR 碼匯入及 SSH 自動填入
- 每個連線可設定 SSH 演算法偏好，提供相容／安全／自訂模式，並為金鑰交換、加密演算法、MAC 與主機金鑰標示安全風險
- 支援 SOCKS5、HTTP 與 ProxyCommand 代理設定；具循環防護的 SSH 跳板主機；本機／遠端／動態通道
- 在具備本機 X 伺服器的環境中支援 SSH X11 轉送
- 鎖定畫面、閒置應用程式鎖定、主密碼、診斷設定、本機日誌管理及診斷套件匯出

## 同步、備份與移轉

- 透過 WebDAV、S3 相容儲存空間與 GitHub Gist 進行加密雲端同步及備份
- 同步、備份、加密匯入／匯出或排程加密備份前，必須先設定主密碼
- 啟動時檢查同步、受支援的本機變更經防抖處理後自動推送、詳細狀態更新及排程備份保留
- 手動測試／推送／拉取／備份操作、還原遠端備份及快照層級的衝突解決
- 從 Xshell、MobaXterm、WindTerm 與 NyaTerm JSON 定義匯入工作階段
- 使用加密 `.nya` 格式匯入／匯出整個應用程式設定，讓 NyaTerm 設定可攜

---

<a name="screenshots"></a>
# 螢幕擷取畫面

## 工作區

在同一個支援分頁與分割面板的工作區中管理 SSH、本機 Shell、Telnet 與序列埠工作階段。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/overview-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/overview-light.png">
    <img alt="NyaTerm 工作區總覽" src="../static/img/home/overview-light.png">
  </picture>
</p>

## 外觀與背景圖片

將本機桌布設為主視窗背景，調整 `Image Sizing`、`Image Opacity` 與 `Background Content Opacity`，並讓子視窗使用實色背景以維持可讀性。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/cover-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/cover-light.png">
    <img alt="NyaTerm 背景圖片自訂" src="../static/img/home/cover-light.png">
  </picture>
</p>

## 終端機增強功能

在終端機中使用命令歷史、搜尋、翻譯、操作連結、時間戳記、關鍵字醒目提示及大量輸出保護。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/terminal-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/terminal-light.png">
    <img alt="NyaTerm 終端機功能" src="../static/img/home/terminal-light.png">
  </picture>
</p>

## 遠端檔案

在終端機旁瀏覽 SFTP 檔案、管理傳輸，並將本機編輯器的變更回傳至遠端路徑。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/files-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/files-light.png">
    <img alt="NyaTerm SFTP 檔案工作流程" src="../static/img/home/files-light.png">
  </picture>
</p>

## 安全與網路工具

透過專用面板管理憑證、OTP、已知主機、代理、跳板主機及 SSH 通道。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/security-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/security-light.png">
    <img alt="NyaTerm 安全與網路工具" src="../static/img/home/security-light.png">
  </picture>
</p>

## 同步與備份

透過 WebDAV 或 S3 相容儲存空間同步加密的可攜式設定快照，並還原備份。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/sync-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/sync-light.png">
    <img alt="NyaTerm 同步與備份" src="../static/img/home/sync-light.png">
  </picture>
</p>

---

<a name="supported-platforms"></a>
# 支援平台

| 作業系統 | 支援範圍 |
| :--- | :--- |
| **Windows** | Windows 10/11，x64／arm64 |
| **macOS** | macOS 12+，Intel／Apple Silicon |
| **Linux** | Ubuntu 20.04+、Fedora 36+、Arch Linux 及類似發行版 |

使用 **glibc 2.31** 的客製化 Ubuntu 20.04 x86-64 桌面，請使用專用的
[focal 離線相容版本](../../scripts/compat/linux-focal/README.md)。
標準 Linux 發行套件是在較新的發行版上建置，可能需要較新版的 glibc。
相容版本包含所有執行階段輔助程式、ABI 報告、
優先使用系統／GPU 的啟動器，以及需明確選用的私有 CPU OpenGL 備援模式；
目標電腦必須具備可正常運作的 X11 工作階段。

請從 [nyaterm.app](https://nyaterm.app) 或 [Releases](https://github.com/nyakang/nyaterm/releases) 頁面下載安裝程式。

---

<a name="supported-session-types"></a>
# 支援的工作階段類型

| 類型 | 典型用途 | 說明 |
|------|-------------|-------|
| SSH | Linux／Unix 遠端伺服器 | 支援 SFTP、OTP、資源／GPU／程序／Docker 監控、代理、跳板主機、通道及個別連線的演算法偏好設定 |
| 本機終端機 | 本機 Shell 工作流程 | 使用本機 Shell 路徑與工作目錄 |
| Telnet | 舊式網路裝置或實驗室系統 | 輕量終端機工作階段，不含 SSH 專屬功能；`Backspace Mode` 可選擇 `Ctrl+H (BS)` 或 `DEL (0x7F)` |
| 序列埠 | 路由器、開發板、嵌入式裝置 | 可設定連接埠、鮑率、資料位元、同位檢查、停止位元與 `Backspace Mode` |

---

<a name="getting-started"></a>
# 開始使用

## 下載

請從 [nyaterm.app](https://nyaterm.app) 或 [Releases](https://github.com/nyakang/nyaterm/releases) 下載適合您平台的最新版本。

| 平台 | 格式 |
|----------|--------|
| Windows | `.exe`／可攜式 `.zip` |
| macOS | `.dmg` |
| Linux | `.deb`／`.AppImage`／`.rpm` |

Windows 可攜版解壓縮 ZIP 後即可執行 `NyaTerm.exe`，資料儲存在旁邊的 `data/` 目錄。GPUI 的**說明 → 檢查更新**使用已簽署的資訊清單，支援為相容的安裝類型下載並安裝更新。Linux deb／rpm 版本透過系統套件管理員，在 Polkit 或 sudo 授權後安裝已簽署的更新，並提供重新啟動選項；相依套件必須事先安裝。Homebrew 與由外部管理的版本使用各自的套件管理員。

**從 1.x 移轉：Windows 可攜版與舊 MSI 安裝需要手動升級。**請先結束應用程式並備份所有資料與金鑰。可攜版應保留 `data/`，並替換完整的程式檔案；MSI 安裝則應先備份、解除安裝舊版，再安裝 NSIS 版本。只替換主執行檔並不足夠，也不保證所有舊安裝程式都能自動移轉。請參閱 [2.0 移轉指南](https://nyaterm.app/en/docs/getting-started/migration-v2)。

### macOS

macOS 使用者可透過 Homebrew 安裝 NyaTerm：

```bash
brew install nyakang/nyaterm/nyaterm
```

此方式使用 [`nyakang/homebrew-nyaterm`](https://github.com/nyakang/homebrew-nyaterm) tap，並安裝 `nyaterm` cask。您也可以從 [nyaterm.app](https://nyaterm.app) 或 [Releases](https://github.com/nyakang/nyaterm/releases) 下載 `.dmg` 安裝程式，再將 NyaTerm 拖曳至 `/Applications`。

NyaTerm 目前尚未使用 Apple 開發者憑證簽署。如果安裝後 macOS 顯示應用程式已損壞或無法開啟，請移除隔離屬性後再開啟：

```bash
sudo xattr -cr /Applications/NyaTerm.app
```

### Arch Linux / AUR

Arch Linux 使用者可從 AUR 安裝 NyaTerm：

```bash
yay -S nyaterm-bin
```

或使用 `paru`：

```bash
paru -S nyaterm-bin
```

您也可以從 AUR 儲存庫手動建置：

```bash
git clone https://aur.archlinux.org/nyaterm-bin.git
cd nyaterm-bin
makepkg -si
```

AUR 套件：[`nyaterm-bin`](https://aur.archlinux.org/packages/nyaterm-bin)

> `nyaterm-bin` 是由社群維護的 AUR 二進位套件。如果 AUR 套件尚未跟上最新版本，請從 [Releases](https://github.com/nyakang/nyaterm/releases) 下載官方套件。


## 開發前置需求

- 透過 [rustup](https://rustup.rs/) 安裝 Rust 穩定版
- GPUI 與您的作業系統所需的平台相依套件

## 開發

```bash
git clone https://github.com/nyakang/nyaterm.git
cd nyaterm
cargo run -p nyaterm-app --bin nyaterm
```

## 專案結構

```text
├── crates/
│   ├── nyaterm-app/             # 原生 GPUI 應用程式進入點與資源
│   ├── nyaterm-desktop/         # GPUI 應用程式組合與功能狀態
│   ├── nyaterm-core/            # 與 UI 無關的領域模型與策略
│   ├── nyaterm-terminal/        # 終端機狀態機與解析
│   ├── nyaterm-terminal-runtime/ # 畫面處理工作執行緒、搜尋與工作階段事件路由
│   ├── nyaterm-terminal-gpui/   # GPUI 終端機版面配置、輸入與繪製
│   ├── nyaterm-transport/       # SSH、PTY、SFTP、Telnet、序列埠與傳輸
│   ├── nyaterm-store/           # 資料持久化與資料庫相容性
│   ├── nyaterm-ui/              # 共用 GPUI 控制項與佈景主題整合
│   ├── nyaterm-remote-desktop/  # 與 UI 無關的 RDP／VNC 工作階段與具型別 IPC
│   ├── nyaterm-rdp-helper/      # 隔離的 IronRDP 協定輔助程序
│   ├── nyaterm-vnc-helper/      # 隔離的 VNC 協定輔助程序
│   └── nyaterm-otp/             # OTP 相容實作
├── docs-site/                   # Docusaurus 文件網站
├── Cargo.toml                   # Rust 工作區定義
└── Cargo.lock
```

---

<a name="credits"></a>
# 致謝

感謝以下專案與函式庫，讓 NyaTerm 得以實現：

- [WindTerm](https://github.com/kingToolbox/WindTerm) — 啟發了 NyaTerm 的設計與功能
- [tabby](https://github.com/Eugeny/tabby) — 優秀的跨平台終端機，提供許多設計靈感
- [Alacritty](https://github.com/alacritty/alacritty) — NyaTerm 使用的終端機模擬元件
- [russh](https://github.com/warp-tech/russh) — 以 Rust 實作的 SSH 用戶端與伺服器

---

<a name="sponsor"></a>
# 贊助

如果 NyaTerm 對您的日常開發、維運或遠端終端機工作有所幫助，歡迎透過[贊助頁面](./en/docusaurus-plugin-content-docs/current/sponsor.md)支持持續維護。

---

<a name="contributors"></a>
# 貢獻者

<a href="https://github.com/nyakang/nyaterm/graphs/contributors">
  <img src="https://contrib-wall.coderkang.workers.dev/image?snapshot=EEZNlyhjBOrARLki-MUhXqKk&amp;sealed_token=eyJ2IjoxLCJ0eXBlIjoic25hcHNob3QiLCJzbmFwc2hvdCI6IkVFWk5seWhqQk9yQVJMa2ktTVVoWHFLayIsImV4cCI6MTgxNTAzNjQ0NH0.UwegiwfiT3KSKQ17n5ivt24EFwAtoaHZGRwx97t0ODA&amp;sealed_github_token=A6pS93BW-53hz_Cs.8rPY0yM6koNS_UowTbu8qkWzmPLRco8HJRJO2_yp5MZH9yz9o1Ob2g5MUL4aLFK24gjbDexjdErw6eR72ruRbNcsFciYuM4DghLZJgi9V5D9f1-_xFT-sGYK6ibc1A4aWz8EyvFIQo0byPAXv-d410cJmhwsH-bqc963L8J-z0kU6oIi-2zCSszxllze23WmJsWnTJBFUBrQbllg5cLwuHm6q-BMkjoVxChLdOMVwEs" alt="貢獻者" />
</a>

---

## Star 歷史

<a href="https://www.star-history.com/?repos=nyakang%2Fnyaterm&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&theme=dark&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
   <img alt="Star 歷史圖表" src="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
 </picture>
</a>

---

<a name="license"></a>
# 授權條款

本專案採用 [Apache License 2.0](../../LICENSE) 授權。
