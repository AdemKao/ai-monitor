# 安全政策

## 支援版本

在專案進入 1.0 前，最新的 `0.1.x` release 是唯一支援的版本線。

## 回報漏洞

請透過 [GitHub Security Advisories](https://github.com/AdemKao/ai-monitor/security/advisories/new) 私下回報疑似漏洞。尚未修補的漏洞請不要建立公開 issue。

請包含：

- 受影響版本、平台與安裝方式。
- 重現步驟或最小 proof of concept。
- 預期行為與實際觀察到的行為。
- 重現所需的 log 或檔案；請先移除 prompt、credentials 與其他私人資料。

我們會透過 GitHub 確認收到回報，並與回報者協調修補、公開時間與致謝方式。

## 本機資料安全

`ai-monitor` 會讀取本機 AI tool 資料。provider database 可能包含 prompt、檔案路徑、帳號與 model metadata。

- OpenCode `usage` 與 `optimize status` 是唯讀操作。
- `opencode optimize create --yes` 與 `remove --yes` 會直接修改第三方 OpenCode database，只建立或移除 `ai_monitor_message_time_created_idx`；執行前請自行備份。
- Codex reset-credit commands 預設可能讀取 `auth.json` 並將 bearer token 傳送到 private endpoint；使用 `--no-private-api` 可停用 fallback。該 endpoint 不是官方公開 API。
- `chatgpt-status` alias 只提供 command prefix compatibility，不保證舊版 flags、輸出或資料遷移行為。
