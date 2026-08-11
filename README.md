# ai-monitor

`ai-monitor` 是以 Rust 撰寫的本機 AI coding tool 使用量與帳號狀態 CLI，整合 Codex profile、Codex rate limits／reset credits，以及 OpenCode SQLite usage。

目前版本：`v0.3.1`  
Repository：<https://github.com/AdemKao/ai-monitor>

## 功能

- `overview` 同時顯示 Codex limits 與 OpenCode usage。
- `codex` 管理隔離 profile，透過官方 Codex CLI 登入、登出與執行命令，並讀取 account、rate limits、usage 與 reset credits。
- `codex usage` 會顯示多帳號 dashboard：帳號數量、各帳號用量、reset 時間、reset credit 與 expiry 狀態。
- terminal dashboard 會以進度條與顏色標示高用量、即將到期與已過期項目；可用 `--color always|never` 控制顏色。
- `codex credits` 與 `codex expiring` 預設只使用 Codex app-server 回傳的資料；private endpoint fallback 必須明確 opt-in。
- `opencode usage` 以唯讀方式讀取本機 OpenCode database，依日期、provider 與 model 彙總 token、訊息數與成本。
- `opencode optimize` 只管理 `ai-monitor` 自己建立的 optional time index；只有 `create --yes`／`remove --yes` 會修改 OpenCode database。
- `update` 直接從 GitHub Release 檢查、驗證 checksum 並更新目前的 `ai-monitor` binary，不需要 local checkout。
- `completion <shell> --install` 可把 shell completion 安裝到使用者目錄，支援 `ai-monitor o<Tab>`、`ai-monitor op<Tab>` 等 command completion。
- 所有資料型命令支援 terminal 或 pretty JSON 輸出；另有 `doctor` 與 shell `completion`。
- 兼容以 `chatgpt-status` 與 `opencode-daily-usage` 作為 executable basename 的 legacy alias，但不承諾完整舊 CLI flag 相容。

## 安裝

### GitHub latest release installer

Unix-like 系統可從 GitHub Releases 的 latest stable release 安裝：

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/AdemKao/ai-monitor/releases/latest/download/ai-monitor-installer.sh | sh
```

Windows PowerShell 可使用 dist 產生的 installer：

```powershell
irm https://github.com/AdemKao/ai-monitor/releases/latest/download/ai-monitor-installer.ps1 | iex
```

installer 預設安裝到 Cargo binary 目錄，通常是 `~/.cargo/bin`。執行 `curl | sh` 或 `irm | iex` 前，請先檢查 release 頁面的 SHA-256 checksum；不信任的環境應先下載、檢閱 script，再執行它。

### 更新已安裝版本

一般使用者不需要用 local checkout 或 `cargo install` 更新。已安裝 v0.3.0 後，直接執行：

```sh
ai-monitor update
```

它會從 GitHub latest Release 下載目前平台 archive，驗證 SHA-256 後原子替換 binary，不會搬移或刪除 Codex profiles、OpenCode database 或 auth。非互動環境使用 `--yes`；只檢查版本使用 `--check`：

```sh
ai-monitor update --check
ai-monitor update --yes
```

v0.3.0 是 direct update 的第一個版本；v0.2.0 使用者先用一次 GitHub installer bootstrap，之後即可使用 `ai-monitor update`。若 installer 是 fallback，仍可使用：

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/AdemKao/ai-monitor/releases/latest/download/ai-monitor-installer.sh | sh
```

更新後若目前 shell 找不到新 binary，執行 `rehash` 或重新開啟 terminal。`cargo install --path .` 僅供專案開發與測試，不是一般使用者的更新方式。

### Cargo

從指定 Git tag 安裝（開發者用途）：

```sh
cargo install --git https://github.com/AdemKao/ai-monitor --tag v0.3.1 ai-monitor
```

在本機 checkout 安裝目前原始碼：

```sh
cargo install --path .
```

### Homebrew

Homebrew formula 尚未發布，以下是 planned 指令，不代表目前可直接執行：

```sh
brew install AdemKao/tap/ai-monitor
```

這需要未來建立並維護 `AdemKao/homebrew-tap`；目前 dist 設定不會執行 Homebrew publisher，也不要求 Homebrew token。

## Optional provider dependencies

核心 binary 使用 bundled SQLite，不需要系統安裝 SQLite server。Codex 與 OpenCode 是不同的本機資料來源，各自只有使用相關 command 時才需要。

### Codex

