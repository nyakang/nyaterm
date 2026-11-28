<p align="center">
  <img src="../static/img/nyaterm.svg" alt="NyaTerm" width="128" height="128">
</p>

<h1 align="center">NyaTerm</h1>

<p align="center">
  <em>/ˈnjaː tɜːrm/</em>
</p>

<p align="center">
  <strong>GPUI で構築された、モダンでネイティブなリモートターミナルワークスペース。</strong><br/>
  <a href="https://nyaterm.app"><strong>nyaterm.app</strong></a> ·
  <a href="https://nyaterm.app/en/docs/"><strong>ドキュメント</strong></a>
</p>

<p align="center">
  SSH、ローカルシェル、Telnet、シリアル、RDP、VNC、SFTP、トンネル、OTP、AI 支援、暗号化同期を 1 つのデスクトップクライアントに。
</p>

<p align="center">
  <a href="https://discord.gg/Tgb9ugMKdM" target="_blank"><img src="https://dcbadge.limes.pink/api/server/Tgb9ugMKdM" alt="Discord" /></a>
  &nbsp;
  <a href="https://docs.qq.com/doc/DYW5TUkZnakxOWUFW" target="_blank"><img src="https://img.shields.io/badge/WeChatGroup-Join-07C160?style=for-the-badge&logo=wechat&logoColor=white" alt="WeChat グループ" /></a>
  <br/>
  <a href="https://linux.do" target="_blank" rel="noopener noreferrer">
    <img src="https://img.shields.io/badge/LINUX%20DO-Forum-F0B003?style=for-the-badge&logo=discourse&logoColor=white" alt="LINUX DO" />
  </a>
  <br/>
  <a href="https://nyaterm.app"><img alt="バージョン" src="https://img.shields.io/github/v/release/nyakang/nyaterm?style=flat-square&logo=github&color=0EA5E9&labelColor=334155"></a>
  &nbsp;
  <a href="https://github.com/nyakang/nyaterm/releases"><img alt="GitHub ダウンロード数" src="https://img.shields.io/github/downloads/nyakang/nyaterm/total?style=flat-square&logo=github&color=0EA5E9&label=Downloads&labelColor=334155"></a>
  &nbsp;
  <a href="#"><img alt="プラットフォーム" src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-0EA5E9?style=flat-square&logo=linux&labelColor=334155"></a>
  &nbsp;
  <a href="../../LICENSE"><img alt="ライセンス" src="https://img.shields.io/badge/License-Apache%202.0-0EA5E9?style=flat-square&logo=readthedocs&labelColor=334155"></a>
</p>

<p align="center">
  <a href="../../README.md">English</a> · <a href="./README.zh-CN.md">简体中文</a> · <a href="./README.zh-TW.md">繁體中文</a> · <strong>日本語</strong> · <a href="./README.ko.md">한국어</a> · <a href="./README.fr.md">Français</a> · <a href="./README.pt-BR.md">Português (Brasil)</a>
</p>

---

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/product-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/product-light.png">
    <img alt="NyaTerm のメインワークスペース" src="../static/img/home/product-light.png">
  </picture>
</p>

---

<a name="ai-assistant"></a>
# AI アシスタント

NyaTerm は、コマンド生成、ターミナル出力の解説、エラー分析、複数の手順にわたるターミナル操作に対応する AI アシスタントパネルを備えています。

## 主な機能

- **Ask モード**：コマンド生成、選択した出力の解説、エラー分析など、単発の相談に対応
- **Agent モード**：アクティブなターミナルセッションを対象に「観察・判断・実行」を繰り返し、複数の手順からなる作業を実施
- **直近の出力に対する操作**：手動でコピーせずに、最新のターミナル出力の解説を AI に依頼
- **構造化されたコマンドカード**：リスクレベル、実行操作、クイックコマンドへの保存機能を提供
- **承認済みコマンドの実行ツール**：Agent のワークフローでコマンドを実行し、ターミナル作業の完了後に別のステップで最終回答を提示
- **ターミナル出力のインライン取得**：Agent の実行中に出力を取得し、`Terminal Output Lines` で行数を設定
- **セッションへのメンション**：`@` を使って他のターミナルセッションを AI のコンテキストに追加
- **プロバイダー管理**：組み込みプロバイダー、手動でのモデル登録、認証情報グループ、カスタムの OpenAI 互換エンドポイントに対応
- **リスク管理**：影響の大きいコマンドに承認を求め、より安全な代替案を提示

