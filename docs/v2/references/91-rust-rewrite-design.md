# AGTMUX v2 — Full Rust Rewrite Design Document

## Context

AGTMUX v1（Go daemon + Swift/SwiftUI app）は、tmux対応ターミナルマルチプレクサ＋AIエージェント統合というコアコンセプトを検証するPOCとして機能した。27フェーズの反復開発を通じて、根本的なアーキテクチャ上の制約が明らかになった：

1. **2プロセスIPC遅延** — Go daemon ↔ Swift app のUnixドメインソケット通信がフレームあたり2-5msの遅延を追加
2. **SwiftUIレンダリングボトルネック** — `@Published outputPreview: String` が全VTフレームでbody全体の再評価をトリガー
3. **差分追跡なし** — SwiftTermに変更追跡機能がなく、毎出力でバッファ全体を再送する必要
4. **スクロール/リセットバグ** — スクロールバック復帰時にforce-resetがターミナル状態を破壊
5. **スレッドホップオーバーヘッド** — MainActorアイソレーションがI/O→レンダー間で不要なディスパッチを発生

v2ではさらに以下の機能を追加する：

6. **tmux window表示** — 単一ペインだけでなく、tmux windowの複数ペインスプリットレイアウトを表示
7. **右クリックコンテキストメニュー** — サイドバーのペインを右クリックで「ペインとして開く」「tmux windowとして開く」
8. **DnDレイアウト変更** — アプリ内でペインをドラッグ&ドロップして自由にレイアウトを変更

---

## 1. コア技術スタック

| レイヤー | クレート | 役割 |
|---------|--------|------|
| VTエミュレーション | `wezterm-term` (git dep, pinned rev) | ヘッドレスターミナル + `get_changed_since(seqno)` 差分追跡 |
| ターミナル型定義 | `termwiz` | Cell, Color, エスケープシーケンス型 |
| GUIフレームワーク | `egui` + `eframe` | 即時モードGUI、ネイティブウィンドウ |
| タイリングレイアウト | `egui_tiles` (Rerun製) | DnD対応スプリットレイアウト、リサイズ可能ボーダー |
| GPUレンダリング | `egui-wgpu` | PaintCallback経由でカスタムwgpuレンダーパス |
| フォント/シェーピング | `cosmic-text` | フォント検出、CJKシェーピング、グリフラスタライズ |
| 非同期ランタイム | `tokio` (multi-thread) | FIFOリード、サブプロセス管理、タイマー |
| tmux I/O | `nix` + `tokio::process` | mkfifo, pipe-pane, コントロールモード |
| 永続化 | `rusqlite` (bundled) | セッションメタデータ、ペイン状態、設定 |

`wezterm-term` はcrates.io非公開 — gitの特定コミットにピン止め：
```toml
[dependencies.wezterm-term]
git = "https://github.com/wezterm/wezterm"
rev = "<pinned-commit>"
```

### iced から egui に変更した理由

| 要件 | iced | egui |
|------|------|------|
| DnDタイリングレイアウト | 自前実装が必要 | **egui_tiles** が完全対応 |
| 右クリックメニュー | 自前オーバーレイ実装 | `response.context_menu()` 1行 |
| リサイズ可能スプリット | 自前実装 | egui_tiles内蔵 |
| カスタムGPUレンダー | widget::shader | **PaintCallback** + wgpu |
| エコシステム | 成長中 | **最大** (25k stars, 500+ contributors) |

---

## 2. プロジェクト構造

```
agtmux/
  Cargo.toml                    # ワークスペースルート
  crates/
    agtmux-tmux/                # Tmux統合レイヤー
      src/
        lib.rs
        pipe_pane.rs            # FIFOストリーミング（pane_tap.goの移植）
        control_bridge.rs       # tmux -C パーサー（tmux_control_bridge.goの移植）
        command.rs              # tmux CLIラッパー
        discovery.rs            # セッション/ウィンドウ/ペイン列挙
        layout_parser.rs        # tmuxレイアウト文字列の再帰パーサー【新規】
        types.rs                # PaneRef, SessionInfo, LayoutGeometry, LayoutTree
    agtmux-term/                # ターミナルエミュレーションラッパー
      src/
        lib.rs
        pane_terminal.rs        # ペイン別 wezterm-term::Terminal + 差分追跡
        snapshot.rs             # バックグラウンドペイン capture-pane 処理
        scrollback.rs           # スクロールバックバッファポリシー
    agtmux-render/              # GPUターミナルレンダラー
      src/
        lib.rs
        pipeline.rs             # wgpu レンダーパイプライン + 頂点/フラグメントシェーダー
        glyph_atlas.rs          # フォントラスタライズ + テクスチャアトラス
        cell_renderer.rs        # Cell → quad マッピング
        cursor.rs               # カーソル形状/点滅
        selection.rs            # 選択ハイライト
        font.rs                 # フォント検出 + メトリクス
        callback.rs             # egui PaintCallback + CallbackTrait 実装【新規】
    agtmux-store/               # 永続化
      src/
        lib.rs
        schema.rs               # SQLiteマイグレーション（migrations.goの移植）
        queries.rs              # CRUD操作
        types.rs                # ドメイン型
    agtmux-app/                 # 単一バイナリエントリーポイント
      src/
        main.rs                 # eframe::run_native + tokioランタイム起動
        app.rs                  # eframe::App 実装、トップレベルUI
        sidebar.rs              # セッション/ウィンドウ/ペインツリー + コンテキストメニュー【新規】
        tile_behavior.rs        # egui_tiles::Behavior 実装（DnD + ペインレンダリング）【新規】
        window_view.rs          # tmux window のタイルツリー構築【新規】
        input.rs                # キーボード/IME/ペーストパイプライン
        theme.rs                # カラースキーム、ANSIパレット
        config.rs               # TOML設定読み込み
      shaders/
        terminal.wgsl           # WGSL 頂点+フラグメントシェーダー
```

