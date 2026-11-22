//! The transcript bridge: live `rig_ecs` tool-call effects -> rig-tui `RenderBatch` -> looks.
//!
//! rig-tui's `renderers::add` is `pub(crate)`, so we drive the batch ourselves: fill a
//! `RenderBatch` from the kernel's effect entities, let the looks registered on
//! `RenderSystems::Render` rewrite `RenderOutput::lines`, then read those lines and turn them
//! into feathers rows.

use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use rig_core::message::{Text, ToolResult, ToolResultContent};
use rig_ecs::prelude::*;
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};
use rig_tui::{
    CallState, LookLine, RenderBatch, RenderInput, RenderKey, RenderOutput, RenderRequest,
    RenderSystems, ToolRender, TranscriptRender,
};

use crate::editor::SubmittedPrompts;
use crate::frame::{PanelAreas, PanelRoots};
use crate::pane::TranscriptRows;
use crate::panels::{KeyedRow, PanelId};
use crate::transcript::Row;
use rig_core::message::ToolCall;

/// 已结算的调用（效应实体结算后会被回收，转写需要自己记历史）。
#[derive(Resource, Default)]
pub struct CompletedCalls(pub Vec<(ToolCall, ToolResult)>);

/// 已记入历史的效应。
#[derive(Component)]
pub struct Recorded;

/// 在 Review 阶段（Commit 之前）把 answered 的调用抄进历史。
fn record_calls(
    mut commands: Commands,
    calls: Query<(Entity, &ToolRequest, &Outcome), Without<Recorded>>,
    mut completed: ResMut<CompletedCalls>,
) {
    for (entity, request, outcome) in calls.iter() {
        let tool = outcome.tool_result();
        let body = tool.output().render();
        completed.0.push((
            request.call.clone(),
            ToolResult {
                call: request.call.id.clone(),
                name: request.call.function.name.clone(),
                content: vec![ToolResultContent::Text(Text::new(body))],
                is_error: tool.is_error(),
            },
        ));
        commands.entity(entity).insert(Recorded);
    }
}

/// 模拟模型流式输出（真模型接上之前，用它验证"增量刷新"）。
#[derive(Resource, Default)]
pub struct ReplyStream {
    pub text: String,
    pub ticks: u32,
    pub finished: bool,
}

fn key_of(parts: &[&str]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut hasher);
    hasher.finish()
}

fn row_hash(row: &Row) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for span in &row.line.spans {
        span.content.hash(&mut hasher);
        format!("{:?}", span.style.fg).hash(&mut hasher);
    }
    for span in row.right.iter().flat_map(|right| right.spans.iter()) {
        span.content.hash(&mut hasher);
        format!("{:?}", span.style.fg).hash(&mut hasher);
    }
    hasher.finish()
}

/// 流式追加：每帧吐几个字，直到说完。
fn drive_reply_stream(mut stream: ResMut<ReplyStream>) {
    if stream.finished {
        return;
    }
    const FULL: &str = "推荐第 1 双：Nike Air Zoom Pegasus 40，469 元，官方旗舰店，缓震和口碑都稳。要下单吗？";
    stream.ticks += 1;
    // 两帧一个字：巡游前半段能看到"正在流"，后半段能看到说完
    let shown = ((stream.ticks + 1) / 2) as usize;
    let next: String = FULL.chars().take(shown).collect();
    stream.text = next;
    if stream.text.chars().count() >= FULL.chars().count() {
        stream.finished = true;
    }
}

/// Reads the kernel's tool-call effects into a fresh batch.
///
/// `width` is the transcript pane's text columns, so looks wrap at the same
/// width rig-tui's panes give them.
pub fn rebuild_batch(
    mut batch: ResMut<RenderBatch>,
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
    completed: Res<CompletedCalls>,
    requests: Query<
        (Entity, &ToolRequest, Option<&Outcome>, Option<&Running>),
        (With<ForAgent>, Without<Settled>),
    >,
    holds: Query<&Holds>,
) {
    batch.requests.clear();
    batch.width = roots
        .entity(PanelId::Transcript)
        .and_then(|root| areas.get(root))
        .map_or(78, |area| area.cols);
    let mut ordinal = 0usize;

    // 1) 历史：已结算的调用永远是 Done
    for (call, result) in completed.0.iter() {
        let render = ToolRender::new(call.clone(), Some(result.clone()), CallState::Done);
        batch.requests.insert(
            RenderKey::Tool(0, ordinal),
            RenderRequest {
                input: RenderInput::Tool(render),
                output: RenderOutput::default(),
            },
        );
        ordinal += 1;
    }

    // 2) 现场：还在排队/运行/被闸门按住的调用
    for (entity, request, outcome, running) in requests.iter() {
        let held = holds.get(entity).map(|h| h.len()).unwrap_or(0) > 0;
        let state = if outcome.is_some() {
            CallState::Done
        } else if held {
            CallState::Held(vec!["waiting for your confirmation".into()])
        } else if running.is_some() {
            CallState::Running(None)
        } else {
            CallState::Waiting
        };

        // The kernel's outcome -> the conversation-shaped result a look reads.
        let result = outcome.map(|outcome| {
            let tool = outcome.tool_result();
            let body = tool.output().render();
            ToolResult {
                call: request.call.id.clone(),
                name: request.call.function.name.clone(),
                content: vec![ToolResultContent::Text(Text::new(body))],
                is_error: tool.is_error(),
            }
        });

        let call = ToolRender::new(request.call.clone(), result, state);
        batch.requests.insert(
            RenderKey::Tool(0, ordinal),
            RenderRequest {
                input: RenderInput::Tool(call),
                output: RenderOutput::default(),
            },
        );
        ordinal += 1;
    }
}