---

<a name="what-is-nyaterm"></a>
# NyaTerm とは

**NyaTerm** は、SSH を中心とした運用と多様なターミナル作業に対応する、GPUI 製のネイティブデスクトップクライアントです。Rust ワークスペースに、デスクトップ UI、ターミナル描画、通信プロトコル、永続化、認証、ネットワークツール、AI によるターミナル操作支援、セッションのインポート／エクスポート、診断、暗号化同期／バックアップを統合しています。

- **NyaTerm は**開発者、システム管理者、DevOps エンジニア向けの SSH クライアントです
- **NyaTerm は**タブ、水平分割、垂直分割を備えたターミナルワークスペースです
- **NyaTerm は**転送キューと「ローカルで編集してアップロード」するワークフローを備えた SFTP ブラウザーです
- **NyaTerm は** SSH、ローカルターミナル、Telnet、シリアル、RDP、VNC のセッションに対応しています

---

<a name="why-nyaterm"></a>
# NyaTerm を選ぶ理由

NyaTerm は、日々サーバー、ローカルコマンド、デバイス、設定ファイルを行き来する方のために作られています。

- **ワークスペースを中心に整理**：タブ、分割ペイン、サイドパネル、子ウィンドウで関連するターミナルをまとめて管理
- **セッション内で遠隔操作**：セッションを離れずに SFTP ファイルの参照、ターミナルパスへの追従、転送、リモートファイルの編集を実施
- **セキュリティに配慮した操作**：認証情報、鍵、既知のホスト、OTP、画面ロック、マスターパスワードで保護されたストレージを管理
- **持ち運べる設定**：既存ツールからのインポート、暗号化 `.nya` バックアップのエクスポート、WebDAV や S3 互換ストレージを使った暗号化スナップショットの同期に対応
- **必要な場面で使える AI**：アクティブなターミナルのコンテキストから、コマンド生成、出力の確認、承認済みの複数ステップの操作を実施

---

<a name="features"></a>
# 機能

## セッションとワークスペース

- SSH、ローカルターミナル、Telnet、シリアル、RDP、VNC セッションに対応
- 水平・垂直のペイン分割、タブのドラッグによるドッキング、レイアウト復元を備えたマルチタブワークスペース
- RDP と VNC のリモートデスクトップペイン。VNC は現在、直接 TCP 接続、None／従来の VNC Auth、Raw／ZRLE／Tight／Tight JPEG のフレームバッファ更新、ウィンドウの拡大縮小、回数制限付きの再接続、Latin-1 テキストのクリップボード共有に対応
- 保存済み接続のフォルダー、アイコン、メタデータ、複製、キーボードによるコピー、再接続、インポートに対応
- コマンドパレットとセッションのクイック切り替えで、操作、開いているセッション、保存済み接続、新規セッションの作成入口を検索
- メインウィンドウの `Background Image` をカスタマイズ。`cover`／`contain`／`stretch`／`tile` の表示方法と `Background Content Opacity` を設定可能
- 左右のアクティビティバーから、ファイルエクスプローラー、ネットワーク、セキュリティ／認証、同期とバックアップ、AI アシスタント、ノート、保存済み接続、アクティブなセッション、コマンド履歴、資産監視、リソース監視、GPU 監視、プロセスマネージャー、Docker マネージャーにアクセス
- ノートパネルとエディターに、ツリーナビゲーション、コンテキストメニュー、自動保存、ツールバー操作、同期／バックアップに対応した永続化を搭載
- 資産監視ワークスペースに、接続のグループ表示、パンくずナビゲーション、テーブル／カード表示、リソース／GPU／NPU の状態表示を統合
- SSH セッション向けのリモートホスト監視パネル：リソース監視、NVIDIA GPU と Ascend NPU の概要、プロセスマネージャー（signal／renice）、Docker マネージャー（コンテナー、イメージ、ボリューム、ネットワーク、Compose）
- セッション入力の同期グループで、入力内容や送信コマンドを複数のセッションに同時配信
- `ssh://` URL や `ssh` コマンドを貼り付けて、接続を保存せずに一時的な SSH 接続を作成
- 設定、新規セッション作成、クイックコマンド編集、リモートファイル編集、自動アップロードの確認を子ウィンドウで表示
- システムトレイに対応し、トレイへの最小化やメインウィンドウの非表示を設定可能