クレート依存グラフ：
```
agtmux-app → agtmux-tmux, agtmux-term, agtmux-render, agtmux-store
           → egui, eframe, egui_tiles, egui-wgpu, tokio
agtmux-term → wezterm-term, termwiz
agtmux-render → wgpu (egui-wgpu経由), cosmic-text, wezterm-term (Line/Cell型)
agtmux-tmux → tokio, nix
agtmux-store → rusqlite
```

---

## 3. Tmux統合レイヤー (`agtmux-tmux`)

### 3.1 Pipe-Pane FIFOストリーミング — `pipe_pane.rs`

`internal/daemon/pane_tap.go` の直接移植。

```rust
pub struct PipePaneHandle {
    pane_id: String,
    fifo_path: PathBuf,           // /tmp/agtmux-pane-tap/pane-tap-<pid>-<nanos>.fifo
    cancel: CancellationToken,
}

pub struct PipePaneEvent {
    pub pane_id: String,
    pub bytes: Vec<u8>,           // 生VTバイト、16KBチャンク
}
```

設計判断（POCから引き継ぎ）：
- FIFOパス：`PID + nanotime` でユニーク性確保
- イベントチャネルバッファ：**512**
- リードバッファ：**16KB**
- `O_RDWR` オープン：writer不在時のブロック回避
- リードループは `tokio::task::spawn_blocking` で実行
- クリーンアップ：`tmux pipe-pane -t <pane_id>`（`-O`なし）でデタッチ後、`unlink`

### 3.2 コントロールブリッジ — `control_bridge.rs`

`internal/daemon/tmux_control_bridge.go` の直接移植。

```rust
pub enum ControlEvent {
    Output { pane_id: String, bytes: Vec<u8> },
    ExtendedOutput { pane_id: String, bytes: Vec<u8> },
    LayoutChange { window_id: String, layout_raw: String, cols: u16, rows: u16 },
    SessionChanged { session_id: String, session_name: String },
    WindowAdd { window_id: String },
    Exit,
}
```

**変更点**: `LayoutChange` に `layout_raw` を追加。ネスト構造パースに必要。

### 3.3 tmuxレイアウト文字列パーサー — `layout_parser.rs` 【新規】

tmuxのレイアウト文字列はペインのスプリットツリーを再帰的に記述する：

```
# 単一ペイン
checksum,120x40,0,0,%1

# 水平分割（{} = 左右に分割）
checksum,120x40,0,0{60x40,0,0,%1,60x40,61,0,%2}

# 垂直分割（[] = 上下に分割）
checksum,120x40,0,0[120x20,0,0,%1,120x20,0,21,%2]

# ネスト
checksum,120x40,0,0{60x40,0,0,%1,60x40,61,0[30x20,61,0,%2,30x20,61,21,%3]}
```

```rust
/// tmuxレイアウトツリーのノード。
#[derive(Debug, Clone)]
pub enum LayoutNode {
    /// 単一ペイン。
    Pane {
        pane_id: String,       // e.g., "%1"
        cols: u16,
        rows: u16,
        x_offset: u16,
        y_offset: u16,
    },
    /// 水平分割（左右）。tmuxの `{...}` に対応。
    HSplit {
        cols: u16,
        rows: u16,
        children: Vec<LayoutNode>,
    },
    /// 垂直分割（上下）。tmuxの `[...]` に対応。
    VSplit {
        cols: u16,
        rows: u16,
        children: Vec<LayoutNode>,
    },
}

/// tmuxレイアウト文字列をパースしてツリーを返す。
/// 入力例: "abc1,120x40,0,0{60x40,0,0,%1,60x40,61,0,%2}"
pub fn parse_tmux_layout(raw: &str) -> Result<LayoutNode> {
    // 1. checksumをスキップ（最初のカンマまで）
    // 2. WxH,x,y を読む
    // 3. 次の文字が '{' なら HSplit を再帰パース
    //    次の文字が '[' なら VSplit を再帰パース
    //    次の文字が ',' なら PaneID を読む（単一ペイン）
    // 4. ネストされた子ノードを再帰的にパース
}

/// LayoutNode を egui_tiles::Tree<TerminalPane> に変換。
pub fn layout_to_tiles(
    node: &LayoutNode,
    tiles: &mut egui_tiles::Tiles<TerminalPane>,
) -> egui_tiles::TileId {
    match node {
        LayoutNode::Pane { pane_id, cols, rows, .. } => {
            tiles.insert_pane(TerminalPane::new(pane_id, *cols, *rows))
        }
        LayoutNode::HSplit { children, .. } => {
            let child_ids: Vec<_> = children.iter()
                .map(|c| layout_to_tiles(c, tiles))
                .collect();
            tiles.insert_horizontal_tile(child_ids)
        }
        LayoutNode::VSplit { children, .. } => {
            let child_ids: Vec<_> = children.iter()
                .map(|c| layout_to_tiles(c, tiles))
                .collect();
            tiles.insert_vertical_tile(child_ids)
        }
    }
}
```