/// Runs rig-tui's look schedule once (exclusive: it needs the whole world).
pub fn run_looks(world: &mut World) {
    world.run_schedule(TranscriptRender);
}

/// Turns the batch's look lines into rows, rebuilding only when something changed.
pub fn collect_rows(
    batch: Res<RenderBatch>,
    stream: Res<ReplyStream>,
    prompts: Res<SubmittedPrompts>,
    mut transcript: ResMut<TranscriptRows>,
    asks: Query<(Entity, &rig_harness::prelude::Ask), Without<Outcome>>,
) {
    let mut rows: Vec<Row> = Vec::new();
    let mut row_keys: Vec<u64> = Vec::new();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    for (index, prompt) in prompts.0.iter().enumerate() {
        row_keys.push(key_of(&["prompt", &index.to_string()]));
        rows.push(Row::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(RColor::Green)),
            Span::styled(
                prompt.clone(),
                Style::default().fg(RColor::Cyan),
            ),
        ]))
        .band(RColor::Rgb(16, 22, 34)));
        stream.text.hash(&mut hasher);
    }
    if !prompts.0.is_empty() {
        row_keys.push(key_of(&["prompts-gap"]));
        rows.push(Row::new(Line::from("")));
    }

    let mut order: Vec<&RenderKey> = batch.requests.keys().collect();
    order.sort_by_key(|key| match key {
        RenderKey::Tool(message, block) => (*message as u64) * 1000 + *block as u64,
        _ => u64::MAX,
    });
    for key in order {
        if let Some(request) = batch.requests.get(key) {
            let base = format!("{key:?}");
            for (index, line) in request.output.lines.iter().enumerate() {
                let row = Row::from_look(line);
                let plain: String = row.line.spans.iter().map(|s| s.content.as_ref()).collect();
                plain.hash(&mut hasher);
                row_keys.push(key_of(&[&base, &index.to_string()]));
                rows.push(row);
            }
            "".hash(&mut hasher);
            row_keys.push(key_of(&[&base, "gap"]));
            rows.push(Row::new(Line::from("")));
        }
    }

    // 模拟的流式回复（真模型接上后换成 rig-core 的 stream）
    if !stream.text.is_empty() {
        stream.text.hash(&mut hasher);
        row_keys.push(key_of(&["reply"]));
        rows.push(Row::new(Line::from(vec![
            Span::styled("assistant  ", Style::default().fg(RColor::Cyan)),
            Span::styled(
                stream.text.clone(),
                Style::default().fg(RColor::Rgb(220, 226, 240)),
            ),
            Span::styled(
                if stream.finished { " |" } else { " ▍" },
                Style::default().fg(RColor::Rgb(120, 128, 145)),
            ),
        ])));
    }

    // 闸门：把未回答的 Ask 追加成可点的行
    for (ask_entity, ask) in asks.iter() {
        ask.title.hash(&mut hasher);
        ask.body.hash(&mut hasher);
        let ask_key = format!("{ask_entity:?}");
        row_keys.push(key_of(&[&ask_key, "title"]));
        rows.push(Row::new(Line::from(vec![
            Span::styled("? ", Style::default().fg(RColor::Magenta)),
            Span::styled(
                ask.title.clone(),
                Style::default().fg(RColor::LightMagenta).add_modifier(Modifier::BOLD),
            ),
        ])));
        for (index, body_line) in ask.body.lines().enumerate() {
            row_keys.push(key_of(&[&ask_key, "body", &index.to_string()]));
            rows.push(Row::new(Line::from(Span::styled(
                format!("    {body_line}"),
                Style::default().fg(RColor::Rgb(200, 206, 220)),
            ))));
        }
        for (index, choice) in ask.choices.iter().enumerate() {
            row_keys.push(key_of(&[&ask_key, "choice", &index.to_string()]));
            rows.push(
                Row::new(Line::from(Span::styled(
                    format!("    [{}] {}", index + 1, choice.label),
                    Style::default()
                        .fg(RColor::Green)
                        .add_modifier(Modifier::BOLD),
                )))
                .ask_choice(ask_entity, choice.id.clone()),
            );
        }
        row_keys.push(key_of(&[&ask_key, "gap"]));
        rows.push(Row::new(Line::from("")));
    }

    let keyed: Vec<KeyedRow> = rows
        .into_iter()
        .zip(row_keys)
        .map(|(row, key)| KeyedRow {
            key,
            hash: row_hash(&row),
            row,
        })
        .collect();
    transcript.0 = keyed;
}