- 需要官方 `codex` CLI；預設從 `PATH` 執行。
- 可用 `AI_MONITOR_CODEX_BIN` 指定 binary；`CHATGPT_STATUS_CODEX_BIN` 是 legacy alias。
- `login`、`logout`、`run` 直接委派給官方 Codex CLI。
- `usage`、`credits`、`expiring` 與 `all` 會啟動 `codex app-server --listen stdio://`，因此需要可用且已登入的 profile。
- `overview` 會讀取所有可解析的 Codex profiles；單一 profile 失敗會以結果中的 error 呈現。

### OpenCode

`opencode usage` 需要可讀取的 OpenCode SQLite database。database path 依以下順序解析：

1. `--db <PATH>`。
2. `AI_MONITOR_OPENCODE_DB`。
3. `~/.local/share/opencode/opencode.db`，只在檔案存在時使用。
4. 執行 `opencode db path` 取得 path。

若使用標準 path 以外的 database，優先使用 `--db` 或 `AI_MONITOR_OPENCODE_DB`。`doctor` 也會檢查 `opencode --version`，並嘗試解析 database path。

## 快速開始

### 1. 確認安裝與依賴

```sh
ai-monitor --version
ai-monitor doctor
```

`doctor` 會顯示 Codex／OpenCode binary、profile home 與 OpenCode database path。它不會替你登入，也不會建立或修改 OpenCode index。

### 2. 查看整合概覽

準備好 OpenCode database 並至少建立一個 Codex profile 後：

```sh
ai-monitor overview
```

常用範例：

```sh
ai-monitor overview --days 14 --all-projects
ai-monitor overview --days 7 --project "$PWD" --db "$HOME/.local/share/opencode/opencode.db"
```

`overview` 的 `--db` 只指定 OpenCode database；Codex 會從 profile store 讀取所有 profiles。

### 3. 建立與查看 Codex profiles

```sh
ai-monitor codex profiles
ai-monitor codex login personal
ai-monitor codex login work
ai-monitor codex default work
ai-monitor codex usage
ai-monitor codex usage --profile work
```

`codex login <NAME>` 會建立隔離 profile，設定該 profile 的 `CODEX_HOME`，再執行官方 `codex login`。若 profile 已有 auth，必須明確使用 `--force` 才會再次登入：

`codex usage` 不指定 profile 時會掃描所有 profiles；指定 `--profile` 時仍會顯示所有帳號摘要，並將指定 profile 標記為 selected。這樣可以一次確認帳號數量與每個帳號狀態。

```sh
ai-monitor codex login work --force
```

### 4. 查看 reset credits

```sh
ai-monitor codex credits --profile work
ai-monitor codex expiring --profile work --days 7
ai-monitor codex usage --allow-private-api
ai-monitor --color always codex usage
```

預設不呼叫 private credits endpoint。只有明確加入 `--allow-private-api` 時，`credits`／`expiring` 才會在 app-server 沒有詳細 credit list 時嘗試 fallback：

```sh
ai-monitor codex credits --profile work --allow-private-api
ai-monitor codex expiring --profile work --days 7 --allow-private-api
```

這個 fallback 會讀取該 profile 的 `auth.json`，以 bearer token 直接請求 `https://chatgpt.com/backend-api/wham/rate-limit-reset-credits`。每個 eligible profile 每次 command 最多請求一次；指定 `--profile` 時只會對 selected profile 執行 private fallback。這是 ChatGPT/Codex 的 private、非官方公開 API，不保證穩定或允許第三方使用；它可能傳送帳號 token 到該 endpoint。除非你理解風險，請不要使用 `--allow-private-api`。

若 endpoint 回傳 HTTP 429，代表服務端 rate limit，不代表本機 auth 解析失敗。ai-monitor 不會自動密集 retry，請等待後再試。

### 5. 查看 OpenCode usage

```sh
ai-monitor opencode usage
ai-monitor opencode usage --days 30 --project "$PWD" --top-models 20
ai-monitor opencode usage --all-projects --include-cache
```

預設查詢最近 7 天、目前 project、前 10 個 model，token 顯示 input／output／reasoning 的 active token。`--include-cache` 會把 cache read／write token 納入計算；`--top-models 0` 代表不限制 model ranking 數量。

OpenCode usage 以 SQLite read-only mode 開啟 database，不會建立 index、不會更新 message，也不會執行 optimize。`--all-projects` 未安裝 index 時會提出可能掃描整個 database 的警告。

### 6. 管理 OpenCode optional index

```sh
ai-monitor opencode optimize status
ai-monitor opencode optimize create --yes
ai-monitor opencode optimize remove --yes
```