## ターミナルの使い勝手

- ターミナル検索に、結果間の移動、検索履歴、コピー／貼り付け、コンテキストメニュー、選択テキストに対する操作を搭載
- コマンド履歴に、あいまい検索による候補、設定可能な長さフィルター、対話型プログラムでの候補抑制を搭載
- emoji、結合文字、ZWJ シーケンスに対応した Unicode 書記素の描画
- 行番号とタイムスタンプのガターを必要に応じて表示
- IPv4 アドレス、`host:port`、アーカイブファイル名のアクションリンクを有効化可能
- 拡充された組み込みプリセット、カスタムルール、JSON インポートに対応したキーワード強調表示を有効化可能
- ターミナルの拡大縮小、ワークスペースの余白、フォントの太さ、ネイティブ IME、画像パスの貼り付け処理、ターミナルのクリア操作に対応
- 直近の出力を解説する AI ショートカットと、`Terminal Output Lines` で行数を設定できる Agent コマンド出力のインライン表示
- 大量出力からの保護、設定可能なスクロールバック、SSH キープアライブ、セッション録画
- 選択したターミナルテキストのオンライン検索と翻訳
- ターミナルから直接 Zmodem でファイルを転送し、転送キューに表示
- すべてのセッションを閉じる前に確認ダイアログを表示
- ターミナルと UI 操作のキーボードショートカットをカスタマイズ可能。Telnet とシリアルセッションでは `Backspace Mode` も選択可能

## SFTP とファイル操作

- SSH セッションに SFTP ファイルエクスプローラーを内蔵
- アップロード、ダウンロード、名前変更、移動、削除、プロパティ、新規ファイル／フォルダー、OpenSSH 互換のシンボリックリンク操作に対応
- フォルダーのアップロード／ダウンロード、複数フォルダーのアップロード選択、複数選択、編集可能なパスバー、ターミナルの作業ディレクトリとの手動／自動同期
- 転送キューに速度表示、一時停止、再開、キャンセル、再試行、転送先の重複処理、タイムスタンプ保持、同時転送数の設定を搭載
- SCP とディレクトリ転送の処理を強化し、可能な場合は元の属性を保持
- リモートファイルをローカルエディターで開き、ファイル監視による自動アップロードで保存した変更を反映。内容のフィンガープリントにより、実際に内容が変わった場合のみ再アップロード
- テキスト、JSON、Markdown、CSV／TSV、画像、PDF の読み取り専用リモートプレビューを複数タブで表示。タブごとのセッションラベル、拡大縮小／回転／フィット、PDF のページ単位の描画に対応
- プレビューの読み込みとデコードは描画スレッドの外で実行し、形式ごとにサイズを制限。大きすぎるファイルや未対応の形式は、UI を止めずに明確なメッセージを表示
- SFTP チャネルの同時実行数を制限し、一時的なチャネル作成の失敗時には自動で再試行
- Windows では外部からのドラッグ＆ドロップによるアップロードに対応
- Zmodem 転送でローカルパスを追跡し、完了したファイルをシステムのファイルマネージャーで表示可能

## セキュリティ、認証、ネットワーク

- パスワード認証、秘密鍵、ホスト鍵の検証、暗号化されたローカル永続化
- 正規表現に基づくターミナル自動入力に対応した認証情報管理
- TOTP／HOTP、QR コードからのインポート、SSH 自動入力に対応した OTP 管理
- 接続ごとの SSH アルゴリズム設定に、互換／安全／カスタムモードを用意。鍵交換、暗号、MAC、ホスト鍵のセキュリティリスクを表示
- SOCKS5、HTTP、ProxyCommand のプロキシ設定、循環防止付き SSH 踏み台ホスト、ローカル／リモート／動的トンネルに対応
- ローカル X サーバーがある環境で SSH X11 転送に対応
- 画面ロック、アイドル時のアプリロック、マスターパスワード、診断設定、ローカルログ管理、診断バンドルのエクスポート

