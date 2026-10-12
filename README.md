# local-markdown-mail

`lmm`（Local Markdown Mail）は、ローカルのファイルシステムだけを使って、AI エージェントと人の間で Markdown メッセージをやり取りする CLI です。常駐プロセスやデータベースは使わず、`~/.lmm` 以下のディレクトリを `ls` と `cat` だけで追えます。仕様は [docs/prd.md](docs/prd.md) を参照してください。

## インストール

### ビルド済みのバイナリ

[Releases](https://github.com/szk302/local-markdown-mail/releases) から、環境に合ったファイルをダウンロードして展開し、`lmm` を `PATH` の通った場所に置きます。

| ファイル名の末尾 | 環境 |
| :--- | :--- |
| `x86_64-unknown-linux-musl.tar.gz` | Linux（x86_64、静的リンク） |
| `aarch64-unknown-linux-musl.tar.gz` | Linux（arm64、静的リンク） |
| `aarch64-apple-darwin.tar.gz` | macOS（Apple Silicon） |
| `x86_64-apple-darwin.tar.gz` | macOS（Intel） |

```bash
v=v0.2.0; t=aarch64-apple-darwin
curl -LO "https://github.com/szk302/local-markdown-mail/releases/download/$v/lmm-$v-$t.tar.gz"
curl -LO "https://github.com/szk302/local-markdown-mail/releases/download/$v/SHA256SUMS"
grep "lmm-$v-$t.tar.gz" SHA256SUMS | shasum -a 256 -c -   # Linux では sha256sum -c -
tar xzf "lmm-$v-$t.tar.gz"
mkdir -p ~/.local/bin
install "lmm-$v-$t/lmm" ~/.local/bin/
```

`~/.local/bin` が `PATH` に入っていない場合は、シェルの設定（`~/.zshrc` など）に `export PATH="$HOME/.local/bin:$PATH"` を加えてください。アーカイブには、`LICENSE` と、同梱しているサードパーティーのライセンス表示（`THIRD-PARTY-LICENSES.txt`、Linux 版は `MUSL-COPYRIGHT` も）が入っています。

macOS で、ブラウザからダウンロードしたファイルが Gatekeeper にブロックされる場合は、`xattr -d com.apple.quarantine lmm` で解除できます。

### ソースから

Rust ツールチェーンは [mise](https://mise.jdx.dev/) で管理しています。

```bash
mise install                 # mise.toml の Rust をインストール
cargo install --locked --path .
```

Linux と macOS に対応します（`/bin/sh` が必要）。

## 使い方

```bash
# メールボックスを作る（エージェントの起動前に行う）
lmm box create manager --exist-ok
lmm box create reviewer --exist-ok
export LMM_NAME=reviewer

# 下書きを作って送る
d=$(lmm draft -s "Review result")
echo "LGTM" >> "$d/index.md"
read -r id path < <(lmm post -t manager "$d")

# 受け取る（古いものから 1 通。既定の展開先は ./.lmm-inbox/<LMM_NAME>/）
LMM_NAME=manager lmm receive

# 表示する・返信する・待つ
lmm show --id "$id" --json
LMM_NAME=manager lmm post --reply .lmm-inbox/manager/<msg> <DIR>
lmm wait --in-reply-to "$id" --timeout 60
```

| コマンド | 内容 |
| :--- | :--- |
| `lmm init [--root] [--name ADDR] [DIR]` | 設定ファイルとメールボックスを用意する |
| `lmm post [-t ADDR] [-s TEXT] [-r PATH] [--thread ID] [--forward] [-m K=V]... DIR` | メッセージディレクトリを送る |
| `lmm draft [-s TEXT] [--thread ID] [-m K=V]... [DIR]` | 下書きのディレクトリを作る |
| `lmm receive [--max N \| --all] [DIR]` | 新着を古いものから展開先へコピーする（既定は 1 通） |
| `lmm wait [--from ADDR] [--thread ID] [--in-reply-to ID] [--meta K=V]... [--timeout SEC] [--max N \| --all] [DIR]` | 条件に合うメッセージを待って展開する（既定は 1 通） |
| `lmm show (PATH \| --id ID) [--json \| --field KEY]` | メッセージを表示する |
| `lmm hook [DIR]` | AI CLI のフックから呼び、新着があれば通知する |
| `lmm box create/list/remove` | メールボックスを管理する |
| `lmm config show [-C DIR] [--origin]` | 有効な設定を表示する |

終了コードは、0 が成功、1 が実行時エラー、2 が使い方の誤り・設定エラー、3 が `lmm wait` の時間切れ、128+N がシグナルによる中断です。

## コマンドリファレンス

自分のアドレスは、環境変数 `LMM_NAME`、なければ個人設定（`.lmm.local.yml`）の `name` で決まります（オプションでは指定できません。[ワークツリーごとにアドレスを分ける](#ワークツリーごとにアドレスを分ける)を参照）。アドレスに使える文字は `a-z 0-9 . _ -` で、先頭は `a-z` か `0-9`、長さは 1〜64 文字です（`lmm` は予約済み）。

表の「既定値」は、省略したときに使う値です。`→` は、左から順に最初に決まったものを使うことを表します。「—」は、指定が必要なため既定値がないものです。

### `lmm init [--root] [--name ADDRESS] [DIR]`

`DIR`（省略時はカレントディレクトリ）に設定ファイルを作ります。`--name` を指定すると、そのアドレスのメールボックスも作ります。作成・追記したファイルのパスを 1 行ずつ出力します。

| 引数・オプション | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `DIR` | 任意 | カレントディレクトリ | 用意するディレクトリ |
| `--root` | 任意 | オフ | `DIR` をプロジェクトルートとして用意します。省略すると、`DIR` を外側のプロジェクトに加えます |
| `--name ADDRESS` | 条件付き（`--root` を省略した場合は必須） | なし | 自分のアドレス。`.lmm.local.yml` に書き、メールボックスを作ります |

- `--root` あり: `.lmm.yml`（`root: true`、`data_dir: "{root_dir}/.lmm"`、展開先と下書きの置き場）を、なければ作ります。`.gitignore` に `.lmm/` と `.lmm.local.yml` を追記し、`--name` があれば `name` だけを書いた `.lmm.local.yml` を作ります。
- `--root` なし: `root: false` と `name` を書いた `.lmm.local.yml` を作ります。コミットした `.lmm.yml` の複製（`root: true`）を打ち消して、外側のプロジェクトの設定（データディレクトリなど）を使うためです。上に `root: true` のプロジェクトがなければエラーです。`.lmm.yml` と `.gitignore` には触れません。
- `.lmm.local.yml` が別の内容ですでにある場合は、何も変えずにエラーです。同じ引数でもう一度実行しても、何も変えずに成功します。

### `lmm post [OPTIONS] DIR`

メッセージディレクトリ `DIR`（`index.md` と添付ファイル）を送ります。成功すると、`<lmm_id> <配送先の絶対パス>` を 1 行で出力します。シェルでは `read -r id path` で受け取れます。

| 引数・オプション | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `DIR` | 必須 | — | 送るメッセージディレクトリ。ディレクトリだけを受け付けます（ファイルや標準入力は不可）。中身は変更しません。受け取ったメッセージを渡せば、そのまま転送できます |
| `-t, --to ADDRESS` | 条件付き（`--reply` も `post.reply_to` もない場合は必須） | 返信元の送信者 → `post.reply_to` | 宛先。1 つだけ指定できます |
| `-s, --subject TEXT` | 任意 | 返信元の件名に `Re: ` を付けたもの → `index.md` の `lmm_subject` → なし | 件名 |
| `-r, --reply PATH` | 任意 | なし | 返信元のメッセージディレクトリ、またはその `index.md`。宛先・件名・スレッド ID を返信元から決め、`lmm_in_reply_to` を記録します。返信元の添付ファイルは引き継ぎません |
| `--thread ID` | 任意 | 返信元のスレッド ID（なければ返信元の ID）→ `index.md` の `lmm_thread_id` → なし | スレッド ID（1〜128 文字、改行不可） |
| `--forward` | 任意 | オフ | 受け取ったメッセージを転送し、`lmm_forwarded` に転送の履歴を残します。`--reply` とは同時に使えません |
| `-m, --meta KEY=VALUE` | 任意 | なし | 独自キーをフロントマターに追加します（複数回指定可）。キーは `A-Z a-z 0-9 _ . -` で、`lmm_` で始めることはできません。値は常に文字列になります。設定の `post.meta`、`index.md` の独自キーより優先します |

- `index.md` のフロントマターは省略できます。`lmm_` で始まるキーは、すべてシステムが決めた値で置き換えます。
- 宛先のメールボックスがない場合はエラーになります（自動では作りません）。
- 送信控えは、送信元の `sent/` に保存されます。

### `lmm draft [OPTIONS] [DIR]`

`lmm post` に渡すメッセージディレクトリと `index.md` を作り、その絶対パスを出力します。本文と添付ファイルは、出力されたディレクトリに置きます。

| 引数・オプション | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `DIR` | 任意 | 下書きの置き場（`LMM_DRAFT_DIR` → `draft.dir` → `.lmm-drafts/<自分のアドレス>/`）に作る `<日時>_<乱数>` | 作るディレクトリ。すでに存在する場合はエラーです |
| `-s, --subject TEXT` | 任意 | なし | 件名（`lmm_subject`） |
| `--thread ID` | 任意 | なし | スレッド ID（`lmm_thread_id`） |
| `-m, --meta KEY=VALUE` | 任意 | なし | 独自キー（複数回指定可）。規則は `lmm post -m` と同じです |

- 宛先は下書きに書きません。`lmm post` の `-t` か `--reply` で指定します。
- 送った後も下書きは残ります。不要になったら削除してください。

### `lmm receive [--max N | --all] [DIR]`

自分の `inbox/new/` の新着を、古いもの（送信時刻の早いもの）から順に展開先へコピーし、原本を `inbox/cur/` へ移します。展開したディレクトリの絶対パスを 1 行に 1 つ出力します。

| 引数・オプション | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `DIR` | 任意 | `LMM_RECEIVE_DIR` → `receive.dir` → `.lmm-inbox/<自分のアドレス>/` | 展開先。データディレクトリの中は指定できません |
| `--max N` | 任意 | `1` | 1 回に展開する件数の上限（1 以上）。残りは `new/` に残り、次の受信で展開されます |
| `--all` | 任意 | オフ | 件数の上限なしで、すべての新着を展開します。`--max` とは同時に使えません |

- 壊れたメッセージは `inbox/failed/` へ移し、終了コード 1 にします（ほかのメッセージの処理は続けます）。
- 展開したメッセージは、処理し終えたら削除してください（例: `rm -r <パス>`）。原本は `cur/` に残ります。

### `lmm wait [OPTIONS] [DIR]`

条件に合うメッセージが `inbox/new/` に届くまで待ち、届いたら `lmm receive` と同じように展開します。条件に合わないメッセージは `new/` に残します。

| 引数・オプション | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `DIR` | 任意 | `lmm receive` と同じ | 展開先 |
| `--from ADDRESS` | 任意 | なし（絞り込まない） | 送信元（`lmm_from`）が一致するものに限ります |
| `--thread ID` | 任意 | なし（絞り込まない） | スレッド ID（`lmm_thread_id`）が一致するものに限ります |
| `--in-reply-to ID` | 任意 | なし（絞り込まない） | 返信元の ID（`lmm_in_reply_to`）が一致するものに限ります。`lmm post` が出力した ID で、返信を待てます |
| `--meta KEY=VALUE` | 任意 | なし（絞り込まない） | 独自キーの値が文字列で、一致するものに限ります（複数回指定可）。引用符のない `1.10` のような数値は一致しません |
| `--timeout SEC` | 任意 | 無制限 | 待つ時間の上限（秒、小数可）。時間切れは終了コード 3 です |
| `--max N` | 任意 | `1` | 1 回に展開する件数の上限（古いものから順）。残りは `new/` に残ります |
| `--all` | 任意 | オフ | 件数の上限なしで、その時点で条件に合うものをすべて展開します。`--max` とは同時に使えません |

- 条件を複数指定した場合は、すべてを満たすものが対象です。条件がなければ、すべてのメッセージが対象です。
- `SIGINT` と `SIGTERM` では、130 と 143 で終了します。
- 同じアドレスで `lmm hook` も使うと、フックが先に受け取ってしまいます。`wait` を使うアドレスでは `hook` を使わないでください。

### `lmm show (PATH | --id ID) [--json | --field KEY]`

メッセージを表示します。表示しても、展開済みにはなりません。

| 引数・オプション | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `PATH` | どちらか一方が必須 | — | メッセージディレクトリ、またはその `index.md`。展開先のメッセージも指定できます |
| `--id ID` | どちらか一方が必須 | — | `lmm_id` で探します。探す範囲は、自分のメールボックスの `inbox/new/`、`inbox/cur/`、`inbox/failed/`、`sent/` です（この順で最初に見つかったもの） |
| `--json` | 任意 | オフ（`index.md` をそのまま出力） | `path`、`frontmatter`、`body`、`attachments` を持つ JSON を出力します |
| `--field KEY` | 任意 | なし | フロントマターの `KEY` の値だけを出力します。文字列はそのまま、それ以外は 1 行の JSON です。キーがなければ終了コード 1 です |

`PATH` と `--id` はどちらか一方を、`--json` と `--field` は多くとも一方を指定します。どちらも指定しない場合は、`index.md` の内容をそのまま出力します。

### `lmm hook [DIR]`

AI CLI のフックから呼ぶコマンドです。新着を展開し、1 通以上展開した場合だけ通知します。

| 引数 | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `DIR` | 任意 | `lmm receive` と同じ | 展開先 |

- 自分のアドレスが決まらない（`LMM_NAME` も `name` もない）なら、何もせずに終了コード 0 です。メールボックスがなければ、警告を出して 0 です。
- 設定ファイルに誤りがあると、アドレスが決まらない場合も終了コード 1 です（`name` を探すために設定ファイルを読むため）。
- 1 回に展開する件数は、設定の `hook.max_messages`（既定 20）までです。
- 通知は、`LMM_HOOK_CMD` か設定の `hook.cmd` のコマンドを `/bin/sh -c` で実行します。どちらもなければ、展開したパスの一覧を標準出力に出します。
- 通知コマンドには、次の環境変数を渡します。

  | 環境変数 | 値 |
  | :--- | :--- |
  | `LMM_RECEIVED` | 展開したディレクトリの絶対パス（改行区切り） |
  | `LMM_RECEIVED_COUNT` | 展開した件数 |
  | `LMM_PENDING_COUNT` | 上限のために `new/` に残った件数 |
  | `LMM_RECEIVE_DIR` | 展開先の絶対パス |
  | `LMM_NAME` | 受信者のアドレス（`name` で決まった場合も含む） |

- 通知コマンドを実行した場合は、その終了コードを返します。それ以外は 0 か 1 で、2 は返しません。
- 通知コマンドが `hook.timeout`（既定 30 秒）を超えたら、プロセスグループごと終了させます。

### `lmm box create ADDRESS [--exist-ok]`

メールボックスを作ります。メールボックスは、エージェントを起動する前に作ってください。設定の `data_dir` を使う場合は、そのデータディレクトリが有効な場所（ワークツリーの中など）で実行します。

| 引数・オプション | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `ADDRESS` | 必須 | — | 作るアドレス |
| `--exist-ok` | 任意 | オフ | すでに存在してもエラーにしません |

### `lmm box list`

メールボックスの一覧を、`<アドレス>\tnew=<未受信の件数>\tfailed=<検査で不合格の件数>` の形で出力します。

### `lmm box remove ADDRESS [--force]`

メールボックスを、受信済み（`cur/`）と送信控え（`sent/`）の履歴ごと削除します。

| 引数・オプション | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `ADDRESS` | 必須 | — | 削除するアドレス |
| `--force` | 任意 | オフ | 未受信のメッセージや書き込み中の送信があっても削除します |

前回の削除が途中で止まって残骸があれば、それを削除して完了させます。

### `lmm config show [-C DIR] [--origin]`

有効な設定を YAML で表示します。先頭に、読み込んだ設定ファイルの一覧と `{root_dir}` の値をコメントで出します。

| オプション | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `-C DIR` | 任意 | カレントディレクトリ | 設定を探し始めるディレクトリ。そのディレクトリで実行したものとして、データディレクトリとグローバル設定も決めます |
| `--origin` | 任意 | オフ | 各値の後ろに、その値を決めた場所（設定ファイルのパス、環境変数名、`(default)`）をコメントで付けます |

## 環境変数と設定

| 環境変数 | 設定キー | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `LMM_NAME` | `name` | なし | 自分のアドレス。設定は `.lmm.local.yml` にだけ書けます |
| `LMM_DATA_DIR` | `data_dir` | `~/.lmm` | データディレクトリ（すべてのメールボックスと `config.yml` を置く）。設定はグローバル設定には書けません |
| `LMM_CONFIG` | — | データディレクトリの `config.yml` | グローバル設定ファイル |
| `LMM_RECEIVE_DIR` | `receive.dir` | `.lmm-inbox/{name}` | 展開先 |
| `LMM_DRAFT_DIR` | `draft.dir` | `.lmm-drafts/{name}` | 下書きの置き場 |
| `LMM_HOOK_CMD` | `hook.cmd` | なし | `lmm hook` の通知コマンド |
| — | `hook.timeout` | `30` | 通知コマンドの実行時間の上限（秒） |
| — | `hook.max_messages` | `20` | `lmm hook` が 1 回に展開する件数（1〜30） |
| — | `post.reply_to` | なし | `lmm post` で宛先を省略したときの宛先 |
| — | `post.meta` | なし | `lmm post` が毎回付ける独自キー（マップ） |
| — | `max_message_size` | `1G` | 1 通の合計サイズの上限（グローバル設定でのみ有効） |

環境変数は設定ファイルより、コマンドラインの指定は環境変数より優先します。設定ファイルの書き方は、次の「設定ファイル」を参照してください。

## 設定ファイル

各コマンドの既定値は、YAML の設定ファイルで変えられます。すべての項目は省略できます。

### 置き場所と優先順位

| 種類 | 場所 | 用途 |
| :--- | :--- | :--- |
| グローバル | `$LMM_CONFIG`。未設定ならデータディレクトリの `config.yml`（既定は `~/.lmm/config.yml`） | 利用者全体（データディレクトリを分けた場合はそのデータディレクトリ）の既定値 |
| 共有 | 各ディレクトリの `.lmm.yml` | リポジトリにコミットして共有する設定 |
| 個人 | 各ディレクトリの `.lmm.local.yml` | その場所だけの個人設定（git の対象外にする） |

カレントディレクトリから親ディレクトリへ順にたどり、ファイルシステムのルートまでの各ディレクトリで `.lmm.local.yml` と `.lmm.yml` を探します。値は、次の順で上にあるものほど優先します。

1. コマンドラインのオプション・引数
2. 環境変数
3. カレントディレクトリの `.lmm.local.yml`
4. カレントディレクトリの `.lmm.yml`
5. 親ディレクトリの `.lmm.local.yml`、`.lmm.yml`（以下、ルートに向かって同様）
6. グローバル設定
7. 組み込みの既定値

- 各ディレクトリの `root` は、`.lmm.local.yml` を `.lmm.yml` に重ねて決めます（`.lmm.local.yml` の値が優先）。`true` なら、そのディレクトリより上の親は探しません。同じディレクトリのもう一方のファイルと、グローバル設定は引き続き使います。`.lmm.local.yml` に `root: false` を書けば、コミットした `.lmm.yml` の `root: true` を、その場所でだけ打ち消せます。
- グローバル設定はデータディレクトリにあるため、ディレクトリ側の設定ファイル（`data_dir`）と環境変数を先に読んでから読みます。優先順位は上のとおりです。
- `LMM_CONFIG` で指定したファイルがない場合はエラーです。既定の場所（データディレクトリの `config.yml`）にない場合は、グローバル設定なしとして扱います。
- 読み取り権限のないディレクトリやファイルは、警告を出して飛ばします。
- 実際に使われる値と、それを決めた場所は `lmm config show --origin` で確認できます。

### 書式

- YAML 1.2 の core schema で読みます。`yes`、`no`、`on`、`off` は真偽値ではなく文字列です。
- トップレベルはマップにします。同じキーを 2 回書くとエラーです。
- `null`（`~` や値の省略を含む）は書けません。書いた場合は設定エラー（終了コード 2）です。
- 未知のキーは、警告を出して無視します。
- 合成のしかた: 文字列・数値・真偽値は、優先する側の値で丸ごと置き換えます。マップ（`post.meta`）は、キーごとに合成します。

### 設定項目

| キー | 型 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `root` | 真偽値 | `false` | `true` なら、このファイルのあるディレクトリより上を探さない。同じディレクトリでは `.lmm.local.yml` の値が優先する。親や子のディレクトリには効かず、グローバル設定では無視する |
| `name` | 文字列（アドレス） | なし | 自分のアドレス。環境変数 `LMM_NAME` で上書きできる。`.lmm.local.yml` でのみ有効で、`.lmm.yml` とグローバル設定に書くと警告を出して無視する |
| `data_dir` | 文字列（パス） | `~/.lmm` | データディレクトリ。環境変数 `LMM_DATA_DIR` で上書きできる。展開後に絶対パスでなければならない。グローバル設定に書くと警告を出して無視する |
| `receive.dir` | 文字列（パス） | `.lmm-inbox/{name}` | `lmm receive`・`wait`・`hook` の展開先。環境変数 `LMM_RECEIVE_DIR` で上書きできる |
| `draft.dir` | 文字列（パス） | `.lmm-drafts/{name}` | `lmm draft` の下書きの置き場。環境変数 `LMM_DRAFT_DIR` で上書きできる |
| `hook.cmd` | 文字列 | なし（組み込みの出力） | `lmm hook` の通知コマンド。`/bin/sh -c` で実行する。プレースホルダは展開しない。環境変数 `LMM_HOOK_CMD` で上書きできる |
| `hook.timeout` | 数値（秒） | `30` | 通知コマンドの実行時間の上限。0 より大きい値（小数可） |
| `hook.max_messages` | 整数 | `20` | `lmm hook` が 1 回に展開・通知する件数の上限。1〜30 |
| `post.reply_to` | 文字列（アドレス） | なし | `lmm post` で宛先を省略したときの宛先 |
| `post.meta` | マップ（キー・値とも文字列） | なし | `lmm post` が毎回フロントマターに付ける独自キー。キーは `A-Z a-z 0-9 _ . -` で、`lmm_` で始めることはできない。数値なども引用符で囲んで文字列にする |
| `max_message_size` | 整数（バイト）、または `K`・`M`・`G` 付きの文字列（1024 倍単位） | `1G` | 1 通の合計サイズの上限。グローバル設定でのみ有効で、ディレクトリ側に書くと警告を出して無視する |

### プレースホルダとパス

文字列の値では、次のプレースホルダを使えます（`hook.cmd` を除く）。`{` を文字として書く場合は `{{` とします。未知のプレースホルダは設定エラーです。

| プレースホルダ | 値 |
| :--- | :--- |
| `{name}` | 自分のアドレス。決まらないときに使うとエラー。`data_dir` では使えない |
| `{cwd}` | カレントディレクトリ |
| `{config_dir}` | その値を書いた設定ファイルのあるディレクトリ |
| `{root_dir}` | 設定ファイルの探索が止まったディレクトリ（`root` が `true` になったディレクトリ）。値を書いたファイルによらない。見つからないときに使うとエラー |

パスを表す値（`receive.dir`、`draft.dir`、`data_dir`）は、プレースホルダを展開した後、先頭の `~/` をホームディレクトリに置き換えます。相対パスは、設定ファイルの場所ではなく**カレントディレクトリ**を基準に解決します。設定ファイルの場所を基準にしたい場合は `{config_dir}`、プロジェクトルート（探索が止まったディレクトリ）を基準にしたい場合は `{root_dir}` を使ってください。環境変数で指定した値は、プレースホルダを展開しません。

`data_dir` だけは、展開した後に絶対パスでなければなりません（`{root_dir}`、`{config_dir}`、`{cwd}`、`~/` で始めるか、絶対パスを書く）。相対パスを許すと、サブディレクトリごとに別のデータディレクトリになり、メールが届かなくなるためです。

### 例

リポジトリ直下の `.lmm.yml`（コミットして共有する）:

```yaml
root: true                                  # これより上の .lmm.yml は読まない
receive:
  dir: "{config_dir}/.lmm-inbox/{name}"     # サブディレクトリで起動しても、展開先はリポジトリ直下
draft:
  dir: "{config_dir}/.lmm-drafts/{name}"
hook:
  timeout: 10
  max_messages: 5
post:
  reply_to: manager
  meta:
    project: myrepo
    version: "1.10"                         # 値は文字列にする（引用符で囲む）
```

グローバル設定 `~/.lmm/config.yml`:

```yaml
max_message_size: 100M
post:
  meta:
    host: my-laptop
```

この 2 つがあるリポジトリ内では、`post.meta` は `project`、`version`、`host` の 3 つになります（マップはキーごとに合成されるため）。

### ワークツリーごとにアドレスを分ける

git worktree ごとにエージェントを割り当てる場合は、各ワークツリーの直下の `.lmm.local.yml`（git の対象外）にアドレスを書くと、環境変数なしで、ワークツリーの中で実行した `lmm` と `lmm hook` がそのアドレスを使います。ワークツリーをメインのチェックアウトの中に置けば、データディレクトリはコミットした `.lmm.yml` に `{root_dir}` で書くだけで済みます。これらのファイルとメールボックスは `lmm init` で用意できます。

```bash
cd <repo> && lmm init --root --name root    # プロジェクトルート（最初に 1 回）
cd <repo>/feat-foo && lmm init --name coder # ワークツリーを追加するたび
```

作られるファイルは次のとおりです。

```
<repo>/                  # プロジェクトルート。メインのチェックアウト（アドレス root）
├── .lmm.yml             # コミットしたもの（root: true、data_dir）
├── .lmm.local.yml       # name: root
├── .lmm/                # データディレクトリ（git の対象外にする）
└── feat-foo/            # ワークツリー（git の対象外にする）
    ├── .lmm.yml         # コミットしたものの複製
    └── .lmm.local.yml   # root: false、name: coder
```

```yaml
# <repo>/.lmm.yml（コミットする）
root: true
data_dir: "{root_dir}/.lmm"   # ワークツリーの中でも <repo>/.lmm を指す
```

```yaml
# <repo>/feat-foo/.lmm.local.yml
root: false   # 複製の .lmm.yml の root: true を打ち消し、<repo> まで探索する
name: coder   # 必ず書く。書かないと <repo> の name（root）が使われる
```

- アドレスはカレントディレクトリで決まります。別のワークツリーへ移動して実行すると、そのワークツリーのアドレスになります。1 つのディレクトリで複数のエージェントが動く場合は、`LMM_NAME` を使ってください。
- `{root_dir}` は探索が止まったディレクトリなので、`feat-foo` の中では `root: false` によって `<repo>` を指します（`lmm config show` の先頭に表示されます）。`{config_dir}` を使うと、ワークツリーの複製では `feat-foo` を指し、ワークツリーの間でメールが届かなくなります。
- `<repo>/.lmm/` は `lmm init --root` が `.gitignore` に追記します（手で用意する場合は追記してください）。`git clean -fdx` を実行すると消えます。作業ツリーの外に置くなら `"{root_dir}/.git/lmm"` とします。
- ワークツリーをメインのチェックアウトの外に置く場合（`git worktree add ../feat-foo` など）は、探索がプロジェクトルートを通らないため、`{root_dir}` はワークツリー自身になります。各ワークツリーの `.lmm.local.yml` に、同じ場所を指す `data_dir` を書いてください（例: `"{config_dir}/../repo/.lmm"`）。
- ワークツリーの外（`data_dir` のない場所）では、データディレクトリは `~/.lmm` に戻ります。

詳細は [docs/prd.md](docs/prd.md) の §3.2、§4.1、§10 を参照してください。

## Claude Code との連携

`.claude/settings.json` の `Stop` フックに `lmm hook` を登録します。

```json
{
  "hooks": {
    "Stop": [
      { "hooks": [{ "type": "command", "command": "lmm hook" }] }
    ]
  }
}
```

リポジトリ直下の `.lmm.yml` で、`Stop` フック用の JSON を出す通知コマンドを設定します（`jq` が必要です）。

```yaml
root: true
receive:
  dir: "{config_dir}/.lmm-inbox/{name}"
hook:
  cmd: >-
    jq -n --arg paths "$LMM_RECEIVED"
    '{decision: "block", reason: ("新着メッセージがあります。次のディレクトリの index.md を読んで対応し、処理後に削除してください:\n" + $paths)}'
```

## 開発

```bash
mise run build   # cargo build
mise run test    # cargo test（単体テスト・結合テスト）
mise run lint    # rustfmt --check、clippy -D warnings、actionlint（ワークフローの検査）
mise run fmt     # 整形
```

CI（GitHub Actions）は、ubuntu-latest と macos-latest で `mise run lint` と `mise run test` を実行します。

### リリース

1. `Cargo.toml` の `version` を上げ、`cargo build` で `Cargo.lock` も更新して、両方をコミットする（`Cargo.lock` が古いと `--locked` のビルドが失敗します）。
2. プルリクエストをマージした後、`main` のそのコミットに同じバージョンのタグを付けて push する。

```bash
git switch main && git pull --ff-only
git tag v0.1.0
git push origin v0.1.0
```

`.github/workflows/release.yml` が、4 つのターゲットでテストとビルドを行い、GitHub Release にアーカイブと `SHA256SUMS` を添付します。

- タグが `Cargo.toml` のバージョンと一致しない場合や、タグのコミットが `main` に含まれない場合は、何も公開せずに失敗します。
- `-` を含むタグ（例: `v0.2.0-rc.1`）は、プレリリースとして公開します。
- Actions の画面から手動で実行（`workflow_dispatch`）すると、ビルドとテストだけを行い、公開はしません。リリースの前の確認に使えます。

## ライセンス

[MIT](LICENSE)