若 database 不在預設位置，`--db` 是 `optimize` 層級的 option，放在 action 前：

```sh
ai-monitor opencode optimize --db "$HOME/.local/share/opencode/opencode.db" status
```

`status` 是唯讀檢查。`create --yes` 與 `remove --yes` 會直接修改第三方 OpenCode database；沒有 `--yes` 時 command 會失敗並要求重新執行確認。執行前請自行備份。建立的 index 名稱是 `ai_monitor_message_time_created_idx`，只索引 `message.time_created`；`remove` 只會移除這個 `ai-monitor` index，不會刪除 OpenCode message 或 session。

### 7. 使用 JSON、completion 與其他 command

`--format` 是 global option，預設是 `terminal`，可放在 command 前：

```sh
ai-monitor --format json overview
ai-monitor --format json codex profiles
ai-monitor --format json codex credits --profile work
ai-monitor --format json opencode usage --all-projects
ai-monitor --format json opencode optimize status
ai-monitor --format json doctor
```

需要讓其他工具取得資料時使用 JSON。`login`、`logout`、`run` 與 `default` 主要委派或執行命令，不會把所有 child-process output 強制包成 JSON。

可產生 Bash、Elvish、Fish、PowerShell 或 Zsh completion：

```sh
ai-monitor completion zsh > "${fpath[1]}/_ai-monitor"
ai-monitor completion bash > ~/.local/share/bash-completion/completions/ai-monitor
```

最簡單的 zsh 一次性設定：

```sh
ai-monitor completion zsh --install
rehash
```

它會安裝 `~/.zsh/completions/_ai-monitor`，必要時備份並更新 `.zshrc` 的 `fpath`。一般 binary installer 不會暗自修改 shell 設定；完成這次設定後，之後的 `ai-monitor update` 不需要重新設定 completion。

其餘 Codex 操作：

```sh
ai-monitor codex all
ai-monitor codex run --profile work -- --help
ai-monitor codex logout --profile work
```

## Command reference

### Global options

| Syntax | 預設／說明 |
| --- | --- |
| `--format <terminal\|json>` | 預設 `terminal`；資料型 command 使用 pretty JSON 輸出 |
| `--color <auto\|always\|never>` | 預設 `auto`；高用量與 expiry warning 使用 terminal color |

### Top-level commands

| Command | Options | 行為 |
| --- | --- | --- |
| `overview` | `-d, --days <DAYS>` 預設 `7`；`--all-projects`；`--project <PATH>`；`--db <PATH>` | 合併 Codex limits 與 OpenCode usage |
| `codex` | 見下表 | Codex profile、帳號與 limits |
| `opencode` | 見下表 | OpenCode usage 與 optional index |
| `doctor` | 無 | 檢查 binary 與本機 storage path |
| `update` | `--check`；`--yes`；`--force` | 從 GitHub latest Release 驗證並更新目前 binary |
| `completion <SHELL>` | `bash`、`elvish`、`fish`、`powershell`、`zsh`；`--install` | 輸出或安裝 completion script |

### Codex commands

| Command | Options | 行為 |
| --- | --- | --- |
| `codex profiles` | 無 | 列出 profiles、default 標記與 auth 狀態 |
| `codex default <NAME>` | 無 | 設定 default profile |
| `codex login <NAME>` | `--force` | 建立／重做隔離 profile 的官方 Codex login |
| `codex usage` | `--profile <PROFILE>`；`--allow-private-api` | 顯示所有帳號摘要與視覺化 rate limits；指定 profile 時標記 selected |
| `codex credits` | `--profile <PROFILE>`；`--allow-private-api` | 顯示 reset credits；private fallback 需 opt-in |
| `codex expiring` | `--profile <PROFILE>`；`--days <DAYS>` 預設 `7`；`--allow-private-api` | 尋找指定天數內到期的 active／available credits；省略 profile 時掃描所有 profiles |
| `codex all` | `--allow-private-api` | 顯示所有 profiles 的視覺化 dashboard |
| `codex run` | `--profile <PROFILE>`；其餘 args | 以 profile 的 `CODEX_HOME` 執行官方 Codex CLI |
| `codex logout` | `--profile <PROFILE>` | 透過官方 Codex CLI 清除 profile credentials |

`--profile` 省略時使用 `config.json` 的 default profile；若只有一個 profile，也會自動使用該 profile。沒有 default 且有多個 profiles 時 command 會失敗。

### OpenCode commands