### 3.4 コマンド実行 — `command.rs`

```rust
pub struct TmuxCommand { target_kind: TargetKind }

impl TmuxCommand {
    pub async fn send_keys(&self, pane_id: &str, bytes: &[u8]) -> Result<()>;
    pub async fn resize_pane(&self, pane_id: &str, cols: u16, rows: u16) -> Result<()>;
    pub async fn capture_pane(&self, pane_id: &str) -> Result<String>;
}
```

### 3.5 ディスカバリー — `discovery.rs`

```rust
pub struct TmuxDiscovery;

impl TmuxDiscovery {
    pub async fn list_sessions(cmd: &TmuxCommand) -> Result<Vec<SessionInfo>>;
    pub async fn list_panes(cmd: &TmuxCommand) -> Result<Vec<PaneInfo>>;
    pub async fn poll_pane_metadata(cmd: &TmuxCommand) -> Result<Vec<PaneInfo>>;
}
```

定期ポーリング（2-4秒、適応的バックオフ）。

---

## 4. ターミナルエミュレーションレイヤー (`agtmux-term`)

### 4.1 ペイン別ターミナル — `pane_terminal.rs`

各tmuxペインに独自の `wezterm_term::Terminal` インスタンスを割り当て。

```rust
pub struct PaneTerminal {
    terminal: Terminal,
    last_rendered_seqno: SequenceNo,
    size: TerminalSize,
    pane_id: String,
}

impl PaneTerminal {
    pub fn new(pane_id: &str, cols: u16, rows: u16) -> Self;
    pub fn advance_bytes(&mut self, bytes: &[u8]);
    pub fn get_changed_lines(&mut self) -> Vec<(usize, &Line)>;
    pub fn cursor_pos(&self) -> (usize, usize);
    pub fn resize(&mut self, cols: u16, rows: u16);
    pub fn apply_snapshot(&mut self, ansi_content: &str);
    pub fn get_scrollback_range(&self, start: StableRowIndex, count: usize) -> Vec<&Line>;
}
```

### 4.2 差分追跡フロー

1. `wezterm-term` が全行変異に `SequenceNo` を割り当て
2. 各 `Line` が `last_change_seqno` を保持
3. 各レンダーパス後に `last_rendered_seqno` を保存
4. 次レンダー: `line.changed_since(last_rendered_seqno)` → ダーティ行のみ
5. ダーティ行の頂点データのみGPUにアップロード

### 4.3 マルチペイン出力管理【新規】

tmux window表示では複数ペインが同時にアクティブになる。出力ソースの優先度：

```
Window内のフォーカスペイン: pipe-pane（直接FIFO）
Window内の非フォーカスペイン: bridge output（%output / %extended-output）
完全バックグラウンドペイン: capture-pane snapshot（350msコアレシング）
```

```rust
/// 全ペインターミナルを管理。
pub struct TerminalManager {
    /// pane_id → PaneTerminal のマップ
    terminals: HashMap<String, PaneTerminal>,
    /// 現在pipe-paneが接続されているペインID
    pipe_pane_target: Option<String>,
}

impl TerminalManager {
    /// バイトを適切なPaneTerminalにルーティング。
    pub fn feed_bytes(&mut self, pane_id: &str, bytes: &[u8]);

    /// 全可視ペインのダーティ状態を返す。
    pub fn get_all_dirty(&self) -> Vec<(&str, Vec<(usize, &Line)>)>;

    /// ペインが存在しなければ作成。
    pub fn ensure_terminal(&mut self, pane_id: &str, cols: u16, rows: u16);
}
```

---

## 5. レンダリングパイプライン (`agtmux-render`)

### 5.1 egui PaintCallback アーキテクチャ

iced の `widget::shader` の代わりに、egui の `PaintCallback` + `CallbackTrait` を使用。各ペインタイル内でカスタムwgpuレンダーパスを実行。

