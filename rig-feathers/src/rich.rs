//! 富文本：上游的 markdown / 代码高亮 / diff → 我们的行模型（[`Row`]）。
//!
//! 上游这三个模块（`rig_tui::markdown`、`rig_tui::highlight`、`rig_tui::diff`）的
//! 输入输出都是**纯值**：`&str` + `rig_tui::Theme` → `Vec<LookLine>`。所以这里不重写
//! 它们，直接调 `pub` 函数——和 `status.rs` 调 `rig_tui::status::line`、`picker.rs`
//! 调 `rig_tui::wrap::wrap_all` 是同一个范式。真实成本只有两处：
//!
//! 1. **主题**：[`crate::theme::Theme`] 是 10 个前景色 token，上游要的是 17 个
//!    "某种东西的底色" band（`rig_tui::theme::Theme`），两个字段集不相交。这里用
//!    [`upstream_theme`] 把我们的底色当基底、用我们自己的语义前景色去混出 band。
//! 2. **行模型**：上游的 `Line` 有**两级**底色——行级（`.bg()`：diff 的整行红绿、
//!    markdown 的代码块与提示块）和 span 级（改动的词、行内 code）。我们的 [`Row`]
//!    只有整行一个底色 [`Row::band`]，而渲染层只读 `span.style.fg`
//!    （`transcript.rs::spawn_text_line` 连 `Modifier` 都不读）。于是：
//!    - 行级底色 → [`Row::band`]，**无损**；
//!    - span 级底色 → [`expressible`] 里退化成"同色相更亮的前景色 + 下划线/加粗"
//!      （Modifier 现在是死数据，渲染层将来读了才生效）。
//!
//!    想让行内 code、diff 改动词真的带底色，得由 lead 扩行模型/渲染层（见回报）。
//!
//! 三个入口：[`markdown_rows`]、[`highlight_rows`]、[`diff_rows`]（外加一个便利
//! 入口 [`tool_result_rows`]）。宽度都要传进来：折行交给上游
//! [`rig_tui::wrap::wrap_all`]，我们的渲染层不做折行，一行就是一行。

use rig_tui::highlight::{Code, Language};
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::Line;
use rig_tui::{LookLine, Theme as RigTheme};

use crate::theme::Theme;
use crate::transcript::Row;

// ---------------------------------------------------------------------------
// 主题适配
// ---------------------------------------------------------------------------

/// 白（混色的另一头）。
const WHITE: (u8, u8, u8) = (255, 255, 255);
/// 主题 token 没有颜色（`Reset`/解析不出）时的兜底前景：我们默认主题的主文字色。
const FALLBACK_FG: (u8, u8, u8) = (210, 216, 228);
/// [`Theme::band`] 解析不出 RGB 时的兜底底色：我们默认主题的 band。
const FALLBACK_BACKGROUND: (u8, u8, u8) = (20, 26, 38);

/// 上游需要的 `rig_tui::Theme`：从我们的 [`Theme`] 混出来。
///
/// 两套字段不相交，所以分三步：
///
/// - **基底**：把我们唯一的底色 [`Theme::band`] 当成"帧的底色"，交给上游自己的
///   `Theme::on(background, truecolor)`——明暗判断与混合数学因此还是上游的。
/// - **覆写**（我们有同色相前景 token 的 band）：`added`/`added_word`（`ok`）、
///   `removed`/`removed_word`（`err`）、`call_done`（`ok`）、`call_failed`（`err`）、
///   `call_running`（`warn`）、`note`（`assistant`）、`important`（`accent`）、
///   `selected`（`warn`）、`user`（`user`）、`tool`（`fg`）。混合比例照抄上游
///   `theme.rs` 的常量（0.06–0.38），色相用我们的 → diff 的红绿、调用状态的黄绿
///   与 3D 配色一致，而不是上游写死的 hue。
/// - **猜的字段**（我们没有对应 token，保留 `Theme::on` 的默认值）：
///   `panel_surface`、`raised_surface`、`input`、`call_waiting`、`code`
///   （我们只有"代码块底色"的观感、没有 token，`code` 由 band 朝白混 8% 得来）。
///   另外两个也是猜的：`background` 用 [`Theme::band`] 顶替（上游要的是"终端底色"，
///   窗口前端没有这个概念，见 PARITY §4.4）；`plain` 恒 `false`（我们没有
///   `NO_COLOR` 通路）；`truecolor` 恒 `true`（我们的 token 是 RGB/命名色，
///   一旦让它走 256 色板，上面混出来的色相会全被换成写死的索引色）。
pub fn upstream_theme(theme: &Theme) -> RigTheme {
    let background = rgb(theme.band).unwrap_or(FALLBACK_BACKGROUND);
    let mut mapped = RigTheme::on(background, true);
    let fg = |style: Style| style.fg.and_then(rgb).unwrap_or(FALLBACK_FG);
    mapped.user = mix(background, fg(theme.user), 0.12);
    mapped.tool = mix(background, fg(theme.fg), 0.06);
    mapped.added = mix(background, fg(theme.ok), 0.16);
    mapped.added_word = mix(background, fg(theme.ok), 0.38);
    mapped.removed = mix(background, fg(theme.err), 0.16);
    mapped.removed_word = mix(background, fg(theme.err), 0.38);
    mapped.selected = mix(background, fg(theme.warn), 0.30);
    mapped.call_running = mix(background, fg(theme.warn), 0.14);
    mapped.call_done = mix(background, fg(theme.ok), 0.10);
    mapped.call_failed = mix(background, fg(theme.err), 0.24);
    mapped.note = mix(background, fg(theme.assistant), 0.16);
    mapped.important = mix(background, fg(theme.accent), 0.18);
    mapped
}

