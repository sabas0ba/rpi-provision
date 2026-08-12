# CLAUDE.md

Repository specific instructions. These take precedence over the global user
instructions for work inside this repository.

## 開発環境

`https://github.com/sabas0ba/dotfiles` の nix / コンテナ環境を基盤とする。
作業開始時に当該リポジトリの `CLAUDE.md` と devshell 定義を参照し、
ツールチェーンをそちらに合わせること。

本リポジトリ固有の前提:

- Rust ツールチェーンは `rust-toolchain.toml` で 1.95.0 に固定している。
  サンドボックス等で当該バージョンが入手できない場合のみ
  `RUSTUP_TOOLCHAIN=stable` で暫定回避してよいが、CI の設定は変更しない。
- 生成されるシェルスクリプトの検証には `dash` が必要。Raspberry Pi OS の
  `/bin/sh` が dash であるため、`bash` のみでの検証は不十分。
- 一時ファイルは `.tmp/` に置く (gitignore 済み)。

## 依存関係

**このワークスペースは外部クレートを一切持たない。** 依存を追加する変更は
受け入れない。判断の根拠は `docs/adr/0001-zero-dependencies.md` を参照。
TOML パーサ、SHA-256、行 diff、引数パーサはいずれも自前実装である。

CI に `Cargo.lock` の検査があり、workspace member 以外のパッケージが現れると
失敗する。

## アーキテクチャの不変条件

`spec -> render -> apply` の三層構造を崩さないこと。

- `crates/spec` が外部に触れるのは `SecretProvider` 経由のみ。秘密情報の解決と
  `[[files]]` のアセット読み込みがこれに当たる (render は純粋関数なので、
  転送するファイルの中身は Spec の時点で載っている必要がある)。検証はここで
  完結し、以降の層は「仕様は妥当である」と仮定してよい。
- `crates/render` は純粋関数である。`Spec` と digest だけを入力に `Plan` を
  返す。ファイルシステムに触れてはならない。ゴールデンテストはこの性質に
  依存している。
- `crates/apply` が唯一の I/O 層。`execute` は全アクションを **書き込み前に**
  解決する。途中失敗で config.txt が半端な状態にならないための不変条件で
  あり、崩してはならない。

## 冪等性

以下は回帰テストで担保している。変更時は必ず確認すること。

- `config.txt` の管理ブロックは追記ではなく置換。かつ常にファイル末尾。
  `[all]` 等の条件フィルタが sticky であるため、途中に挿入すると他人が
  書いた行のスコープを変えてしまう。
- `cmdline.txt` はトークン単位で編集する。正規表現による一括置換は不可。
- 2 回 apply して差分ゼロになること。

## 生成物の規約

- 生成するシェルは POSIX sh。先頭は `#!/bin/sh`、直後に `set -eu`。
  bash 拡張を使わない。
- 生成ファイルの改行は常に LF。Windows 上で実行しても同様。
- シェルへ埋め込む値は必ず `scripts::quote()` を通す。
- 秘密情報 (パスワードハッシュ、Wi-Fi PSK) は
  `payload/etc/NetworkManager/system-connections/*.nmconnection` と
  `secrets/password.hash` 以外に出現してはならない。テストで検査している。
- ステップスクリプトの番号は依存順を表す。挿入時は既存の番号を詰めずに
  空き番号を使う。現在 10/20/30/40/50/60/70/80 が埋まっている。
- `[[run]]` の `command` は仕様が唯一「値ではなくコード」を寄与する場所であり、
  意図的に `quote()` を通さない。それ以外は必ず通すこと。

## Raspberry Pi 5 固有の事項

誤りやすい箇所であり、`docs/raspberry-pi-5.md` に一次情報の出典と併せて
記載している。要点のみ:

- GPIO 14/15 の UART は `dtparam=uart0=on` で `/dev/ttyAMA0`。
  `enable_uart=1` は専用デバッグコネクタ (`/dev/ttyAMA10`) を指す。
  Pi 4 以前とは意味が異なる。
- USB gadget には `dr_mode=peripheral` の明示が必須。OTG_ID 線が無い。
- USB-C は給電口と兼用。gadget 使用時は GPIO ヘッダから給電する。

## コミット

Conventional Commits。機能追加は branch または worktree で実施し、
`--no-ff` で main へマージする。
