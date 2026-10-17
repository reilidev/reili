# 引き継ぎメモ: `claude/goal-driven-teammate`

別のセッションでこのブランチの作業を続けるためのメモです。**PR を出す前（main にマージする前）にこのファイルは削除してください。**

## 1. 新しいセッションに貼るプロンプト

```
reilidev/reili の claude/goal-driven-teammate ブランチの作業を引き継いでください。
まずリポジトリ直下の HANDOFF.md を読み、「4. 進行中のタスク」から再開してください。
会話は日本語でお願いします。情報の根拠となる URL を示してください。
PR は明示的に頼むまで作らないでください。
```

## 2. 目的と経緯

- 目的: Slack で頼まれたときだけ単発で調査する Reili を、`reili.toml` に書いた長期的な
  オブジェクティブに向かって自走するチームメイトにする。
- 設計思想の参考: https://felixrieseberg.com/how-to-be-more-ambitious-a-tutorial-for-humans-with-models/
  （細かいタスクではなく目標を渡し、モデルを長く自走させ、人間は結果を見て舵を取る）
- 何度もの見直しで、次のものはユーザーの判断で**削除済み**（再追加しないこと）:
  - 検証（verification）、`reili eval`、効果測定・成功条件の測定
  - アラート起点の実行、`goal run`、`/reili rerun`、Slack や Issue からのコマンド
  - オブジェクティブ専用の台帳 Issue、本文の機械向け JSON、revision/schema_version、
    リース、段階チェックポイント、試行回数、同一スロット再試行とバックオフ、
    類似度による重複判定、Slack 報告の頻度制限、1日のトークン予算
- 名称: goal → objective、Investigate → Work、ステータス investigating → in-progress に変更済み。

## 3. 守ること（ユーザー指定の制約）

- SQLite・PostgreSQL・Redis などの独自の永続 DB を導入しない。
- 本番環境の変更、デプロイ、ロールバック、監視設定の変更は禁止のまま。既存の安全性を損なわない。
- 書き込みの例外は、明示的に設定した 1 リポジトリへの Issue・コメント・ラベル操作だけ。
- PR はユーザーが明示的に頼むまで作らない。
- コミットメッセージの末尾に次の 2 行を付ける（セッション URL は作業しているセッションのもの）:
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/<session>
  ```
- コミット・PR・コードにモデル識別子を書かない。
- 会話は日本語。根拠となる URL を示す。
- CLAUDE.md のルール（`crates/` で `cargo fmt --all`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace`、README など文書の更新）に従う。

## 4. 進行中のタスク（ここから再開）

**ユーザーの依頼:**「各ファイルに書いているコメントはリーダブルコードの原則に従って見直して。ほとんど不要だと思う。」

- 対象: このブランチで追加したコメントだけ（main にもともとあるコメントには触れない）。
- 方針:
  - コードを読めば分かること、名前や型で表せることの言い換えは削除する。
  - モジュール冒頭の `//!` は main ではほとんど使われていない（183 ファイル中 5 ファイル）ので、原則削除する。
  - 残すのは、コードから読み取れない「なぜ」だけ。例:
    - GitHub Apps は担当者に確実には設定できないので、担当をラベルで代用している
    - 検索 API は反映が遅れるので、一覧 API を使う
    - コピーされたマーカーは信頼しない
    - 422 / 404 の扱い
    - 作成順の一覧に依存した重複の解消
    - レポート提出ツールは予算を超えても常に許可する理由
  - トレイトのメソッドの契約（並び順、`closed_since` の意味など）は、シグネチャから分からないので短く残してよい。
  - テストの区切りコメント（`// A. ... ----`）は削除する。テスト名で十分。
- 状況: 対象行の洗い出しまで完了。**ファイルの編集はまだしていない。**
- 対象行の一覧は次のスクリプトで再生成できる（リポジトリ直下で実行。約 340 行、約 50 ファイル）:

  ```python
  import subprocess, re
  files = subprocess.run(['git','diff','main...HEAD','--name-only','--','crates/**/*.rs'],
                         capture_output=True, text=True).stdout.split()
  for f in files:
      try: lines = open(f).read().split('\n')
      except FileNotFoundError: continue
      d = subprocess.run(['git','diff','-U0','main...HEAD','--',f], capture_output=True, text=True).stdout
      added, new = set(), None
      for l in d.split('\n'):
          m = re.match(r'^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@', l)
          if m: new = int(m.group(1)); continue
          if new is None: continue
          if l.startswith('+') and not l.startswith('+++'): added.add(new); new += 1
          elif not l.startswith('-'): new += 1
      out = [(i+1, lines[i]) for i in range(len(lines)) if (i+1) in added and re.match(r'^\s*//', lines[i])]
      if out:
          print(f'=== {f}')
          for n, t in out: print(f'{n}: {t.strip()}')
  ```