// ---------------------------------------------------------------------------
// 三个入口
// ---------------------------------------------------------------------------

/// `text` 当 markdown 画：标题/列表/引用/表格/提示块/代码围栏都按上游的规则，
/// `width` 既是表格的适配列宽（传给 `markdown::render` 的 `Some(width)`），
/// 也是折行宽度。
///
/// 对应上游 `rig_tui::markdown::render(text, &theme, Some(width))`；围栏代码的
/// 语法色由它内部的 `Grammars` 走 [`Code`]，所以这里不用另接一遍高亮。
pub fn markdown_rows(text: &str, width: usize, theme: &Theme) -> Vec<Row> {
    let width = width.max(1);
    let rig = upstream_theme(theme);
    let looks = rig_tui::markdown::render(text, &rig, Some(width));
    rows_of(looks, width)
}

/// 一段代码当代码块画：有语法就按 `language` 的语法着色，没语法就按
/// [`Theme::dim`] 的暗前景色（上游 `code_rows` 的兜底是 `text_rows` + `Modifier::DIM`，
/// 可我们的渲染层不读 `Modifier`，所以换成我们主题的 dim 前景色才看得出是"暗的"）。
///
/// `language` 是围栏上写的那个名字/扩展名（`rust`、`py`、`bash`）；
/// 对应上游 `rig_tui::highlight::code_rows(code, Language::Named(language))`。
pub fn highlight_rows(code: &str, language: &str, width: usize, theme: &Theme) -> Vec<Row> {
    let width = width.max(1);
    let language = Language::Named(language);
    // 只为了问"这门语言有没有语法"而构造一次 `Code`（`code_rows` 里还会再构造一次；
    // 4 KiB 以上的文本上游用线程本地缓存复用，重复构造不会重复着色）。
    if !Code::new(code, language).has_grammar() {
        // 和 `code_rows` 的兜底同一条路径（`text_rows` 的 `OUTPUT_INDENT` 缩进），
        // 只是样式换成我们主题的 dim。
        return rows_of_lines(rig_tui::text_rows(code, theme.dim), width);
    }
    rows_of(rig_tui::highlight::code_rows(code, language), width)
}

/// `text` 若是 unified diff 就按 diff 画：新旧行号、增删行整行红/绿、改动词更深一档；
/// 不是 diff（没有 hunk 头）返回 `None`，让调用方走普通文本——判据和上游
/// `diff::unified_lines` 完全一样（`None` 就是上游的 `None`）。
///
/// 对应上游 `rig_tui::diff::unified_lines(text, &theme)`。
pub fn diff_rows(text: &str, width: usize, theme: &Theme) -> Option<Vec<Row>> {
    let width = width.max(1);
    let rig = upstream_theme(theme);
    let looks = rig_tui::diff::unified_lines(text, &rig)?;
    Some(rows_of(looks, width))
}

