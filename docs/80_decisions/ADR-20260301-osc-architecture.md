# ADR-20260301: OSC Tap を semi-deterministic (Post-MVP) とし Hooks を維持する

- **Date**: 2026-03-01
- **Status**: Accepted
- **Deciders**: Orchestrator + User

---

## 背景・問題

Claude Code の OSC シーケンス調査と 2 つの外部レポート評価を踏まえ、以下の設計判断が必要になった:

1. `pipe-pane` で OSC シーケンスを取得できると判明 → source rank の再設計が必要
2. 外部レポートの一方（Architecture Proposal）が OSC 133 + pipe-pane を rank-0 deterministic とし、Hooks deprecated を提案
3. hooks の setup 負荷がユーザー体験の課題として提起された

---

## 調査結果

### Claude Code が emit する OSC シーケンス（実態）

| Sequence | 内容 | 状況 |
|----------|------|------|
| OSC 9;4 | Progress bar: state=3 (indeterminate) = running, state=0 = done | ✅ emit 確認 |
| OSC 9 | Desktop notification | ✅ emit 確認（tmux passthrough 依存） |
| OSC 2/0 | Terminal title | ✅ emit（`/rename` 時のみ） |
| **OSC 133** | **Shell integration** | **✗ Claude Code は emit しない** |

**OSC 133 の状況**: bash/zsh/fish の shell integration スクリプトが emit するもので、TUI アプリは対象外。
GitHub issue #26235 が「Claude Code に OSC 133 を追加してほしい」という **open feature request** として存在する（= 現在は実装されていない）。

Architecture Proposal が「OSC 133 を pipe-pane で取得できた」と主張する実験結果は、pane 内 shell に shell integration スクリプトが入っていた環境でのシェルのシーケンスであり、Claude Code 自体のシーケンスではないと判断した。

---

## 決定

### 採択: Hooks 維持 + OSC 9;4 を Post-MVP semi-deterministic として追加

```
TIER 1 (deterministic):     hooks (rank 0) — if configured, setup via agtmux setup-hooks
TIER 1 (deterministic):     JSONL (rank 1) — transcript_path > fd-based > CWD-based
TIER 2 (semi-deterministic): osc_tap (rank 2) — OSC 9;4 via pipe-pane [Post-MVP]
TIER 3 (heuristic):          poller (rank 3)
```

### Hooks の役割（維持する理由）

OSC と JSONL だけでは代替不能な機能が hooks にある:

| 検出対象 | OSC | JSONL | Hooks |
|----------|-----|-------|-------|
| WaitingApproval (deterministic) | ✗ | △ (遅延) | ✅ PermissionRequest hook |
| SessionEnd 即時検出 | ✗ | ✗ (15s void) | ✅ SessionEnd hook |
| transcript_path (JSONL direct binding) | ✗ | ✗ | ✅ SessionStart payload |
| Running upstream 検出 | ✗ | △ | ✅ UserPromptSubmit hook |
| Error (is_interrupt 付き) | ✗ | △ | ✅ PostToolUseFailure hook |

Setup 負荷の解決策: `agtmux setup-hooks` コマンドが `~/.claude/settings.json` を自動更新する。ゼロ摩擦。

### OSC Tap の位置づけ（Post-MVP、semi-deterministic）

- **採用するシグナル**: OSC 9;4（progress bar）のみ
- **採用しないシグナル**: OSC 133（Claude Code が emit しないため）
- **confidence**: 0.92（process_hint の 1.00 未満、cmd_match の 0.86 超）
- **capability-gated**: tmux 3.3+ AND pipe-pane 先占競合なし
- **負の制約**: OSC 不在は negative evidence に使用しない

将来 Claude Code が OSC 133 を実装した場合（issue #26235 resolve 時）は、osc_tap の rank 昇格と hooks との統合を新 ADR で再検討する。

---

## 却下した代替案

### 却下案 1: OSC Tap を rank-0 として Hooks を deprecated

**却下理由**:
1. OSC 133 という存在しないシーケンスへの依存（Architecture Proposal の根本的な事実誤認）
2. WaitingApproval の deterministic 検出穴が残る
3. SessionEnd による即時終了検出が失われる
4. transcript_path による JSONL direct binding が失われる
5. pipe-pane 先占競合でユーザーの既存設定を破壊するリスク

### 却下案 2: OSC 完全不採用

**却下理由**:
- OSC 9;4 は Claude Code が emit することが確認済み
- pipe-pane 経由での取得が実証可能
- deterministic/JSONL が弱い瞬間の補強として有用
- capability-gated にすれば利用不可環境への影響なし

### 却下案 3: fd-based discovery を CWD-based の完全代替に

**却下理由**:
- Claude Code (Node.js) が JSONL ファイルを常に open 保持するか未確認
- transcript_path (hooks 経由) が最も信頼性が高い primary path
- CWD-based は既存 fallback として維持するコストが低い

---

## 結果・影響

- Phase 8 タスク T-E01〜T-E04 として実装（`docs/60_tasks.md` 参照）
- 新 FR-053〜FR-060 として spec に追加（`docs/20_spec.md` 参照）
- C-017 `agtmux-source-osc-tap` として architecture に追加（`docs/30_architecture.md` 参照）
