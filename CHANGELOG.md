# Changelog

`ai-monitor` 的重要變更記錄於此。

## [0.1.0] - 2026-08-11

初始公開版本：

- 提供 `overview`、`codex`、`opencode`、`doctor` 與 `completion` CLI commands。
- 提供 Codex isolated profiles、official CLI login／logout／run、account limits、usage、reset credits 與 expiring credits 查詢。
- Codex private credits fallback 僅能透過 `--allow-private-api` 明確啟用。
- 提供 OpenCode read-only usage report，以及需要 `--yes` 的 index status／create／remove 操作。
- 提供 terminal／JSON 輸出、跨平台 CI 與使用 dist（formerly cargo-dist）的 GitHub Release automation。
- 預設直接使用 `~/.chatgpt-status` directory profiles；提供有限的 `chatgpt-status` 與 `opencode-daily-usage` executable alias，不宣稱完整舊 flag 相容。

[0.1.0]: https://github.com/AdemKao/ai-monitor/releases/tag/v0.1.0