```rust
/// ターミナルペインのGPUレンダリングコールバック。
/// egui_tiles の pane_ui() 内で生成され、各タイルの矩形にGPU描画。
pub struct TerminalRenderCallback {
    pub pane_id: String,
    pub visible_lines: Arc<Vec<RenderedLine>>,
    pub cursor: CursorState,
    pub selection: Option<SelectionRange>,
    pub force_redraw: bool,
}

impl egui_wgpu::CallbackTrait for TerminalRenderCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let resources: &mut TerminalGpuResources = callback_resources.get_mut().unwrap();

        // 1. 新規グリフが必要な場合アトラスを更新
        resources.glyph_atlas.ensure_glyphs(device, queue, &self.visible_lines);

        // 2. ダーティ行の頂点データのみ再構築（force_redraw時は全行）
        if self.force_redraw {
            resources.rebuild_all_vertices(device, queue, &self.visible_lines);
        } else {
            resources.update_dirty_vertices(device, queue, &self.visible_lines);
        }

        // 3. カーソル/選択ユニフォームを更新
        resources.update_cursor(queue, &self.cursor);

        Vec::new()
    }

    fn paint(
        &self,
        info: PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        let resources: &TerminalGpuResources = callback_resources.get().unwrap();

        // クリップ矩形を設定（このタイルの領域のみ描画）
        let rect = info.viewport_in_pixels();
        render_pass.set_scissor_rect(rect.x(), rect.y(), rect.width(), rect.height());

        // 1. セル背景をレンダー
        resources.render_backgrounds(render_pass);
        // 2. 選択ハイライト
        resources.render_selection(render_pass);
        // 3. グリフ前景（アトラスからのテクスチャ付きquad）
        resources.render_glyphs(render_pass);
        // 4. カーソル
        resources.render_cursor(render_pass);
    }
}

/// eframe初期化時にGPUリソースを登録。
pub struct TerminalGpuResources {
    pub pipeline: wgpu::RenderPipeline,
    pub glyph_atlas: GlyphAtlas,
    pub vertex_buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
    pub viewport_uniform: wgpu::Buffer,
    // ... per-pane vertex caches
}
```

### 5.2 GPUリソース初期化

```rust
// main.rs の CreationContext 内で初期化
fn setup_gpu(cc: &eframe::CreationContext) {
    let wgpu_state = cc.wgpu_render_state.as_ref().unwrap();
    let device = &wgpu_state.device;

    let resources = TerminalGpuResources::new(device);

    // CallbackResources に登録 → paint() でアクセス可能
    wgpu_state.renderer.write().callback_resources.insert(resources);
}
```

### 5.3 グリフアトラス — `glyph_atlas.rs`

```rust
pub struct GlyphAtlas {
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    glyph_cache: HashMap<GlyphKey, AtlasRegion>,
    font_system: FontSystem,  // cosmic-text
    packer: AtlasPacker,
    atlas_size: (u32, u32),   // 初期 2048x2048
}
```

### 5.4 WGSL シェーダー — `terminal.wgsl`

```wgsl
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) fg_color: vec4<f32>,
    @location(3) bg_color: vec4<f32>,
    @location(4) attributes: u32,
}

@group(0) @binding(0) var glyph_atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;
@group(0) @binding(2) var<uniform> viewport: ViewportUniform;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(
        (in.position.x / viewport.width) * 2.0 - 1.0,
        1.0 - (in.position.y / viewport.height) * 2.0,
        0.0, 1.0
    );
    out.tex_coords = in.tex_coords;
    out.fg_color = in.fg_color;
    out.bg_color = in.bg_color;
    out.attributes = in.attributes;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let glyph_alpha = textureSample(glyph_atlas, atlas_sampler, in.tex_coords).r;
    return mix(in.bg_color, in.fg_color, glyph_alpha);
}
```

### 5.5 フォント処理

```rust
const PREFERRED_FONTS: &[&str] = &[
    "JetBrains Mono NL Nerd Font Mono",
    "JetBrains Mono Nerd Font Mono",
    "UDEV Gothic 35JPDOC",
    "Hack Nerd Font Mono",
    "MesloLGS NF",
];
const FALLBACK_FONT: &str = "Menlo";
const DEFAULT_FONT_SIZE: f32 = 13.0;
```

---

## 6. egui_tiles タイリングレイアウト【新規セクション】

### 6.1 概念マッピング

```
tmux Session  →  サイドバーのツリーノード
tmux Window   →  egui_tiles::Tree<TerminalPane>  ← DnDスプリットレイアウト
tmux Pane     →  egui_tiles タイル内の TerminalPane
```

各tmux Windowが1つの `egui_tiles::Tree` を持ち、そのWindow内のペインの分割レイアウトを管理する。ユーザーはDnDでレイアウトを自由に変更可能。

### 6.2 データモデル

```rust
/// 1つのターミナルペイン（egui_tilesのペイン型）。
#[derive(Debug)]
pub struct TerminalPane {
    pub pane_id: String,       // tmux pane ID, e.g., "%1"
    pub title: String,         // pane title or current command
    pub cols: u16,
    pub rows: u16,
    pub focused: bool,         // このペインにキーボードフォーカスがあるか
}

/// tmux Window → egui_tiles Tree のマッピング。
pub struct WindowView {
    pub window_id: String,
    pub window_name: String,
    pub tree: egui_tiles::Tree<TerminalPane>,
}

/// tmux Session。
pub struct SessionView {
    pub session_name: String,
    pub windows: Vec<WindowView>,
}

/// アプリ全体のビューモデル。
pub struct AppViews {
    pub sessions: Vec<SessionView>,
    pub active_session: usize,
    pub active_window: usize,
    /// 単一ペイン表示モード（右クリック→"ペインとして開く"時）
    pub single_pane_mode: Option<String>,  // Some(pane_id) if viewing single pane
}
```

