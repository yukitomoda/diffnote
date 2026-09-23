# 開発者向けの説明

diffnote を直したり、ビルドしたり、リリースしたりする人のための説明です。使い方は [README](../README.md) にあります。

## 構成

- `src/`: CLI とサーバー(Rust)。レビューについての判断(スレッドの位置、一覧の順番、構文の色分け、Markdown の解釈、タイムラインなど)は、すべてここで行い、画面に JSON で渡します。
- `ui/`: 画面(Preact + TypeScript)。`serve` の画面と、`export` の HTML は、同じ画面のコードです。画面は、渡されたデータを並べるだけで、レビューについての判断はしません。詳しくは [ui/README.md](../ui/README.md)。
- `messages/ja.yaml`: ユーザーに見える文言のすべて(下の「文言」)。
- `tests/cli.rs`: 実際の実行ファイルを使った統合テスト。
- `tests/browser/`: 実際の Chrome を使った画面のテスト。詳しくは [tests/browser/README.md](../tests/browser/README.md)。
- `docs/`: この説明と、[レビューファイルの形式](bundle-format.md)。

## 必要なもの

- Rust 1.88 以上(`Cargo.toml` の `rust-version`)
- node 24 以上(画面のビルド)
- Python 3 と Chrome(ブラウザのテストだけ)

Rust 以外は `mise.toml` に書いてあり、`mise install` で入ります。

## ビルドとテスト

画面は node で、実行ファイルは cargo でビルドし、実行ファイルが画面を埋め込んで持ちます。どちらを直したときも、次のタスク(`mise.toml`)で足ります。

```sh
mise run build     # 画面 → 実行ファイル(変わっていないほうは飛ばします)
mise run test      # 上のあと、Rust・画面・ブラウザのすべてのテスト
mise run check     # cargo fmt の書式、clippy の警告、TypeScript の型
cargo install --path .   # ビルドしたものを入れる(先に mise run build)
```

`ui/dist`(ビルドした画面)はコミットしません。cargo だけを直接動かしたときは、`build.rs` が、`ui/dist` があるか、今の `ui/src` から作られたものかを確かめ、違えば `mise run build` を促して止まります(ビルドはしません)。

### テストの分担

できるだけ、ブラウザを使わないテストで確かめます。

- **Rust の単体テスト**(`cargo test`): レビューの記録、位置の追跡、サーバーの API など、ロジックの大部分。
- **統合テスト**(`tests/cli.rs`): 実行ファイルを起動して、`init` / `serve` / `export` がすることを確かめます。コメントは、起動した `serve` の HTTP API で書くので、Rust と git のほかには何も要らず、Linux と Windows のどちらでも動きます。別の場所で作った実行ファイルを試すときは、環境変数 `DIFFNOTE_BIN` にそのパスを指定します。
- **画面のロジック**(`npm --prefix ui test`): 画面側の計算(行の選択、横並びの対応、展開、行リンクの解釈、タイムラインのまとめ方など)は、コンポーネントの外のモジュールに置き、node のテストで確かめます。
- **ブラウザのテスト**(`tests/browser`): クリック、ドラッグ、描画など、ページがないと確かめられないことだけ。

### CI

`.github/workflows/ci.yml` が、プッシュとプルリクエストごとに、画面を 1 度だけビルドして、Linux と Windows(最新の Rust)、最低版の Rust 1.88(Linux)、ブラウザのテストに渡します。最新の Rust では、clippy の警告と rustfmt の書式もエラーとして扱います。

## 文言

CLI のヘルプやエラーから画面のボタンまで、ユーザーに見える文言は、すべて `messages/ja.yaml` に集めてあります(ソースコードの中には直接書きません)。キーはネストした YAML のパスで、モジュールごとにまとまっています(`cli.init.about`、`serve.not_found` など)。`{name}` はプレースホルダです。

Rust 側は `src/messages.rs` の `m(key)` / `mf(key, &[(name, value), ...])`、画面側は `ui/src/lib.ts` の `lib.m(key)` / `lib.mf(key, params)` で引きます(同じファイルを、サーバーがページに JSON として埋め込みます)。文言を直すときは、このファイルだけを見れば足ります。

## 画面について守ること

書き出した HTML は、1 つのファイルで完結し、`file://` で開けて、何も取りに行かないことが前提です。`ui/src/test/sources.test.js` などのテストが、次のことを確かめています。

- 外部を読み込む記述(`<link`、`src=`、`fetch(` など)を、書き出す画面に含めない
- 要素は組み立てて作り、HTML の文字列を書かない
- サーバーと話すのは `api.ts` だけ
- アイコンは絵文字ではなく、埋め込みの SVG の図形(Material Symbols。ライセンスは `ui/THIRD-PARTY.md`)

## リリースの出し方

1. `Cargo.toml` の `version` を上げてコミットする。
2. `git tag v0.2.0 && git push origin main v0.2.0`(タグは `v` + `Cargo.toml` のバージョン)。
3. `.github/workflows/release.yml` が、各プラットフォーム向けにビルドして、Release を作る(`v0.2.0-rc1` のように `-` が付くタグは、プレリリース)。タグとバージョンが食い違うと、ビルドの最初で失敗する。配布物には README が同梱される。
