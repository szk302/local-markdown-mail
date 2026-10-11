# local-markdown-mail

`lmm`（Local Markdown Mail）は、ローカルのファイルシステムだけを使って、AI エージェントと人の間で Markdown メッセージをやり取りする CLI です。常駐プロセスやデータベースは使わず、`~/.lmm` 以下のディレクトリを `ls` と `cat` だけで追えます。仕様は [docs/prd.md](docs/prd.md) を参照してください。

## インストール

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

# 受け取る（既定の展開先は ./.lmm-inbox/<LMM_NAME>/）
LMM_NAME=manager lmm receive

# 表示する・返信する・待つ
lmm show --id "$id" --json
LMM_NAME=manager lmm post --reply .lmm-inbox/manager/<msg> <DIR>
lmm wait --in-reply-to "$id" --timeout 60
```

| コマンド | 内容 |
| :--- | :--- |
| `lmm post [-t ADDR] [-s TEXT] [-r PATH] [--thread ID] [--forward] [-m K=V]... DIR` | メッセージディレクトリを送る |
| `lmm draft [-s TEXT] [--thread ID] [-m K=V]... [DIR]` | 下書きのディレクトリを作る |
| `lmm receive [--max N] [DIR]` | 新着を展開先へコピーする |
| `lmm wait [--from ADDR] [--thread ID] [--in-reply-to ID] [--meta K=V]... [--timeout SEC] [--max N] [DIR]` | 条件に合うメッセージを待って展開する |
| `lmm show (PATH \| --id ID) [--json \| --field KEY]` | メッセージを表示する |
| `lmm hook [DIR]` | AI CLI のフックから呼び、新着があれば通知する |
| `lmm box create/list/remove` | メールボックスを管理する |
| `lmm config show [-C DIR] [--origin]` | 有効な設定を表示する |

終了コードは、0 が成功、1 が実行時エラー、2 が使い方の誤り・設定エラー、3 が `lmm wait` の時間切れ、128+N がシグナルによる中断です。

## コマンドリファレンス

自分のアドレスは環境変数 `LMM_NAME` で決まります（オプションでは指定できません）。アドレスに使える文字は `a-z 0-9 . _ -` で、先頭は `a-z` か `0-9`、長さは 1〜64 文字です（`lmm` は予約済み）。

表の「既定値」は、省略したときに使う値です。`→` は、左から順に最初に決まったものを使うことを表します。「—」は、指定が必要なため既定値がないものです。

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
| `DIR` | 任意 | 下書きの置き場（`LMM_DRAFT_DIR` → `draft.dir` → `.lmm-drafts/<LMM_NAME>/`）に作る `<日時>_<乱数>` | 作るディレクトリ。すでに存在する場合はエラーです |
| `-s, --subject TEXT` | 任意 | なし | 件名（`lmm_subject`） |
| `--thread ID` | 任意 | なし | スレッド ID（`lmm_thread_id`） |
| `-m, --meta KEY=VALUE` | 任意 | なし | 独自キー（複数回指定可）。規則は `lmm post -m` と同じです |

- 宛先は下書きに書きません。`lmm post` の `-t` か `--reply` で指定します。
- 送った後も下書きは残ります。不要になったら削除してください。

### `lmm receive [--max N] [DIR]`

自分の `inbox/new/` の新着を展開先へコピーし、原本を `inbox/cur/` へ移します。展開したディレクトリの絶対パスを 1 行に 1 つ出力します。

| 引数・オプション | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `DIR` | 任意 | `LMM_RECEIVE_DIR` → `receive.dir` → `.lmm-inbox/<LMM_NAME>/` | 展開先。データディレクトリの中は指定できません |
| `--max N` | 任意 | 上限なし | 1 回に展開する件数の上限（1 以上）。残りは `new/` に残り、次の受信で展開されます |

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
| `--max N` | 任意 | 上限なし | 1 回に展開する件数の上限 |

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

- `LMM_NAME` が未設定なら、何もせずに終了コード 0 です。メールボックスがなければ、警告を出して 0 です。
- 1 回に展開する件数は、設定の `hook.max_messages`（既定 20）までです。
- 通知は、`LMM_HOOK_CMD` か設定の `hook.cmd` のコマンドを `/bin/sh -c` で実行します。どちらもなければ、展開したパスの一覧を標準出力に出します。
- 通知コマンドには、次の環境変数を渡します。

  | 環境変数 | 値 |
  | :--- | :--- |
  | `LMM_RECEIVED` | 展開したディレクトリの絶対パス（改行区切り） |
  | `LMM_RECEIVED_COUNT` | 展開した件数 |
  | `LMM_PENDING_COUNT` | 上限のために `new/` に残った件数 |
  | `LMM_RECEIVE_DIR` | 展開先の絶対パス |
  | `LMM_NAME` | 受信者のアドレス |

- 通知コマンドを実行した場合は、その終了コードを返します。それ以外は 0 か 1 で、2 は返しません。
- 通知コマンドが `hook.timeout`（既定 30 秒）を超えたら、プロセスグループごと終了させます。

### `lmm box create ADDRESS [--exist-ok]`

メールボックスを作ります。メールボックスは、エージェントを起動する前に作ってください。

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

有効な設定を YAML で表示します。先頭に、読み込んだ設定ファイルの一覧をコメントで出します。

| オプション | 必須 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `-C DIR` | 任意 | カレントディレクトリ | 設定を探し始めるディレクトリ |
| `--origin` | 任意 | オフ | 各値の後ろに、その値を決めた場所（設定ファイルのパス、環境変数名、`(default)`）をコメントで付けます |

## 環境変数と設定

| 環境変数 | 設定キー | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `LMM_NAME` | — | なし | 自分のアドレス |
| `LMM_HOME` | — | `~/.lmm` | データディレクトリ（すべてのメールボックスと `config.yml` を置く） |
| `LMM_CONFIG` | — | `$LMM_HOME/config.yml` | グローバル設定ファイル |
| `LMM_RECEIVE_DIR` | `receive.dir` | `.lmm-inbox/{name}` | 展開先 |
| `LMM_DRAFT_DIR` | `draft.dir` | `.lmm-drafts/{name}` | 下書きの置き場 |
| `LMM_HOOK_CMD` | `hook.cmd` | なし | `lmm hook` の通知コマンド |
| — | `hook.timeout` | `30` | 通知コマンドの実行時間の上限（秒） |
| — | `hook.max_messages` | `20` | `lmm hook` が 1 回に展開する件数（1〜30） |
| — | `post.reply_to` | なし | `lmm post` で宛先を省略したときの宛先 |
| — | `post.meta` | なし | `lmm post` が毎回付ける独自キー（マップ） |
| — | `max_message_size` | `1G` | 1 通の合計サイズの上限（グローバル設定でのみ有効） |

- 設定ファイルは、グローバル設定と、カレントディレクトリから上にたどった各ディレクトリの `.lmm.yml`（共有用）・`.lmm.local.yml`（個人用）です。近いものほど優先し、`root: true` のファイルより上は探しません。
- 環境変数は設定ファイルより、コマンドラインの指定は環境変数より優先します。
- 設定の値では、`{name}`（`LMM_NAME`）、`{cwd}`、`{config_dir}`（その設定ファイルのあるディレクトリ）を使えます。`hook.cmd` は展開しません。

詳細は [docs/prd.md](docs/prd.md) の §10 を参照してください。

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
mise run lint    # rustfmt --check と clippy -D warnings
mise run fmt     # 整形
```

CI（GitHub Actions）は、ubuntu-latest と macos-latest で `mise run lint` と `mise run test` を実行します。