/// 工具结果的便利入口：能当 diff 就 [`diff_rows`]（上游 `ToolRender::result_lines`
/// 的 diff 分支），否则按 [`rig_tui::text_rows`] 缩进 + 我们主题的 dim 画
/// （上游那支用的是 `result_style(failed)`，我们这里只做"不是红的"这一半）。
pub fn tool_result_rows(text: &str, width: usize, theme: &Theme) -> Vec<Row> {
    diff_rows(text, width, theme)
        .unwrap_or_else(|| rows_of_lines(rig_tui::text_rows(text, theme.dim), width))
}

// ---------------------------------------------------------------------------
// LookLine -> Row
// ---------------------------------------------------------------------------

/// 上游的 `Vec<LookLine>` → 我们的行：先 `colored()`（把代码高亮的颜色、diff 改动词
/// 的底色叠到行上），再按 `width` 折行，最后落进 [`Row`]。
fn rows_of(looks: Vec<LookLine>, width: usize) -> Vec<Row> {
    rows_of_lines(looks.iter().map(LookLine::colored).collect(), width)
}

/// 折行 + 落行。折行用上游的 [`rig_tui::wrap::wrap_all`]（按 unicode 列宽、词边界、
/// 列表项续行缩进），所以和终端里的行数一致。
fn rows_of_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Row> {
    let wrapped = rig_tui::wrap::wrap_all(&lines, width.max(1));
    wrapped.into_iter().map(expressible).collect()
}

/// 把上游的 `Line` 塞进我们的行模型。
///
/// - **行级底色**（`.bg()`）→ [`Row::band`]：这是 [`Row`] 唯一能表达的底色，
///   上游 diff 的整行红绿、markdown 的代码块/提示块正好都用这一级，落过去是无损的。
///   注意折行会把行级底色抄进每个 span（`rig-markdown` 的 `cells` 做
///   `line.style.patch(span.style)`），所以和 `band` 同色的 span 底色要认出来丢掉。
/// - **span 级底色**（和行级不同的那些，如 diff 改动词、行内 code）→ 退化成
///   前景色 + `UNDERLINED | BOLD`（见 [`lifted`]）。
fn expressible(mut line: Line<'static>) -> Row {
    let band = line.style.bg.take();
    for span in &mut line.spans {
        let Some(tint) = span.style.bg.take() else {
            continue;
        };
        // 折行（`wrap_all`）会把行级底色抄进每个 span（`line.style.patch(span.style)`），
        // 那不是"这一块被标出来"，已经由 `band` 表达，丢掉即可。
        if Some(tint) == band {
            continue;
        }
        span.style.fg = Some(lifted(tint));
        span.style.add_modifier |= Modifier::UNDERLINED | Modifier::BOLD;
    }
    let mut row = Row::new(line);
    if let Some(band) = band {
        row = row.band(band);
    }
    row
}

/// span 级底色的退化：底色当不了底色，就当"这块被标出来的颜色"——取同色相、
/// 朝白提亮的前景（暗底上的高亮块 → 亮字）。
/// 解析不出 RGB 的颜色（`Reset`）退回渲染层的默认文字色（`Reset` 走 `style.rs`
/// 的浅灰）。
///
/// 提亮比例 72% 是**猜的**：上游没有"底色转前景"这个操作，留着足够对比度又不至于
/// 全白，是先拍一个值。
fn lifted(tint: RColor) -> RColor {
    match rgb(tint) {
        Some(tint) => {
            let (r, g, b) = blend(tint, WHITE, 0.72);
            RColor::Rgb(r, g, b)
        }
        None => RColor::Reset,
    }
}

// ---------------------------------------------------------------------------
// 颜色小工具
// ---------------------------------------------------------------------------