### 6.3 Behavior 実装 — `tile_behavior.rs`

```rust
pub struct TerminalTileBehavior<'a> {
    /// 全ペインターミナルへのアクセス
    terminal_manager: &'a mut TerminalManager,
    /// GPU描画用の共有状態
    gpu_resources_ready: bool,
    /// フォーカスされたペインID
    focused_pane: Option<String>,
    /// コンテキストメニューアクション
    pending_action: &'a mut Option<TileAction>,
}

pub enum TileAction {
    FocusPane(String),
    DetachPane(String),
    OpenPaneInNewWindow(String),
}

impl<'a> egui_tiles::Behavior<TerminalPane> for TerminalTileBehavior<'a> {
    /// タイルのタブタイトル。
    fn tab_title_for_pane(&mut self, pane: &TerminalPane) -> egui::WidgetText {
        format!("{} ({})", pane.title, pane.pane_id).into()
    }

    /// 各タイル内のレンダリング — ここでGPUターミナル描画。
    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        tile_id: egui_tiles::TileId,
        pane: &mut TerminalPane,
    ) -> egui_tiles::UiResponse {
        // タイル領域全体を確保
        let (rect, response) = ui.allocate_exact_size(
            ui.available_size(),
            egui::Sense::click_and_drag(),
        );

        // クリックでフォーカス切り替え
        if response.clicked() {
            self.focused_pane = Some(pane.pane_id.clone());
            pane.focused = true;
        }

        // 右クリックメニュー（タイル内）
        response.context_menu(|ui| {
            if ui.button("Detach Pane").clicked() {
                *self.pending_action = Some(TileAction::DetachPane(pane.pane_id.clone()));
                ui.close_menu();
            }
            if ui.button("Open in New Window").clicked() {
                *self.pending_action = Some(TileAction::OpenPaneInNewWindow(pane.pane_id.clone()));
                ui.close_menu();
            }
        });

        // PaintCallback でGPUターミナル描画
        if let Some(term) = self.terminal_manager.terminals.get(&pane.pane_id) {
            let callback = TerminalRenderCallback {
                pane_id: pane.pane_id.clone(),
                visible_lines: Arc::new(/* render lines from term */),
                cursor: cursor_state(term.cursor_pos()),
                selection: None,
                force_redraw: false,
            };
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, callback));
        }

        // DnDドラッグ開始
        if response.drag_started() {
            return egui_tiles::UiResponse::DragStarted;
        }

        egui_tiles::UiResponse::None
    }
}
```

### 6.4 tmux Window → egui_tiles Tree 変換

```rust
/// %layout-change イベント受信時に呼び出し。
/// tmuxのレイアウト文字列をパースして egui_tiles::Tree に変換。
pub fn build_window_tree(layout_raw: &str) -> Result<egui_tiles::Tree<TerminalPane>> {
    let layout_node = parse_tmux_layout(layout_raw)?;
    let mut tiles = egui_tiles::Tiles::default();
    let root = layout_to_tiles(&layout_node, &mut tiles);
    Ok(egui_tiles::Tree::new("window", root, tiles))
}

/// ユーザーがDnDでレイアウトを変更した場合、
/// tmux側にも変更を反映するオプション。
pub async fn sync_layout_to_tmux(
    tree: &egui_tiles::Tree<TerminalPane>,
    tmux_cmd: &TmuxCommand,
    window_id: &str,
) -> Result<()> {
    // tree の構造を tmux の move-pane / join-pane コマンドに変換
    // （オプション機能: 最初はアプリ内レイアウトのみ変更、tmux同期は後で実装）
}
```

---

## 7. サイドバー + コンテキストメニュー【新規セクション】

### 7.1 サイドバーツリー — `sidebar.rs`