- 終わったら fmt / clippy / test を通し、コミットしてプッシュする。docs/ や README の文章は対象外（コード内コメントのみ）。

## 5. 現在の設計（要点）

Reili は**チームと同じ GitHub Issue の上で、チームメイトとして働く**。Reili 専用の状態は持たない。

| ラベル | 意味 |
|---|---|
| `reili-objective:<id>` | どのオブジェクティブの Issue か（絞り込むと進み具合が見える） |
| `reili:assignee` | Reili の担当（GitHub App は担当者に確実に設定できないためラベルで代用） |
| `reili:needs-decision` | チームの判断待ち（質問は Reili の最新コメントにある） |

- Reili の担当は「上の 2 つのラベルが付いた open の Issue」。メンバーが作った Issue でもアサインされれば引き受ける。
  ラベルを外されたら手を引く。not planned で閉じられたものは開き直さない。
- 進捗は Issue ごとにコメント 1 件。新しい仕事は新しい Issue（本文末尾に見えないメタデータ
  `<!-- reili:work-item key=... -->` を入れ、本文は作成後に書き換えない）。
- Reili が閉じるのは自分で作った Issue だけ。人の Issue は「よければ閉じてください」とコメントする。
- Reili の最後のコメントより後の、owner・member・collaborator のコメントを「新しい指示」として AI に渡す。
  それ以外の作者のコメントは件数だけ伝える。
- サイクル: Observe → Work（AI）→ Decide → Report → Learn。AI が動くのは Work だけで、書き込みはプログラムが行う。
- スケジューラは cron の予定時刻にだけ動く。停止中の時刻は取り戻さない。前回がまだ動いていればその回は飛ばす。
- Slack に投稿するのは、新規 Issue・判断待ち・完了があったときだけ。キー `objective:slot` で二重投稿を防ぐ。
- 予算は 1 サイクルあたりの時間・トークン・ツール呼び出し回数・新規 Issue 数の 4 つ。
- トラッカーは差し替えられる: `WorkStatePort`（`crates/core/src/work/port.rs`）。今は GitHub だけ。
  JIRA などを追加するときは、`assert_work_state_contract`（`crates/core/src/work/conformance.rs`）に通す。

主なファイル:

- core: `work/{item,port,record,report,evidence,sanitize,conformance}.rs`、`cycle_agent/mod.rs`、`objective/`
- application: `objective/{scheduler,brief,policy,decide,render,report}.rs`、
  `objective/cycle/{mod,observe,work,apply,notify,learn}.rs`、`testing.rs`（偽物）、`scenarios.rs`（受け入れテスト）
- adapters: `outbound/github/work_state/{repository,codec,fake_github,tests}.rs`、
  `outbound/agents/tools/submit_report.rs`、`outbound/agents/task_agent/objective_instructions.rs`
- runtime: `config/{objective_file,objective_loader,model}.rs`、`bootstrap/objectives.rs`、
  `bootstrap/objectives_integration_tests.rs`
- 文書: `docs/autonomy.md`（仕様）、`docs/architecture.md`、`docs/capabilities-and-permissions.md`、`README.md`

## 6. 検証コマンド

`crates/` で実行する:

```
cargo fmt --all
CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets -- -D warnings
CARGO_INCREMENTAL=0 cargo test --workspace --locked --all-targets
```

最新コミット f81b3a4 時点で、テストは 625 件すべて成功（adapters 312 / application 104 / core 79 / runtime 127 / main 3）。
clippy の警告もない。

## 7. 参考

- 解説アーティファクト（新人向け図解。新しい設計に更新済み）: https://claude.ai/artifact/C6HdfZjEWBsm2zUGNt24nQ
- 直近の大きな変更: https://github.com/reilidev/reili/commit/f81b3a4
- GitHub 担当者 API: https://docs.github.com/en/rest/issues/assignees
- GitHub ラベル API: https://docs.github.com/en/rest/issues/labels
- 未確認事項: 本物の LLM・GitHub・Datadog・Slack にはまだ接続していない。テストはすべて偽物を相手にしたもの。
