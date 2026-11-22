//! Drive rig-tui's look pipeline ourselves.
//!
//! rig-tui's `renderers::add` is `pub(crate)`, so an outside crate cannot ask it to fill a
//! `RenderBatch`. What *is* public is enough to do the job:
//!   * `RenderBatch` is a plain `Resource` with public fields,
//!   * `TranscriptRender` is a public schedule and `RenderSystems` its phases,
//!   * `RenderOutput::lines: Vec<LookLine>` and `LookLine::colored()` are public.
//! So this module builds the batch (with rig-core's `ToolCall`/`ToolResult`), registers looks
//! in `RenderSystems::Render`, runs the schedule once, and turns the resulting look lines into
//! feathers rows. rig-tui's own driver would do exactly this on every layout pass.

use std::collections::HashMap;

use bevy::prelude::*;
use rig_core::message::{CallId, Text, ToolCall, ToolFunction, ToolName, ToolResult, ToolResultContent};
use rig_tui::ratatui::style::{Color as RColor, Style};
use rig_tui::ratatui::text::{Line, Span};
use rig_tui::{
    CallState, LookLine, RenderBatch, RenderInput, RenderKey, RenderOutput, RenderRequest,
    RenderSystems, ToolRender, TranscriptRender,
};

use crate::transcript::Row;

/// Look lines produced by one tool call, kept so the app can spawn them.
#[derive(Resource, Default)]
pub struct BatchRows(pub Vec<Row>);

fn tool_call(id: &str, name: &str, args_json: &str) -> ToolCall {
    let name = ToolName::new(name).expect("non-empty tool name");
    ToolCall::new(CallId::from_wire(id), ToolFunction::parse(name, args_json))
}

/// A completed `search_products` call, with the catalog JSON a real backend would return.
fn search_result(call_id: &str, name: &str) -> ToolResult {
    ToolResult {
        call: CallId::from_wire(call_id),
        name: ToolName::new(name).expect("non-empty tool name"),
        content: vec![ToolResultContent::Text(Text::new(
            r#"[
          {"id":"p-1001","name":"Nike Air Zoom Pegasus 40","price":469,"shop":"official store","rating":4.8},
          {"id":"p-1002","name":"ASICS Gel-Kayano 30","price":499,"shop":"runner shop","rating":4.7},
          {"id":"p-1003","name":"Li-Ning ChiTu 7 Pro","price":439,"shop":"li-ning","rating":4.6}
        ]"#,
        ))],
        is_error: false,
    }
}

/// The three calls the demo transcript shows: one streaming, one answered, one held by a gate.
pub fn demo_batch() -> RenderBatch {
    let mut batch = RenderBatch {
        pane: None,
        agent: None,
        width: 78,
        requests: HashMap::new(),
    };
    let mut put = |ordinal: usize, call: ToolRender| {
        batch.requests.insert(
            RenderKey::Tool(0, ordinal),
            RenderRequest {
                input: RenderInput::Tool(call),
                output: RenderOutput::default(),
            },
        );
    };

    // 1) 流式中的调用：参数还没收全
    put(
        0,
        ToolRender::new(
            tool_call("call_a", "search_products", r#"{"query":"running shoe""#),
            None,
            CallState::Streaming,
        ),
    );
    // 2) 已完成：带结果 JSON
    put(
        1,
        ToolRender::new(
            tool_call(
                "call_b",
                "search_products",
                r#"{"query":"running shoe","max":500,"limit":3}"#,
            ),
            Some(search_result("call_b", "search_products")),
            CallState::Done,
        ),
    );
    // 3) 被等待：闸门还没答复
    put(
        2,
        ToolRender::new(
            tool_call(
                "call_c",
                "create_order",
                r#"{"cart":"cart_1","address":"addr_1"}"#,
            ),
            None,
            CallState::Held(vec!["waiting for payment confirmation".into()]),
        ),
    );
    batch
}

/// The demo look for `search_products`:参数行 + 商品表 + 结果中的店铺/评分。
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
        CallState::Streaming => {
            let partial: String = call
                .argument("query")
                .unwrap_or("")
                .to_string();
            format!("(streaming) query=\"{partial}\" ...")
        }
        CallState::Running(_) => "(running)".to_string(),
        CallState::Held(reasons) => format!("(held: {})", reasons.join(", ")),
        CallState::Waiting => "(waiting)".to_string(),
        CallState::Done => "(done)".to_string(),
        CallState::Stopped => "(stopped)".to_string(),
    };

    let mut lines: Vec<LookLine> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("* ", Style::default().fg(RColor::Green)),
        Span::styled(
            "search_products",
            Style::default()
                .fg(RColor::Yellow)
                .add_modifier(rig_tui::ratatui::style::Modifier::BOLD),
        ),
        Span::styled(format!("  {state}"), dim),
    ])
    .into());

    if let Some(text) = call.result_text() {
        if let Ok(products) = serde_json::from_str::<serde_json::Value>(&text) {
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
    }
    lines
}

/// `create_order` 在等闸门：用 look 显示"等用户确认"。
fn order_look(mut batch: ResMut<RenderBatch>) {
    for (call, out) in batch.tools_mut("create_order") {
        let held = matches!(call.state, CallState::Held(_));
        out.lines = vec![
            Line::from(vec![
                Span::styled("* ", Style::default().fg(RColor::Magenta)),
                Span::styled(
                    "create_order",
                    Style::default().fg(RColor::LightMagenta),
                ),
                Span::styled(
                    if held {
                        "  awaiting your confirmation (gate)"
                    } else {
                        "  order submitted"
                    },
                    Style::default().fg(RColor::Rgb(180, 186, 200)),
                ),
            ])
            .into(),
        ];
    }
}

/// Registers the looks and runs the batch once into feathers rows.
pub struct BatchPlugin;

impl Plugin for BatchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderBatch>()
            .init_resource::<BatchRows>()
            .configure_sets(
                TranscriptRender,
                (
                    RenderSystems::Render,
                    RenderSystems::Decorate,
                    RenderSystems::Edit,
                )
                    .chain(),
            )
            .add_systems(TranscriptRender, search_product_look.in_set(RenderSystems::Render))
            .add_systems(TranscriptRender, order_look.in_set(RenderSystems::Render))
            .add_systems(PreStartup, build_rows);
    }
}

/// Fills a batch, runs rig-tui's render schedule once, and keeps the look lines as rows.
fn build_rows(world: &mut World) {
    let batch = demo_batch();
    world.insert_resource(batch);
    world.run_schedule(TranscriptRender);

    let mut rows: Vec<Row> = Vec::new();
    {
        let batch = world.resource::<RenderBatch>();
        let mut keys: Vec<&RenderKey> = batch.requests.keys().collect();
        keys.sort_by_key(|key| match key {
            RenderKey::Tool(message, block) => (*message as u64) * 1000 + *block as u64,
            _ => u64::MAX,
        });
        for key in keys {
            if let Some(request) = batch.requests.get(key) {
                for line in &request.output.lines {
                    rows.push(Row::from_look(line));
                }
                rows.push(Row::new(Line::from("")));
            }
        }
    }
    let count = rows.len();
    world.insert_resource(BatchRows(rows));
    bevy::log::info!("[batch] built {count} rows from rig-tui RenderBatch");
}