/// The two looks this demo ships, registered on rig-tui's own render schedule.
fn search_product_look(mut batch: ResMut<RenderBatch>) {
    for (call, out) in batch.tools_mut("search_products") {
        out.lines = search_product_lines(call);
    }
}

fn search_product_lines(call: &ToolRender) -> Vec<LookLine> {
    let dim = Style::default().fg(RColor::Rgb(120, 128, 145));
    let secondary = Style::default().fg(RColor::Rgb(165, 175, 195));
    let price_style = Style::default().fg(RColor::Green);
    let rate = Style::default().fg(RColor::LightYellow);
    let state = match &call.state {
        CallState::Streaming => "(streaming)".to_string(),
        CallState::Running(_) => "(running)".to_string(),
        CallState::Held(reasons) => format!("(held: {})", reasons.join(", ")),
        CallState::Waiting => "(queued)".to_string(),
        CallState::Done => "(done)".to_string(),
        CallState::Stopped => "(stopped)".to_string(),
    };

    let mut lines: Vec<LookLine> = Vec::new();
    lines.push(
        Line::from(vec![
            Span::styled("* ", Style::default().fg(RColor::Green)),
            Span::styled(
                "search_products",
                Style::default().fg(RColor::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {state}"), dim),
            Span::styled(
                format!("  query={}", call.argument("query").unwrap_or("-")),
                secondary,
            ),
        ])
        .into(),
    );

    if let Some(text) = call.result_text()
        && let Ok(products) = serde_json::from_str::<serde_json::Value>(&text)
    {
        lines.push(
            Line::from(Span::styled(
                "  #  name                          price  shop             rate",
                dim,
            ))
            .into(),
        );
        for (index, product) in products.as_array().into_iter().flatten().enumerate() {
            let name = product["name"].as_str().unwrap_or("?");
            let shop = product["shop"].as_str().unwrap_or("?");
            let price = product["price"].as_i64().unwrap_or(0);
            let rating = product["rating"].as_f64().unwrap_or(0.0);
            lines.push(
                Line::from(vec![
                    Span::styled(format!("  {}  ", index + 1), dim),
                    Span::styled(format!("{name:<29}"), secondary),
                    Span::styled(format!("{price:<6}"), price_style),
                    Span::styled(format!("{shop:<17}"), secondary),
                    Span::styled(format!("{rating:.1}"), rate),
                ])
                .into(),
            );
        }
    }
    lines
}

/// `create_order` 的 look：等待确认 / 已下单。
fn order_look(mut batch: ResMut<RenderBatch>) {
    for (call, out) in batch.tools_mut("create_order") {
        let held = matches!(call.state, CallState::Held(_));
        let done = matches!(call.state, CallState::Done);
        let tail = if held {
            "  waiting for your confirmation (gate)"
        } else if done {
            "  order placed"
        } else {
            "  queued"
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled("* ", Style::default().fg(RColor::Magenta)),
                Span::styled("create_order", Style::default().fg(RColor::LightMagenta)),
                Span::styled(tail, Style::default().fg(RColor::Rgb(180, 186, 200))),
            ])
            .into(),
        ];
        if done && let Some(text) = call.result_text() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                lines.push(
                    Line::from(Span::styled(
                        format!(
                            "  order {} · total {} · status {}",
                            value["order_id"].as_str().unwrap_or("-"),
                            value["total"].as_i64().unwrap_or(0),
                            value["status"].as_str().unwrap_or("-")
                        ),
                        Style::default().fg(RColor::Green),
                    ))
                    .into(),
                );
            }
        }
        out.lines = lines;
    }
}

pub struct BatchPlugin;

impl Plugin for BatchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderBatch>()
            .init_resource::<ReplyStream>()
            .configure_sets(
                TranscriptRender,
                (
                    RenderSystems::Render,
                    RenderSystems::Decorate,
                    RenderSystems::Edit,
                )
                    .chain(),
            )
            .add_systems(
                TranscriptRender,
                search_product_look.in_set(RenderSystems::Render),
            )
            .add_systems(TranscriptRender, order_look.in_set(RenderSystems::Render))
            .add_systems(
                AgentPass,
                record_calls.in_set(Stage::Review),
            )
            .add_systems(
                Update,
                (drive_reply_stream, rebuild_batch, run_looks, collect_rows)
                    .chain()
                    .in_set(crate::FeathersSystems::Draw),
            );
    }
}