/// `from` 朝 `to` 走 `by`（0..=1）。
fn blend(from: (u8, u8, u8), to: (u8, u8, u8), by: f32) -> (u8, u8, u8) {
    let channel = |from: u8, to: u8| {
        let mixed = f32::from(from) + (f32::from(to) - f32::from(from)) * by;
        mixed.round().clamp(0.0, 255.0) as u8
    };
    (
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

/// `from` 朝 `to` 走 `by`，结果是一个 RGB 色。
fn mix(from: (u8, u8, u8), to: (u8, u8, u8), by: f32) -> RColor {
    let (r, g, b) = blend(from, to, by);
    RColor::Rgb(r, g, b)
}

/// ratatui 的颜色 → RGB，只给"混色"用：命名色取 xterm 的 16 色默认值，
/// `Indexed` 按 xterm 256 的色板展开；`Reset` 等没有色相的值返回 `None`
/// （调用方保留上游默认/兜底）。
fn rgb(color: RColor) -> Option<(u8, u8, u8)> {
    // xterm 的 16 个基础色（0–15），和 `style.rs` 的 `indexed` 同一张表。
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match color {
        RColor::Reset => None,
        RColor::Black => Some(BASE[0]),
        RColor::Red => Some(BASE[1]),
        RColor::Green => Some(BASE[2]),
        RColor::Yellow => Some(BASE[3]),
        RColor::Blue => Some(BASE[4]),
        RColor::Magenta => Some(BASE[5]),
        RColor::Cyan => Some(BASE[6]),
        RColor::Gray => Some(BASE[7]),
        RColor::DarkGray => Some(BASE[8]),
        RColor::LightRed => Some(BASE[9]),
        RColor::LightGreen => Some(BASE[10]),
        RColor::LightYellow => Some(BASE[11]),
        RColor::LightBlue => Some(BASE[12]),
        RColor::LightMagenta => Some(BASE[13]),
        RColor::LightCyan => Some(BASE[14]),
        RColor::White => Some(BASE[15]),
        RColor::Rgb(r, g, b) => Some((r, g, b)),
        RColor::Indexed(index) => Some(match index {
            0..=15 => BASE[usize::from(index)],
            16..=231 => {
                // 6×6×6 立方体，每级 0/95/135/175/215/255。
                const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
                let n = index - 16;
                (
                    STEPS[usize::from(n / 36)],
                    STEPS[usize::from(n / 6 % 6)],
                    STEPS[usize::from(n % 6)],
                )
            }
            // 232–255 是 8 起步、间隔 10 的灰阶。
            index => {
                let level = 8 + 10 * (index - 232);
                (level, level, level)
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_tui::ratatui::text::Span;

    /// 一行按渲染层的方式拼出来的纯文本。
    fn text(row: &Row) -> String {
        row.line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    /// 第一个文本里含 `needle` 的行。
    fn row_with<'a>(rows: &'a [Row], needle: &str) -> &'a Row {
        rows.iter().find(|row| text(row).contains(needle)).unwrap_or_else(|| {
            panic!(
                "没有含 {needle:?} 的行：{:?}",
                rows.iter().map(text).collect::<Vec<_>>()
            )
        })
    }

    /// 一行里第一个文本正好是 `needle` 的 span。
    fn span_with<'a>(row: &'a Row, needle: &str) -> &'a Span<'static> {
        row.line
            .spans
            .iter()
            .find(|span| span.content == needle)
            .unwrap_or_else(|| {
                panic!(
                    "行 {:?} 里没有 span {needle:?}",
                    text(row)
                )
            })
    }

    /// 我们的行模型没有 span 级底色：退化之后所有 span 的 `bg` 都必须是 `None`。
    fn assert_no_span_backgrounds(rows: &[Row]) {
        for row in rows {
            for span in &row.line.spans {
                assert_eq!(span.style.bg, None, "span {:?} 还带着底色", span.content);
            }
        }
    }

    #[test]
    fn theme_bands_borrow_our_palette() {
        let rig = upstream_theme(&Theme::default());
        // diff 的红绿用我们的 err/ok 色相，不是上游写死的 hue。
        let RColor::Rgb(ar, ag, ab) = rig.added else {
            panic!("added 应该是 RGB：{:?}", rig.added);
        };
        let RColor::Rgb(rr, rg, rb) = rig.removed else {
            panic!("removed 应该是 RGB：{:?}", rig.removed);
        };
        assert!(ag > ar && ag > ab, "added 不是偏绿的：{:?}", rig.added);
        assert!(rr > rg && rr > rb, "removed 不是偏红的：{:?}", rig.removed);
        assert_ne!(rig.added, rig.removed);
        // 猜的字段之一 `code`：由 band 朝白混 8%，是中性灰（没有色相）。
        let RColor::Rgb(cr, cg, cb) = rig.code else {
            panic!("code 应该是 RGB：{:?}", rig.code);
        };
        assert!(cg.abs_diff(cr) < 8 && cb.abs_diff(cg) < 16, "code 不该有色相");
        // `plain` 是猜的：我们没有 NO_COLOR 通路，恒为 false。
        assert!(!rig.plain);
    }

    #[test]
    fn markdown_headings_and_lists_drop_their_markers() {
        let theme = Theme::default();
        let rows = markdown_rows("## 标题\n\n- 第一项\n", 40, &theme);
        // 标题：没有 `##`，整行加粗（上游 heading 的样式）。
        let heading = row_with(&rows, "标题");
        assert!(!text(heading).contains('#'));
        assert!(
            heading.line.spans.iter().all(|span| span
                .style
                .add_modifier
                .contains(Modifier::BOLD)),
            "标题行没有整行加粗：{:?}",
            text(heading)
        );
        // 列表：上游把 `-` 画成 `• `，前缀是强调色。
        let item = row_with(&rows, "第一项");
        assert!(text(item).starts_with("• "), "{:?}", text(item));
        assert_eq!(span_with(item, "• ").style.fg, Some(RColor::Cyan));
        assert_no_span_backgrounds(&rows);
    }

    #[test]
    fn a_fenced_block_takes_the_code_band_and_its_syntax_colors() {
        let theme = Theme::default();
        let code = upstream_theme(&theme).code;
        let rows = markdown_rows("```rust\nfn main() {}\n```\n", 40, &theme);
        assert_eq!(
            rows.iter().map(text).collect::<Vec<_>>(),
            vec!["  rust".to_string(), "  fn main() {}".to_string()]
        );
        // 代码块的行级底色 → `Row.band`（渲染层唯一能画的底色）。
        for row in &rows {
            assert_eq!(row.band, Some(code), "{:?} 没落代码底色", text(row));
        }
        // ansi 语法主题给的是终端调色板的索引色，`fn` 与 `main` 不同色。
        let keyword = span_with(&rows[1], "fn").style.fg;
        let name = span_with(&rows[1], "main").style.fg;
        assert!(matches!(keyword, Some(RColor::Indexed(_))), "{keyword:?}");
        assert!(matches!(name, Some(RColor::Indexed(_))), "{name:?}");
        assert_ne!(keyword, name);
        assert_no_span_backgrounds(&rows);
    }

    #[test]
    fn a_note_block_takes_the_note_band() {
        let theme = Theme::default();
        let note = upstream_theme(&theme).note;
        let rows = markdown_rows("> [!NOTE]\n> 注意事项\n", 40, &theme);
        assert_eq!(rows.len(), 2, "{:?}", rows.iter().map(text).collect::<Vec<_>>());
        assert_eq!(text(&rows[0]), "Note");
        assert!(text(&rows[1]).contains("注意事项"));
        for row in &rows {
            assert_eq!(row.band, Some(note), "{:?} 没落 note 底色", text(row));
        }
    }

    #[test]
    fn markdown_rows_fit_the_width() {
        let theme = Theme::default();
        let rows = markdown_rows(
            "The quick brown fox jumps over the lazy dog again and again.",
            20,
            &theme,
        );
        assert!(rows.len() > 1, "宽度 20 应该折行：{:?}", rows.iter().map(text).collect::<Vec<_>>());
        for row in &rows {
            assert!(
                row.line.width() <= 20,
                "{:?} 宽 {}",
                text(row),
                row.line.width()
            );
        }
    }

    #[test]
    fn inline_code_becomes_a_bright_underlined_span() {
        let theme = Theme::default();
        let rows = markdown_rows("正文 `inline` 结束\n", 40, &theme);
        let row = row_with(&rows, "inline");
        let span = span_with(row, "inline");
        // span 级底色表达不了：退化成亮前景 + 下划线，底色本身必须清掉。
        assert_eq!(span.style.bg, None);
        assert!(span.style.add_modifier.contains(Modifier::UNDERLINED));
        let Some(RColor::Rgb(r, g, b)) = span.style.fg else {
            panic!("退化后的前景应该是 RGB：{:?}", span.style.fg);
        };
        assert!(
            r > 150 && g > 150 && b > 150,
            "退化后的前景应该是亮的：{:?}",
            span.style.fg
        );
    }

    /// 一个两删两增的小 diff：旧文件 3 行、新文件 4 行。
    const DIFF: &str = "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,4 @@\n fn main() {\n-    let x = 1;\n+    let value = 42;\n+    println!(\"{value}\");\n }\n";

    #[test]
    fn diff_rows_put_removed_and_added_lines_on_their_bands() {
        let theme = Theme::default();
        let rig = upstream_theme(&theme);
        let rows = diff_rows(DIFF, 80, &theme).expect("是 unified diff");
        assert_eq!(rows.len(), 8, "{:?}", rows.iter().map(text).collect::<Vec<_>>());
        // 删行：整行红底、gutter 红字、行首是新旧行号里的旧号。
        let removed = row_with(&rows, "let x = 1;");
        assert_eq!(removed.band, Some(rig.removed));
        assert_eq!(removed.line.spans[0].style.fg, Some(RColor::Red));
        assert!(text(removed).starts_with("    2   - "), "{:?}", text(removed));
        // 增行：整行绿底、gutter 绿字。
        let added = row_with(&rows, "let value = 42;");
        assert_eq!(added.band, Some(rig.added));
        assert_eq!(added.line.spans[0].style.fg, Some(RColor::Green));
        assert!(text(added).starts_with("      2 + "), "{:?}", text(added));
        // 没变的上下文行不该有底色。
        assert_eq!(row_with(&rows, "fn main() {").band, None);
        assert_no_span_backgrounds(&rows);
    }

    #[test]
    fn diff_changed_words_trade_the_deeper_band_for_a_brighter_foreground() {
        let theme = Theme::default();
        let rig = upstream_theme(&theme);
        let rows = diff_rows(DIFF, 80, &theme).expect("是 unified diff");
        // `x` → `value` 是改动词：上游给它更深的 added_word 底色，我们只能抬成亮前景。
        let added = row_with(&rows, "let value = 42;");
        let word = span_with(added, "value");
        assert_eq!(word.style.bg, None);
        assert!(word.style.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(word.style.fg, Some(lifted(rig.added_word)));
        // 同一行没改的 `let` 还是语法色，不是改动词的亮色。
        assert_ne!(span_with(added, "let").style.fg, word.style.fg);
        // 删行一侧同理。
        let removed = row_with(&rows, "let x = 1;");
        assert_eq!(span_with(removed, "x").style.fg, Some(lifted(rig.removed_word)));
    }

    #[test]
    fn plain_text_is_not_a_diff() {
        let theme = Theme::default();
        assert!(diff_rows("hello\nworld\n", 20, &theme).is_none());
        // 工具结果的便利入口退到缩进文本。
        let rows = tool_result_rows("hello\n", 20, &theme);
        assert_eq!(rows.len(), 1);
        assert_eq!(text(&rows[0]), format!("{}hello", rig_tui::OUTPUT_INDENT));
        assert_eq!(rows[0].band, None);
        assert_eq!(rows[0].line.spans[0].style.fg, theme.dim.fg);
        // 是 diff 就走 diff 分支：至少增删行有底色。
        assert!(
            tool_result_rows(DIFF, 80, &theme)
                .iter()
                .any(|row| row.band.is_some())
        );
    }

    #[test]
    fn a_code_block_is_colored_by_its_languages_grammar() {
        let theme = Theme::default();
        let rows = highlight_rows("fn main() {}\n", "rust", 40, &theme);
        assert_eq!(rows.len(), 1);
        assert_eq!(text(&rows[0]), "   fn main() {}");
        let keyword = span_with(&rows[0], "fn").style.fg;
        let name = span_with(&rows[0], "main").style.fg;
        assert!(matches!(keyword, Some(RColor::Indexed(_))), "{keyword:?}");
        assert!(matches!(name, Some(RColor::Indexed(_))), "{name:?}");
        assert_ne!(keyword, name);
        assert_eq!(rows[0].band, None, "单独的代码块没有行级底色");
    }

    #[test]
    fn a_language_without_a_grammar_falls_back_to_the_dim_color() {
        let theme = Theme::default();
        let rows = highlight_rows("just text\n", "no-such-language", 40, &theme);
        assert_eq!(rows.len(), 1);
        assert_eq!(text(&rows[0]), "   just text");
        // 上游兜底是 `Modifier::DIM`，我们的渲染层不读 Modifier → 换成主题的 dim 前景。
        assert_eq!(rows[0].line.spans[0].style.fg, theme.dim.fg);
        assert_eq!(theme.dim.fg, Some(RColor::Rgb(120, 128, 145)));
    }

    #[test]
    fn code_wraps_at_the_width() {
        let theme = Theme::default();
        let line = format!("let x = \"{}\";\n", "a".repeat(40));
        let rows = highlight_rows(&line, "rust", 20, &theme);
        assert!(rows.len() > 1, "宽度 20 应该折行");
        for row in &rows {
            assert!(
                row.line.width() <= 20,
                "{:?} 宽 {}",
                text(row),
                row.line.width()
            );
        }
    }
}