```rust
pub fn render_sidebar(
    ctx: &egui::Context,
    views: &mut AppViews,
    terminal_manager: &TerminalManager,
) {
    egui::SidePanel::left("session_tree")
        .resizable(true)
        .default_width(220.0)
        .width_range(180.0..=320.0)
        .show(ctx, |ui| {
            ui.heading("Sessions");
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (s_idx, session) in views.sessions.iter().enumerate() {
                    // セッションノード
                    let s_response = egui::CollapsingHeader::new(&session.session_name)
                        .id_salt(s_idx)
                        .default_open(true)
                        .show(ui, |ui| {
                            for (w_idx, window) in session.windows.iter().enumerate() {
                                // ウィンドウノード
                                let w_response = egui::CollapsingHeader::new(
                                    format!("{} ({})", window.window_name, window.window_id)
                                )
                                    .id_salt((s_idx, w_idx))
                                    .show(ui, |ui| {
                                        // 各ペイン
                                        for tile_id in window.tree.active_tiles() {
                                            if let Some(pane) = window.tree.tiles.get_pane(&tile_id) {
                                                let is_active =
                                                    views.active_session == s_idx &&
                                                    views.active_window == w_idx &&
                                                    views.single_pane_mode.as_deref() == Some(&pane.pane_id);
                                                let pane_response = ui.selectable_label(
                                                    is_active,
                                                    &pane.title,
                                                );

                                                // ★ 右クリックメニュー
                                                render_pane_context_menu(
                                                    &pane_response, views,
                                                    s_idx, w_idx, &pane.pane_id,
                                                );

                                                // 左クリック → 単一ペインとして開く
                                                if pane_response.clicked() {
                                                    views.active_session = s_idx;
                                                    views.active_window = w_idx;
                                                    views.single_pane_mode = Some(pane.pane_id.clone());
                                                }
                                            }
                                        }
                                    });

                                // ★ ウィンドウノードの右クリックメニュー
                                if let Some(inner) = w_response {
                                    inner.header_response.context_menu(|ui| {
                                        if ui.button("Open as tmux Window").clicked() {
                                            views.active_session = s_idx;
                                            views.active_window = w_idx;
                                            views.single_pane_mode = None; // Window全体表示
                                            ui.close_menu();
                                        }
                                    });
                                }
                            }
                        });
                }
            });
        });
}
```

### 7.2 ペインコンテキストメニュー

```rust
fn render_pane_context_menu(
    response: &egui::Response,
    views: &mut AppViews,
    session_idx: usize,
    window_idx: usize,
    pane_id: &str,
) {
    response.context_menu(|ui| {
        // ペインとして開く（単一ペインフルスクリーン表示）
        if ui.button("Open as Pane").clicked() {
            views.active_session = session_idx;
            views.active_window = window_idx;
            views.single_pane_mode = Some(pane_id.to_string());
            ui.close_menu();
        }

        // tmux Windowとして開く（スプリットレイアウト表示）
        if ui.button("Open as tmux Window").clicked() {
            views.active_session = session_idx;
            views.active_window = window_idx;
            views.single_pane_mode = None;
            ui.close_menu();
        }

        ui.separator();

        // サブメニュー
        ui.menu_button("Move to...", |ui| {
            for (idx, session) in views.sessions.iter().enumerate() {
                for (w_idx, window) in session.windows.iter().enumerate() {
                    if ui.button(format!("{}/{}", session.session_name, window.window_name)).clicked() {
                        // tmux move-pane コマンドを発行
                        ui.close_menu();
                    }
                }
            }
        });
    });
}
```

---

## 8. 状態管理 — egui 即時モード

### 8.1 アプリケーション状態

```rust
pub struct App {
    // --- ビューモデル ---
    views: AppViews,                    // セッション/ウィンドウ/ペインの階層
    terminal_manager: TerminalManager,  // 全PaneTerminalの管理

    // --- Tmux I/O ---
    pipe_pane: Option<PipePaneHandle>,
    control_bridge: Option<ControlBridge>,
    tmux_cmd: TmuxCommand,

    // --- 非同期チャネル（tokio → egui） ---
    pipe_pane_rx: mpsc::UnboundedReceiver<PipePaneEvent>,
    bridge_rx: mpsc::UnboundedReceiver<ControlEvent>,
    bg_capture_rx: mpsc::UnboundedReceiver<(String, String)>,  // (pane_id, content)
    discovery_rx: mpsc::UnboundedReceiver<Vec<PaneInfo>>,

    /// コマンド送信（egui → tokio）
    cmd_tx: mpsc::UnboundedSender<AsyncCommand>,

    // --- UI ---
    theme: Theme,
    config: AppConfig,
    store: Store,
    pending_tile_action: Option<TileAction>,
}
```

### 8.2 eframe::App 実装

```rust
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ========================================
        // Phase 1: 非同期イベントの処理（非ブロッキング）
        // ========================================

        // pipe-pane バイト処理（ホットパス）
        while let Ok(event) = self.pipe_pane_rx.try_recv() {
            self.terminal_manager.feed_bytes(&event.pane_id, &event.bytes);
        }

        // コントロールブリッジイベント処理
        while let Ok(event) = self.bridge_rx.try_recv() {
            match event {
                ControlEvent::Output { pane_id, bytes } => {
                    // pipe-paneが接続されていないペインのみ
                    if self.terminal_manager.pipe_pane_target.as_deref() != Some(&pane_id) {
                        self.terminal_manager.feed_bytes(&pane_id, &bytes);
                    }
                }
                ControlEvent::LayoutChange { window_id, layout_raw, cols, rows } => {
                    self.handle_layout_change(&window_id, &layout_raw, cols, rows);
                }
                _ => {}
            }
        }

        // バックグラウンドキャプチャ処理
        while let Ok((pane_id, content)) = self.bg_capture_rx.try_recv() {
            self.terminal_manager.apply_snapshot(&pane_id, &content);
        }

        // ディスカバリー更新
        while let Ok(panes) = self.discovery_rx.try_recv() {
            self.update_views_from_discovery(panes);
        }

        // ========================================
        // Phase 2: UI描画
        // ========================================

        // サイドバー
        render_sidebar(ctx, &mut self.views, &self.terminal_manager);

        // メインパネル
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(ref pane_id) = self.views.single_pane_mode {
                // 単一ペイン表示モード
                self.render_single_pane(ui, pane_id);
            } else {
                // tmux Window表示モード（egui_tiles スプリットレイアウト）
                self.render_window_tiles(ui);
            }
        });

        // ========================================
        // Phase 3: 遅延アクション処理
        // ========================================
        if let Some(action) = self.pending_tile_action.take() {
            self.handle_tile_action(action);
        }

        // データがある場合は再描画を要求
        ctx.request_repaint();
    }
}
```