| Command | Options | 行為 |
| --- | --- | --- |
| `opencode usage` | `-d, --days <DAYS>` 預設 `7`；`--all-projects`；`--project <PATH>`；`--db <PATH>`；`--include-cache`；`--top-models <N>` 預設 `10` | Read-only usage report；省略 project 時使用目前工作目錄 |
| `opencode optimize status` | `--db <PATH>` 位於 `optimize` 前 | Read-only 檢查 `ai-monitor` index 是否存在 |
| `opencode optimize create` | `--db <PATH>` 位於 `optimize` 前；`--yes` | 建立 `ai_monitor_message_time_created_idx`，直接修改第三方 DB |
| `opencode optimize remove` | `--db <PATH>` 位於 `optimize` 前；`--yes` | 移除 `ai-monitor` 自己的 index，直接修改第三方 DB |

OpenCode database 必須具備 `session(id, project_id, directory)` 與 `message(session_id, time_created, data)` 欄位。usage 只計算 `role == "assistant"` 的 message；缺少 provider／model 時會使用 `unknown`，無法解析的 timestamp／JSON row 會略過。

## Codex profiles 與 legacy 相容策略

預設 profile root 是 `~/.chatgpt-status`，因此會直接相容既有的 legacy directory profiles，而不是另建一個平行 auth store。root 解析順序是：

1. `AI_MONITOR_HOME`。
2. `CHATGPT_STATUS_HOME`（legacy environment alias）。
3. `~/.chatgpt-status`。

profile 目錄位於 `<root>/profiles/<name>`。profile name 只允許 ASCII letters、numbers、`-`、`_`、`.`；`auth.json` 存在時會顯示為 authenticated。每個 profile 都會以自己的目錄作為 `CODEX_HOME`。

`ai-monitor` 不搬移、複製或重新保存既有 `auth.json`，也不把 auth token 寫入 ai-monitor config 或輸出。`codex login` 只在指定 root 下建立 profile 並委派官方 Codex CLI；`codex run`、`usage`、`logout` 也使用同一個 profile directory。執行隔離的 Codex command 時，`OPENAI_API_KEY` 會從 child process environment 移除。

為了保持 compatibility，以下 executable basename 會自動插入 command prefix：

- 將 `ai-monitor` symlink 或改名為 `chatgpt-status` 執行時，會視為 `ai-monitor codex ...`。
- 將 `ai-monitor` symlink 或改名為 `opencode-daily-usage` 執行時，會視為 `ai-monitor opencode usage ...`。

這只是 command prefix alias，不是完整舊程式的 flag parser。舊版專用 flags、輸出格式與互動行為不保證相容；新功能請使用本 README 列出的 command syntax。

## 資料隱私與安全

- OpenCode `usage` 與 `optimize status` 只讀本機資料；`optimize create/remove` 是明確的第三方 database 寫入操作，且必須 `--yes`。
- Codex `login`、`run`、`logout` 與 `app-server` 是委派給官方 Codex CLI 的操作；其網路與帳號行為以 Codex CLI 為準。
- `codex credits`／`expiring` 的 private fallback 只有在使用者加入 `--allow-private-api` 時才會執行，且 endpoint 不是官方公開 API。
- ai-monitor 沒有 telemetry；但 command 可能輸出 account email、model、成本、檔案路徑與 rate-limit 資訊，請保護 terminal history、JSON 檔案與 log。
- OpenCode database 與 Codex `auth.json` 都可能包含敏感資料。請只授予必要權限，並在 `optimize create/remove` 前自行備份。

## 開發檢查

需求：Rust `1.85` 或更新版本。提交前執行：

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
```

CI 會在 Ubuntu、macOS 與 Windows 執行 clippy、test 與 release build，並在 Ubuntu 執行 rustfmt check。release 維護者也可以安裝 dist 後執行 `dist plan` 檢查 release matrix。

建立 release 時，先確認版本、`CHANGELOG.md` 與檢查都已更新，再推送 tag：

```sh
git tag v0.1.0
git push origin v0.1.0
```

tag 會觸發 `.github/workflows/release.yml`，並在 GitHub Releases 發布 dist 產物。發行流程只使用內建的 `GITHUB_TOKEN`，不會假設 Homebrew、npm 或其他不存在的 secret。

## 貢獻與安全回報

- 貢獻流程請見 [CONTRIBUTING.md](CONTRIBUTING.md)。
- 安全問題請依 [SECURITY.md](SECURITY.md) 的私下回報流程處理。
- 版本變更請見 [CHANGELOG.md](CHANGELOG.md)。

## License

本專案以 [MIT](LICENSE-MIT) **或** [Apache-2.0](LICENSE-APACHE) 授權，使用者可選擇其一。