## 同期、バックアップ、移行

- WebDAV、S3 互換ストレージ、GitHub Gist による暗号化クラウド同期とバックアップ
- 同期、バックアップ、暗号化インポート／エクスポート、定期的な暗号化バックアップにはマスターパスワードが必要
- 起動時の同期確認、対応するローカル変更をまとめてからの自動プッシュ、詳細な状態更新、定期バックアップの保持管理
- 手動のテスト／プッシュ／プル／バックアップ、リモートバックアップの復元、スナップショット単位の競合解決
- Xshell、MobaXterm、WindTerm、NyaTerm の JSON 定義からセッションをインポート
- アプリ全体の設定を暗号化 `.nya` 形式でインポート／エクスポートし、NyaTerm の設定を持ち運び可能

---

<a name="screenshots"></a>
# スクリーンショット

## ワークスペース

タブと分割ペインを備えた 1 つのワークスペースで、SSH、ローカルシェル、Telnet、シリアルのセッションを管理できます。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/overview-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/overview-light.png">
    <img alt="NyaTerm のワークスペース概要" src="../static/img/home/overview-light.png">
  </picture>
</p>

## 外観と背景画像

ローカルの壁紙をメインウィンドウの背景に設定し、`Image Sizing`、`Image Opacity`、`Background Content Opacity` を調整できます。子ウィンドウは単色の背景で読みやすさを保ちます。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/cover-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/cover-light.png">
    <img alt="NyaTerm の背景画像設定" src="../static/img/home/cover-light.png">
  </picture>
</p>

## ターミナルの拡張機能

ターミナルでコマンド履歴、検索、翻訳、アクションリンク、タイムスタンプ、キーワード強調表示、大量出力からの保護を利用できます。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/terminal-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/terminal-light.png">
    <img alt="NyaTerm のターミナル機能" src="../static/img/home/terminal-light.png">
  </picture>
</p>

## リモートファイル

ターミナルの隣で SFTP ファイルを参照し、転送を管理し、ローカルエディターでの変更をリモートのパスへアップロードできます。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/files-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/files-light.png">
    <img alt="NyaTerm の SFTP ファイル操作" src="../static/img/home/files-light.png">
  </picture>
</p>

## セキュリティとネットワークツール

専用パネルから認証情報、OTP、既知のホスト、プロキシ、踏み台ホスト、SSH トンネルを管理できます。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/security-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/security-light.png">
    <img alt="NyaTerm のセキュリティとネットワークツール" src="../static/img/home/security-light.png">
  </picture>
</p>

## 同期とバックアップ

WebDAV や S3 互換ストレージを使って、暗号化されたポータブル設定スナップショットの同期とバックアップの復元を行えます。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/sync-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/sync-light.png">
    <img alt="NyaTerm の同期とバックアップ" src="../static/img/home/sync-light.png">
  </picture>
</p>

---

<a name="supported-platforms"></a>
# 対応プラットフォーム

| OS | 対応範囲 |
| :--- | :--- |
| **Windows** | Windows 10/11、x64／arm64 |
| **macOS** | macOS 12+、Intel／Apple Silicon |
| **Linux** | Ubuntu 20.04+、Fedora 36+、Arch Linux、および同様のディストリビューション |

**glibc 2.31** を使用するカスタマイズされた Ubuntu 20.04 x86-64 デスクトップには、専用の
[focal オフライン互換ビルド](../../scripts/compat/linux-focal/README.md)を使用してください。
標準の Linux リリースパッケージは新しいディストリビューションでビルドされており、
より新しい glibc が必要になる場合があります。互換ビルドには、すべてのランタイムヘルパー、ABI レポート、
システム／GPU を優先するランチャー、明示的に選択する専用 CPU OpenGL フォールバックが含まれます。
対象マシンには正常に動作する X11 セッションが必要です。