### 8.3 Window表示 vs 単一ペイン表示

```rust
impl App {
    /// tmux Windowのスプリットレイアウトを表示。
    fn render_window_tiles(&mut self, ui: &mut egui::Ui) {
        let session = &mut self.views.sessions[self.views.active_session];
        let window = &mut session.windows[self.views.active_window];

        let mut behavior = TerminalTileBehavior {
            terminal_manager: &mut self.terminal_manager,
            gpu_resources_ready: true,
            focused_pane: self.terminal_manager.pipe_pane_target.clone(),
            pending_action: &mut self.pending_tile_action,
        };

        // egui_tiles がスプリットレイアウト + DnD + リサイズを全て処理
        window.tree.ui(&mut behavior, ui);
    }

    /// 単一ペインをフルスクリーンで表示。
    fn render_single_pane(&mut self, ui: &mut egui::Ui, pane_id: &str) {
        let (rect, response) = ui.allocate_exact_size(
            ui.available_size(),
            egui::Sense::click_and_drag(),
        );

        if let Some(term) = self.terminal_manager.terminals.get(pane_id) {
            let callback = TerminalRenderCallback {
                pane_id: pane_id.to_string(),
                visible_lines: Arc::new(/* ... */),
                cursor: cursor_state(term.cursor_pos()),
                selection: None,
                force_redraw: false,
            };
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, callback));
        }
    }
}
```

---

## 9. データフロー

### 9.1 フォーカスペイン

```
tmux pipe-pane → FIFO → tokio spawn_blocking → mpsc(unbounded)
  → app.update() { pipe_pane_rx.try_recv() }
  → terminal_manager.feed_bytes() → PaneTerminal::advance_bytes()
  → PaintCallback::prepare() → ダーティ行の頂点データ更新
  → PaintCallback::paint() → GPU render
```

### 9.2 Window内の非フォーカスペイン

```
tmux -C attach → ControlBridge → bridge_rx → ControlEvent::Output
  → terminal_manager.feed_bytes() （pipe-pane非接続ペインのみ）
  → PaintCallback → GPU render
```

### 9.3 ユーザー入力

```
egui keyboard event → tmux send-keys → pipe-pane がエコーをキャプチャ → render
```
ローカルエコーなし（Phase 27の教訓）。

### 9.4 DnDレイアウト変更

```
egui_tiles DnD → Tree<TerminalPane> の構造変更（自動）
  → 各タイルの矩形が再計算
  → PaneTerminal::resize() → tmux resize-pane
  → 次フレームで新レイアウトで描画
```

---

## 10. 並行モデル

### 10.1 eframe + tokio 統合

```rust
fn main() {
    // チャネル作成
    let (pipe_pane_tx, pipe_pane_rx) = mpsc::unbounded_channel();
    let (bridge_tx, bridge_rx) = mpsc::unbounded_channel();
    let (bg_capture_tx, bg_capture_rx) = mpsc::unbounded_channel();
    let (discovery_tx, discovery_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

    // tokio を別スレッドで起動
    let rt = tokio::runtime::Runtime::new().unwrap();
    std::thread::spawn(move || {
        rt.block_on(async_main(pipe_pane_tx, bridge_tx, bg_capture_tx, discovery_tx, cmd_rx));
    });

    // eframe をメインスレッドで起動
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("AGTMUX", options, Box::new(move |cc| {
        setup_gpu(cc);
        Ok(Box::new(App::new(pipe_pane_rx, bridge_rx, bg_capture_rx, discovery_rx, cmd_tx)))
    })).unwrap();
}
```

### 10.2 効率的な再描画

```rust
// tokio側: データ送信後にegui再描画をトリガー
async fn pipe_pane_read_loop(
    tx: mpsc::UnboundedSender<PipePaneEvent>,
    egui_ctx: egui::Context,  // Clone可能
) {
    loop {
        let bytes = read_fifo().await;
        tx.send(PipePaneEvent { pane_id, bytes }).ok();
        egui_ctx.request_repaint();  // UI スレッドを起こす
    }
}
```

アイドル時は `request_repaint()` が呼ばれるまでスリープ → CPU使用率が低下。

