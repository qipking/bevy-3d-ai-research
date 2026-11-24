//! `Notice` 进转写的**唯一一条路**：应用层把通知交给引擎的 [`TextId::Notice`]，
//! 排序、折行、和对话的顺序/回滚都由 [`Transcript`] 负责。
//!
//! 上一轮的 `NoticeLog`（应用层自己拼行、直接塞 `TranscriptRows`）已经删掉，
//! 否则真 agent 的通知会画两遍。这里保留的 [`NoticeHistory`] 不是第二条渲染
//! 路径：它只是原始通知的文本行，每帧重新交给引擎，避免引擎在 `collect_rows`
//! 里被 `retain_text` 清掉后丢失。
//!
//! ## 为什么在 `Fit` 里回填
//!
//! `batch::collect_rows`（`Draw`）每帧用 `retain_text` 清掉它不认识的文字块，
//! 目前只保留 Prompt / 对话文字 / 流式回复，不保留 `TextId::Notice`。所以本系统
//! 排在它之后：把历史重新 `set_text` 进引擎，取引擎排好序的行流，再把
//! `collect_rows` 追加在引擎行之后的 Ask 行原样接回去（按"引擎行数"切，而不是
//! 靠 key 猜）。
//!
//! 如果 `rig-feathers` 以后让 `collect_rows` 保留 `TextId::Notice(_)`，本系统的
//! 回填就可以去掉，只留"收到 Notice 时 set_text + place"。

use bevy::prelude::*;
use rig_ecs::prelude::{Agent, Notice, NoticeLevel};
use rig_feathers::frame::{PanelAreas, PanelRoots};
use rig_feathers::pane::TranscriptRows;
use rig_feathers::panels::PanelId;
use rig_feathers::transcript::{TextId, Transcript};
use rig_tui::RenderBatch;
use rig_tui::ratatui::style::{Color as RColor, Style};
use rig_tui::ratatui::text::Line;

/// 通知的原始文本行（按出现顺序），交给转写引擎。
///
/// 一条通知可以有多行；这里只存行，不存 key/位置——位置由引擎按
/// [`TextId::Notice`] 的序号和对话块排序决定。
#[derive(Resource, Default)]
pub struct NoticeHistory {
    lines: Vec<Vec<Line<'static>>>,
}

impl NoticeHistory {
    /// 收到过多少条通知（多行通知算一条）。
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// 一条通知的文本行（测试断言用）。
    pub fn lines(&self, index: usize) -> Option<&[Line<'static>]> {
        self.lines.get(index).map(Vec::as_slice)
    }

    /// 通知 -> 行：和上游 `transcript::notice_lines` 同一套（按级别着色、逐行、
    /// tab 换成四个空格）。
    fn remember(&mut self, notice: &Notice) {
        let style = match notice.level {
            NoticeLevel::Info => Style::new().fg(RColor::Magenta),
            NoticeLevel::Warn => Style::new().fg(RColor::Yellow),
            NoticeLevel::Error => Style::new().fg(RColor::Red),
        };
        let lines: Vec<Line<'static>> = notice
            .text
            .lines()
            .map(|line| Line::styled(line.replace('\t', "    "), style))
            .collect();
        debug!(
            "[notice] {} 行，级别 {:?}：{}",
            lines.len(),
            notice.level,
            notice.text
        );
        self.lines.push(lines);
    }
}

/// 把新通知收进历史，并把历史交给引擎；排在 `collect_rows` 之后、窗口化之前。
pub fn notices_to_transcript(
    mut notices: MessageReader<Notice>,
    agents: Query<(), With<Agent>>,
    mut history: ResMut<NoticeHistory>,
    mut transcript: ResMut<Transcript>,
    batch: Option<Res<RenderBatch>>,
    areas: Option<Res<PanelAreas>>,
    roots: Option<Res<PanelRoots>>,
    mut rows: ResMut<TranscriptRows>,
) {
    for notice in notices.read() {
        // 关于某个已经不在的 agent 的通知不显示（上游同样的过滤）。
        if let Some(agent) = notice.agent
            && !agents.contains(agent)
        {
            continue;
        }
        history.remember(notice);
    }

    if history.is_empty() {
        return;
    }
    let Some(width) = engine_width(batch.as_deref(), areas.as_deref(), roots.as_deref()) else {
        // 还没从 Layout 拿到列数：引擎这一帧不出行，下一帧再来。
        return;
    };
    transcript.set_width(width);

    // `collect_rows` 这一帧写进 `TranscriptRows` 的引擎行数（Ask 行追加在它后面）。
    let engine_rows = transcript.keyed_rows().len();
    for (index, lines) in history.lines.iter().enumerate() {
        transcript.set_text(TextId::Notice(index), lines.clone());
    }
    let rebuilt = transcript.keyed_rows().to_vec();

    let old = std::mem::take(&mut rows.0);
    rows.0 = rebuilt;
    rows.0.extend(old.into_iter().skip(engine_rows));
}

/// 引擎的折行宽度：优先 `RenderBatch`（`collect_rows` 用的同一个），没有布局时
/// 退回转写面板的列数。
fn engine_width(
    batch: Option<&RenderBatch>,
    areas: Option<&PanelAreas>,
    roots: Option<&PanelRoots>,
) -> Option<u16> {
    if let Some(width) = batch.map(|batch| batch.width).filter(|width| *width > 0) {
        return Some(width);
    }
    let root = roots?.entity(PanelId::Transcript)?;
    let area = areas?.get(root)?;
    (area.cols > 0).then_some(area.cols)
}
