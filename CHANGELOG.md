# Changelog

`ai-monitor` 的重要變更記錄於此。

## [0.4.2] - 2026-08-12

- Codex profile 已有登入資料時，`codex login` 顯示明確原因與 `--force` 操作提示。

## [0.4.1] - 2026-08-11

- 新增 `PRIMARY -> SUBAGENT RELATIONSHIPS`，顯示 spawns、calls、tokens 與 share。
- 使用 `session.parent_id` 建立關係，避免高成本的 `message.parentID` join。

## [0.4.0] - 2026-08-11

- 新增 `opencode dashboard`，依 project、agent／subagent 顯示 calls、sessions、tokens 與 share。
- dashboard 使用 streaming read-only query，避免讀取 OpenCode 的大型 `part` content；all-project 查詢沿用 optional time index 警告。

## [0.3.5] - 2026-08-11

- Terminal rate-limit dashboard 只顯示剩餘容量；`used_percent` 仍保留於 JSON 與內部資料。

## [0.3.4] - 2026-08-11

- Codex rate-limit dashboard 明確標示 `used` 與 `remaining` 百分比，避免進度條語意不清。

## [0.3.3] - 2026-08-11

- 將版本短參數從 `-V` 改為 `-v`，同時保留 `--version`。

## [0.3.2] - 2026-08-11

- Codex reset-credit private fallback 改為預設啟用，讓 dashboard 預設顯示可用的 reset credits。
- 新增 `--no-private-api` 停用 private fallback；`--allow-private-api` 保留為舊版相容參數。

## [0.3.1] - 2026-08-11

- `--allow-private-api` 指定 profile 時只對 selected profile 執行 private fallback，避免 dashboard 摘要造成不必要的多帳號請求。
- private credit lookup 遇到 HTTP 429 時保留 retry-after 資訊，且不會自動密集重試。
- `completion <shell> --install` 可安裝 completion script；zsh 會在需要時備份並更新 `.zshrc` 的 completion path。

## [0.3.0] - 2026-08-11

- 新增 `update`、`update --check`、`update --yes` 與 `update --force`，直接從 GitHub Release 驗證 checksum 後更新 binary。
- Codex dashboard 改用固定可見寬度與 ANSI-aware padding，修正不同顏色與長度造成的欄位、card 邊框不對齊。
- private credit lookup 遇到 HTTP 429 時顯示 rate-limit 原因，不再籠統顯示 lookup failed。

## [0.2.0] - 2026-08-11

- `codex usage` 改為視覺化多帳號 dashboard，顯示帳號數量、各帳號 usage、reset time、reset credit 與 credit expiry。
- 以 progress bar 與 terminal colors 標示高用量、即將到期與已過期狀態；新增 `--color auto|always|never`。
- `codex usage` 與 `codex all` 支援 `--allow-private-api` 取得 reset-credit 詳細 expiry rows。
- 文件補充 release installer 的更新流程；一般使用者不需要從 local checkout 更新。

## [0.1.0] - 2026-08-11

初始公開版本：

- 提供 `overview`、`codex`、`opencode`、`doctor` 與 `completion` CLI commands。
- 提供 Codex isolated profiles、official CLI login／logout／run、account limits、usage、reset credits 與 expiring credits 查詢。
- Codex private credits fallback 僅能透過 `--allow-private-api` 明確啟用。
- 提供 OpenCode read-only usage report，以及需要 `--yes` 的 index status／create／remove 操作。
- 提供 terminal／JSON 輸出、跨平台 CI 與使用 dist（formerly cargo-dist）的 GitHub Release automation。
- 預設直接使用 `~/.chatgpt-status` directory profiles；提供有限的 `chatgpt-status` 與 `opencode-daily-usage` executable alias，不宣稱完整舊 flag 相容。

[0.1.0]: https://github.com/AdemKao/ai-monitor/releases/tag/v0.1.0
[0.2.0]: https://github.com/AdemKao/ai-monitor/releases/tag/v0.2.0
[0.3.0]: https://github.com/AdemKao/ai-monitor/releases/tag/v0.3.0
[0.3.1]: https://github.com/AdemKao/ai-monitor/releases/tag/v0.3.1
[0.3.2]: https://github.com/AdemKao/ai-monitor/releases/tag/v0.3.2
[0.3.3]: https://github.com/AdemKao/ai-monitor/releases/tag/v0.3.3
[0.3.4]: https://github.com/AdemKao/ai-monitor/releases/tag/v0.3.4
[0.3.5]: https://github.com/AdemKao/ai-monitor/releases/tag/v0.3.5
[0.4.0]: https://github.com/AdemKao/ai-monitor/releases/tag/v0.4.0
[0.4.1]: https://github.com/AdemKao/ai-monitor/releases/tag/v0.4.1
