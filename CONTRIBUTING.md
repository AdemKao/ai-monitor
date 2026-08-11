# 貢獻 ai-monitor

感謝你協助改善 `ai-monitor`。

## 開始前

- 先閱讀 [README](README.md) 的實際 command 與 provider 行為說明。
- 安全問題請依 [SECURITY.md](SECURITY.md) 的私下流程回報，不要先建立公開 issue。
- 保持變更聚焦，不要提交 credentials、prompt、本機 database 或 generated build output。

## 開發環境

安裝 Rust `1.85` 或更新版本，然後執行 CI 使用的檢查：

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
```

CLI command 已實作，文件與測試必須以 `src/main.rs`、provider 實作及實際 help 為準。不要把未存在的 flags、完整舊 CLI 相容性或未承諾的 provider 行為寫進文件。

## Pull Request

- 說明使用者可見的行為與變更原因。
- 在適當時為已實作行為新增或更新測試。
- 公開行為或 release 資訊變更時，更新 `README.md` 與 `CHANGELOG.md`。
- 明確標示會讀取、寫入或 migrate provider data 的變更。
- 確認 Pull Request 的 CI 檢查通過。

Provider 相關變更必須文件化支援的 application version、data path、schema assumptions，以及操作是否唯讀。任何 OpenCode optimization 都必須在修改第三方 database 前取得明確 `--yes` 同意。

## Release 與更新

一般使用者透過 GitHub Release installer 更新，不要從 maintainer 的 local checkout 取得 binary：

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/AdemKao/ai-monitor/releases/latest/download/ai-monitor-installer.sh | sh
```

v0.3.0 起，一般使用者也可以執行 `ai-monitor update --check` 或 `ai-monitor update --yes`；該流程只能下載 GitHub Release artifact，並會驗證 SHA-256。修改 update pipeline 時，必須保留 checksum verification 與 atomic self-replace 行為。

建立 release 前必須更新 `Cargo.toml`、`Cargo.lock`、`CHANGELOG.md` 與 `README.md`，通過 CI 後建立 `vX.Y.Z` tag。GitHub Actions 會建置各平台 archive、installer 與 checksum；不要把本機 build output、credentials 或 database 上傳到 release。

## License

提交 contribution 即表示你同意該 contribution 依專案的 [MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE) 授權條款提供。
