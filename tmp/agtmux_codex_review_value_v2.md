判定は **Go with changes** です。現状のままだと「価値が出る前に作り込みが先行」しやすいので、MVP境界の再定義が必須です。

1. **Critical**: MVP定義が衝突していて優先順位が固定できない  
根拠: `docs/agtmux-spec.md:31`, `docs/agtmux-spec.md:67`, `docs/agtmux-spec.md:356`, `docs/agtmux-spec.md:376`, `docs/agtmux-spec.md:426`  
具体改善案: 「MVPで何を出荷するか」を章として明文化し、FRを `MVP`/`Post-MVP` に分割。GeminiはMVP外へ統一。

2. **Critical**: `unknown` の扱いが安全要件と矛盾する  
根拠: `docs/agtmux-spec.md:86`, `docs/agtmux-spec.md:139`, `docs/agtmux-spec.md:146`, `docs/agtmux-spec.md:155`, `docs/agtmux-spec.md:163`  
具体改善案: `target_unreachable`/`stale_signal` 時は precedence をバイパスして `unknown` 強制。もしくは `activity_state` と `health_state` を分離。

3. **High**: 価値提供前に基盤が重すぎる（daemon/registry/dedupe完全性）  
根拠: `docs/agtmux-spec.md:98`, `docs/agtmux-spec.md:106`, `docs/agtmux-spec.md:110`, `docs/agtmux-spec.md:344`, `docs/agtmux-spec.md:350`  
具体改善案: Phase 0を「単一プロセスCLI + 最小SQLite + list/watch」へ縮小。`agtmuxd`常駐必須は後ろ倒し。

4. **High**: Multi-target運用の初期負荷が高い  
根拠: `docs/agtmux-spec.md:75`, `docs/agtmux-spec.md:76`, `docs/agtmux-spec.md:277`, `docs/agtmux-spec.md:278`, `docs/agtmux-spec.md:356`  
具体改善案: MVPは `host` 固定または静的 `targets.yaml` 読み取りのみ。`add/connect/remove` は後回し。

5. **High**: データ保護と保守負荷（`connection`/`raw_payload`）が未定義  
根拠: `docs/agtmux-spec.md:192`, `docs/agtmux-spec.md:225`  
具体改善案: DBに秘密情報を置かない（SSH aliasのみ保存）。`raw_payload` はデフォルトでredact、保持期間TTL（例7日）を仕様化。

6. **High**: 2秒ポーリングはコスト上限が未定義  
根拠: `docs/agtmux-spec.md:85`, `docs/agtmux-spec.md:270`  
具体改善案: CPU/メモリ/SSH同時実行上限をNFRに数値で追加（例 CPU <5%、targetごと同時2本まで）。

7. **Medium**: CLI面が広く、初期学習コストが高い  
根拠: `docs/agtmux-spec.md:283`, `docs/agtmux-spec.md:285`, `docs/agtmux-spec.md:289`, `docs/agtmux-spec.md:290`  
具体改善案: MVPコマンドを `list panes`, `watch`, `attach` に限定。`windows/sessions`集約は次段。

8. **Medium**: FR-15とCLI引数が不整合  
根拠: `docs/agtmux-spec.md:81`, `docs/agtmux-spec.md:286`, `docs/agtmux-spec.md:288`  
具体改善案: `attach`/`view-output` に `--if-runtime` を追加、またはFR-15を「破壊的操作のみ」に限定。

9. **Medium**: 早期に厳しすぎる品質契約  
根拠: `docs/agtmux-spec.md:90`, `docs/agtmux-spec.md:92`  
具体改善案: `adapter contract stability` と `deterministic convergence` は GA前提へ移動。MVPは「誤検知時unknownへ倒す」を優先。

10. **Low**: 将来機能の詳細がMVP判断をノイズ化  
根拠: `docs/agtmux-spec.md:117`, `docs/agtmux-spec.md:328`, `docs/agtmux-spec.md:385`  
具体改善案: Copilot/Cursor/macOS app詳細は Appendix 化。

### MVPで残すべき項目
1. `state正規化` と `pane-runtime対応付け`（`FR-2`, `FR-3`）  
2. `list panes` と `watch`（`FR-4` の最小サブセット）  
3. `unknownへ安全に倒す`（`NFR-2`）  
4. `stale自己修復`（`FR-8` の最小実装）  
5. `--json`（まず `list panes` のみ）

### 後回し/削るべき項目
1. `target add/connect/remove` 一式（`FR-10`）  
2. `adapter registry` の厳密運用（`FR-11`, `NFR-6`）  
3. `Copilot/Cursor` 前提の仕様（`FR-12`）  
4. 厳密な順序/重複保証のフル仕様（`FR-14`, `NFR-8`）  
5. `send/view-output/kill`（Phase 1.5以降）  
6. macOS resident app詳細（Phase 3）

### 反映推奨TOP5
1. MVP章を新設し、FRを `MVP/Post-MVP` に再分類  
2. `unknown` 強制遷移条件を仕様に追加（安全矛盾の解消）  
3. Phase 0を「最小縦切り」に削減（list/watchを先に出す）  
4. データ保護要件（secret禁止・payload redaction・TTL）を追加  
5. Multi-target運用を段階導入（初期はhost固定または静的設定）

### 修正案・コード例
```md
## Release Scope (Normative)
- v0.1 (MVP): host only, Claude/Codex, list panes/watch/attach, JSON for panes
- v0.2: multi-target read-only aggregation + Gemini
- v0.3: send/view-output/kill with guard checks
- v0.4: dynamic target management + adapter registry hardening
```

```go
// safety-first state resolution
func ResolveState(targetHealth Health, fresh []Signal) State {
    if targetHealth != HealthOK {
        return State{Value: Unknown, ReasonCode: "target_unreachable", Confidence: "low"}
    }
    if len(fresh) == 0 {
        return State{Value: Unknown, ReasonCode: "stale_signal", Confidence: "low"}
    }
    return HighestPrecedence(fresh)
}
```

```sql
-- operational safety defaults
CREATE INDEX IF NOT EXISTS idx_events_ingested_at ON events(ingested_at);
DELETE FROM events WHERE ingested_at < datetime('now', '-7 day');
-- store ssh alias only, not secrets/json credentials
```