### 10.3 スレッド

| スレッド | 役割 |
|---------|------|
| メイン | eframe イベントループ、egui update()、wgpuレンダー |
| tokio worker 1 | pipe-pane FIFOリード (spawn_blocking) |
| tokio worker 2 | コントロールブリッジ stdout パース |
| tokio worker 3 | tmux コマンド実行 (send-keys, resize) |
| tokio worker 4 | バックグラウンドキャプチャ + ディスカバリー |

---

## 11. 永続化 (`agtmux-store`)

### SQLite スキーマ

POC の `internal/db/migrations.go`（v1-v5）から移植：

```sql
CREATE TABLE targets (target_id, target_name, kind, connection_ref, is_default, ...);
CREATE TABLE panes (target_id, pane_id, session_name, window_id, current_cmd, ...);
CREATE TABLE runtimes (...);
CREATE TABLE states (...);
```

`rusqlite` + `bundled` + `PRAGMA journal_mode=WAL`。

### 設定ファイル

```toml
# ~/.config/agtmux/config.toml
[terminal]
font_family = "JetBrains Mono NL Nerd Font Mono"
font_size = 13.0
scrollback_lines = 10000

[tmux]
pipe_pane_enabled = true
background_capture_interval_ms = 250
```

---

## 12. macOS 統合

- **ウィンドウ**: eframe ネイティブデコレーション
- **キーボード**: Cmd+C/V (コピー/ペースト), Cmd+K (クリア), Cmd+1-9 (ペイン切り替え)
- **IME**: winit の `Ime::Preedit`/`Ime::Commit`。egui の IME サポートは改善中
- **メニューバー**: eframe の `set_menus()` API

---

## 13. リライトが排除するもの

| POCの問題 | 根本原因 | v2の解決 |
|----------|---------|---------|
| IPC遅延 (2-5ms/フレーム) | 2プロセス Go+Swift | 単一Rustバイナリ |
| SwiftUI body再評価 | `@Published String` | egui即時モード（差分なし、直接描画） |
| 差分追跡なし | SwiftTerm の制約 | wezterm-term `get_changed_since(seqno)` |
| スクロールリセットバグ | `resetToInitialState()` | wezterm-term スクロールバック保持 |
| スレッドホップ | MainActor | メインスレッドで直接状態変更 |
| 単一ペイン表示のみ | SwiftUI制約 | **egui_tiles でマルチペインDnDレイアウト** |

---

## 14. 段階的実装計画

| フェーズ | 範囲 | 期間 | 主要成果物 |
|---------|------|------|----------|
| 1 | Cargoワークスペース + tmuxレイヤー | Week 1-2 | CLIツール: pipe-pane → stdout |
| 2 | ターミナルエミュレーション + レイアウトパーサー | Week 3 | PaneTerminal + tmuxレイアウトパーサー + テスト |
| 3 | GPUレンダリング + PaintCallback | Week 4-5 | グリフアトラス + WGSLシェーダー + 単一ペイン描画 |
| 4 | egui_tiles + サイドバー + コンテキストメニュー | Week 6-7 | **DnDレイアウト、Window表示、右クリックメニュー** |
| 5 | マルチペインストリーミング + 永続化 | Week 8-9 | Window内の複数ペイン同時ストリーム、SQLite |
| 6 | ポリッシュ + macOS統合 | Week 10-11 | IME、選択、クリップボード、テーマ、.appバンドル |

---

## 15. リスク評価

| リスク | 緩和策 |
|-------|-------|
| wezterm-term API不安定 | gitコミットピン止め + PaneTerminal抽象で隔離 |
| egui PaintCallback の制約 | Phase 3で早期プロトタイプ。問題あれば中間テクスチャ方式にフォールバック |
| egui IMEサポート | 基本キーボードから開始、IMEは段階的追加 |
| egui_tiles のDnD制約 | Rerun社が積極メンテ中。問題あればフォークして拡張 |
| 即時モードのCPU使用 | `request_repaint()` ベースのオンデマンド再描画で緩和 |

---

## 16. 検証計画

| フェーズ | テスト方法 |
|---------|----------|
| 1 | `cargo run -p agtmux-tmux --example pipe_test` — FIFO経由で生tmuxバイト確認 |
| 2 | `cargo test -p agtmux-term` — VTシーケンス + レイアウトパース |
| 3 | `cargo run -p agtmux-app` — 単一ペインGPU描画確認 |
| 4 | 手動テスト: ペインDnD、スプリットリサイズ、右クリックメニュー |
| 5 | 手動テスト: tmux window開き、3ペイン同時ストリーム |
| 6 | 手動テスト: IME入力、スクロール、テーマ切り替え |

### パフォーマンス目標

| メトリクス | 目標 | POC現状 |
|----------|------|--------|
| 入力→表示 (p95) | < 16ms | ~50-100ms |
| フレームレート | 60fps 安定 | 不安定 |
| スクロール | ネイティブ品質 | バグ |
| ペイン切り替え | < 100ms | ~200-500ms |
| アイドル時CPU | < 1% | 測定なし |