インストーラーは [nyaterm.app](https://nyaterm.app) または [Releases](https://github.com/nyakang/nyaterm/releases) ページからダウンロードできます。

---

<a name="supported-session-types"></a>
# 対応セッション種別

| 種別 | 主な用途 | 備考 |
|------|-------------|-------|
| SSH | Linux／Unix のリモートサーバー | SFTP、OTP、リソース／GPU／プロセス／Docker の監視、プロキシ、踏み台ホスト、トンネル、接続ごとのアルゴリズム設定に対応 |
| ローカルターミナル | ローカルシェルでの作業 | ローカルのシェルパスと作業ディレクトリを使用 |
| Telnet | レガシーなネットワーク機器や実験環境 | SSH 専用機能を持たない軽量なターミナルセッション。`Backspace Mode` で `Ctrl+H (BS)` または `DEL (0x7F)` を選択可能 |
| シリアル | ルーター、開発ボード、組み込み機器 | ポート、ボーレート、データビット、パリティ、ストップビット、`Backspace Mode` を設定可能 |

---

<a name="getting-started"></a>
# はじめに

## ダウンロード

お使いのプラットフォーム向けの最新版を [nyaterm.app](https://nyaterm.app) または [Releases](https://github.com/nyakang/nyaterm/releases) からダウンロードしてください。

| プラットフォーム | 形式 |
|----------|--------|
| Windows | `.exe`／ポータブル `.zip` |
| macOS | `.dmg` |
| Linux | `.deb`／`.AppImage`／`.rpm` |

Windows ポータブル版は ZIP を展開し、`NyaTerm.exe` を実行してください。データは隣接する `data/` ディレクトリに保存されます。GPUI の**ヘルプ → 更新を確認**は署名付きマニフェストを使用し、対応するインストール形式で更新のダウンロードとインストールを行えます。Linux の deb／rpm 版は、Polkit または sudo の認可を受けてシステムのパッケージマネージャーから署名付き更新をインストールし、その後に再起動を案内します。依存パッケージは事前にインストールしておく必要があります。Homebrew や外部で管理されるビルドは、それぞれのパッケージマネージャーを使用します。

**1.x からの移行：Windows ポータブル版と旧 MSI 版は手動アップグレードが必要です。**まずアプリを終了し、すべてのデータと鍵をバックアップしてください。ポータブル版は `data/` を残してプログラム一式を置き換えます。MSI 版はバックアップ後に旧版をアンインストールし、NSIS 版をインストールしてください。メインの実行ファイルだけを置き換えても不十分です。すべての旧インストーラーについて自動移行を保証するものではありません。[2.0 移行ガイド](https://nyaterm.app/en/docs/getting-started/migration-v2)を参照してください。

### macOS

macOS では Homebrew で NyaTerm をインストールできます：

```bash
brew install nyakang/nyaterm/nyaterm
```

このコマンドは [`nyakang/homebrew-nyaterm`](https://github.com/nyakang/homebrew-nyaterm) tap を使い、`nyaterm` cask をインストールします。[nyaterm.app](https://nyaterm.app) または [Releases](https://github.com/nyakang/nyaterm/releases) から `.dmg` インストーラーをダウンロードし、NyaTerm を `/Applications` にドラッグすることもできます。

NyaTerm は現在、Apple Developer 証明書で署名されていません。インストール後に macOS がアプリの破損や起動不可を報告した場合は、隔離属性を削除してから再度開いてください：

```bash
sudo xattr -cr /Applications/NyaTerm.app
```

### Arch Linux / AUR

Arch Linux では AUR から NyaTerm をインストールできます：

```bash
yay -S nyaterm-bin
```

または `paru` を使用します：

```bash
paru -S nyaterm-bin
```

AUR リポジトリから手動でビルドすることもできます：

```bash
git clone https://aur.archlinux.org/nyaterm-bin.git
cd nyaterm-bin
makepkg -si
```

AUR パッケージ：[`nyaterm-bin`](https://aur.archlinux.org/packages/nyaterm-bin)

> `nyaterm-bin` はコミュニティが管理する AUR のバイナリパッケージです。AUR パッケージが最新版に追いついていない場合は、[Releases](https://github.com/nyakang/nyaterm/releases) から公式パッケージをダウンロードしてください。


## 開発に必要なもの

- [rustup](https://rustup.rs/) でインストールした Rust stable
- GPUI とお使いの OS が必要とするプラットフォーム依存パッケージ

## 開発

```bash
git clone https://github.com/nyakang/nyaterm.git
cd nyaterm
cargo run -p nyaterm-app --bin nyaterm
```

## プロジェクト構成

```text
├── crates/
│   ├── nyaterm-app/             # ネイティブ GPUI アプリのエントリーポイントとアセット
│   ├── nyaterm-desktop/         # GPUI アプリの構成と機能の状態
│   ├── nyaterm-core/            # UI に依存しないドメインモデルとポリシー
│   ├── nyaterm-terminal/        # ターミナルの状態機械と解析
│   ├── nyaterm-terminal-runtime/ # フレームワーカー、検索、セッションイベントのルーティング
│   ├── nyaterm-terminal-gpui/   # GPUI ターミナルのレイアウト、入力、描画
│   ├── nyaterm-transport/       # SSH、PTY、SFTP、Telnet、シリアル、転送
│   ├── nyaterm-store/           # 永続化とデータベース互換性
│   ├── nyaterm-ui/              # 共有 GPUI コントロールとテーマ統合
│   ├── nyaterm-remote-desktop/  # UI に依存しない RDP／VNC セッションと型付き IPC
│   ├── nyaterm-rdp-helper/      # 分離された IronRDP プロトコルのヘルパープロセス
│   ├── nyaterm-vnc-helper/      # 分離された VNC プロトコルのヘルパープロセス
│   └── nyaterm-otp/             # OTP 互換実装
├── docs-site/                   # Docusaurus ドキュメントサイト
├── Cargo.toml                   # Rust ワークスペース定義
└── Cargo.lock
```

---

<a name="credits"></a>
# 謝辞

NyaTerm を支える以下のプロジェクトとライブラリに感謝します：

- [WindTerm](https://github.com/kingToolbox/WindTerm) — NyaTerm の設計と機能に着想を提供
- [tabby](https://github.com/Eugeny/tabby) — 多くの設計上の着想を与えてくれた優れたクロスプラットフォームターミナル
- [Alacritty](https://github.com/alacritty/alacritty) — NyaTerm が使用するターミナルエミュレーションコンポーネント
- [russh](https://github.com/warp-tech/russh) — Rust による SSH クライアントとサーバーの実装

---

<a name="sponsor"></a>
# スポンサー

NyaTerm が日々の開発、運用、リモートターミナル作業に役立っている場合は、[スポンサーページ](./en/docusaurus-plugin-content-docs/current/sponsor.md)から継続的なメンテナンスを支援できます。

---

<a name="contributors"></a>
# コントリビューター

<a href="https://github.com/nyakang/nyaterm/graphs/contributors">
  <img src="https://contrib-wall.coderkang.workers.dev/image?snapshot=EEZNlyhjBOrARLki-MUhXqKk&amp;sealed_token=eyJ2IjoxLCJ0eXBlIjoic25hcHNob3QiLCJzbmFwc2hvdCI6IkVFWk5seWhqQk9yQVJMa2ktTVVoWHFLayIsImV4cCI6MTgxNTAzNjQ0NH0.UwegiwfiT3KSKQ17n5ivt24EFwAtoaHZGRwx97t0ODA&amp;sealed_github_token=A6pS93BW-53hz_Cs.8rPY0yM6koNS_UowTbu8qkWzmPLRco8HJRJO2_yp5MZH9yz9o1Ob2g5MUL4aLFK24gjbDexjdErw6eR72ruRbNcsFciYuM4DghLZJgi9V5D9f1-_xFT-sGYK6ibc1A4aWz8EyvFIQo0byPAXv-d410cJmhwsH-bqc963L8J-z0kU6oIi-2zCSszxllze23WmJsWnTJBFUBrQbllg5cLwuHm6q-BMkjoVxChLdOMVwEs" alt="コントリビューター" />
</a>

---

## Star 履歴

<a href="https://www.star-history.com/?repos=nyakang%2Fnyaterm&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&theme=dark&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
   <img alt="Star 履歴グラフ" src="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
 </picture>
</a>

---

<a name="license"></a>
# ライセンス

このプロジェクトは [Apache License 2.0](../../LICENSE) のもとで公開されています。
