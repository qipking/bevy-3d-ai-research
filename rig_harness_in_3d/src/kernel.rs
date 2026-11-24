//! The kernel side: a real `rig_ecs` app with two business tools.
//!
//! No model is needed to exercise the tool path: a `ToolRequest` effect goes through the
//! kernel's stages on its own (`Admit → Prepare → Gate → Start → Arrive → Review → Commit`),
//! which is also where the payment gate hooks in.

use std::future::Future;

use bevy::prelude::*;
use rig_core::message::{CallId, ToolCall, ToolFunction, ToolName};
use rig_core::completion::request::ToolAnnotations;
use rig_core::tool::{Tool, ToolContext};
use rig_core::effect::Decider;
use rig_ecs::prelude::*;
use rig_ecs::tools::AppToolsExt;
use rig_harness::prelude::{Answer, Answerer, Answerers, Ask};
use serde::Deserialize;

/// The one agent this demo runs.
#[derive(Resource)]
pub struct MainAgent(pub Entity);

/// Catalog the mock backend serves.
fn catalog() -> serde_json::Value {
    serde_json::json!([
        {"id":"p-1001","name":"Nike Air Zoom Pegasus 40","price":469,"shop":"官方旗舰店","rating":4.8,"stock":12},
        {"id":"p-1002","name":"ASICS Gel-Kayano 30","price":499,"shop":"跑步装备店","rating":4.7,"stock":5},
        {"id":"p-1003","name":"Li-Ning 赤兔 7 Pro","price":439,"shop":"李宁官方","rating":4.6,"stock":30}
    ])
}

#[derive(Deserialize)]
pub struct SearchArgs {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub max: Option<i64>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `search_products`: a read-only tool, so the kernel may run it beside others.
pub struct SearchProducts;

impl Tool for SearchProducts {
    const NAME: &'static str = "search_products";
    type Args = SearchArgs;
    type Output = String;
    type Error = std::io::Error;

    fn description(&self) -> String {
        "Search the catalog by text, optionally under a price ceiling.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "what to look for"},
                "max": {"type": "integer", "description": "price ceiling"},
                "limit": {"type": "integer", "description": "how many to return"}
            },
            "required": ["query"]
        })
    }

    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations {
            read_only_hint: Some(true),
            open_world_hint: Some(false),
            ..default()
        }
    }

    fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> + Send {
        async move {
            // A real implementation would await an HTTP client here; the kernel already runs
            // this future on Bevy's async compute pool, off the main thread.
            let mut items = catalog();
            if let Some(max) = args.max {
                if let Some(list) = items.as_array_mut() {
                    list.retain(|item| item["price"].as_i64().unwrap_or(0) <= max);
                }
            }
            if let Some(limit) = args.limit {
                if let Some(list) = items.as_array_mut() {
                    list.truncate(limit as usize);
                }
            }
            Ok(items.to_string())
        }
    }
}

#[derive(Deserialize)]
pub struct CreateOrderArgs {
    #[serde(default)]
    pub product_id: String,
    #[serde(default)]
    pub qty: Option<u32>,
}

/// `create_order`: a writing tool; the payment gate holds it until a person confirms.
pub struct CreateOrder;

impl Tool for CreateOrder {
    const NAME: &'static str = "create_order";
    type Args = CreateOrderArgs;
    type Output = String;
    type Error = std::io::Error;

    fn description(&self) -> String {
        "Place the order for a product. Needs the user's confirmation before it runs.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "product_id": {"type": "string"},
                "qty": {"type": "integer"}
            },
            "required": ["product_id"]
        })
    }

    fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> + Send {
        async move {
            let qty = args.qty.unwrap_or(1);
            let unit = catalog()
                .as_array()
                .and_then(|list| {
                    list.iter()
                        .find(|item| item["id"].as_str() == Some(args.product_id.as_str()))
                        .cloned()
                })
                .unwrap_or_else(|| serde_json::json!({"id": args.product_id, "name": "?", "price": 0, "shop": "?"}));
            Ok(serde_json::json!({
                "order_id": "ord_20261123_0001",
                "line": unit,
                "qty": qty,
                "total": unit["price"].as_i64().unwrap_or(0) * qty as i64,
                "status": "draft"
            })
            .to_string())
        }
    }
}

fn tool_call(name: &'static str, args: &str) -> ToolCall {
    let name = ToolName::new(name).expect("non-empty name");
    ToolCall::new(CallId::from_wire("call_0"), ToolFunction::parse(name, args))
}

/// Spawns the agent and the two tool calls the demo transcript shows.
pub fn spawn_kernel(world: &mut World) {
    let agent = rig_ecs::spawn::spawn_root(world, AgentId("main".into())).id();
    world.insert_resource(MainAgent(agent));
    // 这个前端能回答 Ask（闸门靠它判断"有没有人能问"）
    world.spawn(Answerer { agent: Some(agent) });

    // 1) 只读搜索：内核直接跑
    world.spawn((
        ToolRequest {
            call: tool_call("search_products", r#"{"query":"跑鞋","max":500,"limit":3}"#),
        },
        ForAgent(agent),
    ));
    // 2) 下单：会被 Gate 按住，等用户确认
    world.spawn((
        ToolRequest {
            call: tool_call("create_order", r#"{"product_id":"p-1001","qty":1}"#),
        },
        ForAgent(agent),
    ));
}

/// 已经为这个调用问过用户（避免每个 pass 重复提问）。
#[derive(Component)]
pub struct AskAsked;

/// 支付闸门：`create_order` 在 Gate 阶段被按住，问用户要确认。
fn payment_gate(
    mut effects: Query<(&ToolRequest, &mut Decision, Entity), ReadyToStart>,
    holds: Query<&Holds>,
    asked: Query<(), With<AskAsked>>,
    answerers: Answerers,
    agent: Res<MainAgent>,
    mut commands: Commands,
) {
    for (request, mut decision, effect) in effects.iter_mut() {
        if request.call.function.name.as_str() != "create_order"
            || decision.is_decided()
            || asked.contains(effect)
        {
            continue;
        }
        // 已经为它问过（有 hold）就别每帧再问一次
        if holds.get(effect).map(|h| h.len()).unwrap_or(0) > 0 {
            continue;
        }
        if !answerers.can_answer(Some(agent.0)) {
            decision.decide(
                Decider::plugin("pay-gate"),
                Outcome::refused("no front to confirm the payment"),
            );
            continue;
        }
        let ask = Ask::new("确认下单并支付？")
            .body(order_summary(&request.call))
            .choice("confirm", "确认支付")
            .choice("cancel", "取消");
        info!("[gate] holding {:?} for confirmation", request.call.id);
        commands.entity(effect).insert(AskAsked);
        commands.spawn((ask, ForAgent(agent.0), HoldOf(effect)));
    }
}

/// 订单摘要：商品 / 单价 / 数量 / 合计 / 店铺（闸门要让用户点头的那几项）。
fn order_summary(call: &ToolCall) -> String {
    let product_id = call.function.arguments.get("product_id").and_then(|v| v.as_str()).unwrap_or("");
    let qty = call.function.arguments.get("qty").and_then(|v| v.as_u64()).unwrap_or(1);
    let item = catalog()
        .as_array()
        .and_then(|list| list.iter().find(|i| i["id"].as_str() == Some(product_id)).cloned())
        .unwrap_or_else(|| serde_json::json!({"name": product_id, "price": 0, "shop": "?"}));
    let unit = item["price"].as_i64().unwrap_or(0);
    format!(
        "商品：{}\n单价：{}  数量：{}\n合计：{}\n店铺：{}",
        item["name"].as_str().unwrap_or("?"),
        unit,
        qty,
        unit * qty as i64,
        item["shop"].as_str().unwrap_or("?")
    )
}

/// 已答复的 Ask：确认就让它继续（hold 由 Ask 结算时释放），否则拒绝被按住的调用。
fn answer_to_decision(
    asks: Query<(&Ask, &Outcome, &HoldOf), Added<Outcome>>,
    mut decisions: Query<&mut Decision>,
) {
    for (ask, outcome, hold) in asks.iter() {
        let Some(answer) = Answer::of(outcome) else { continue };
        if answer.is("confirm") {
            info!("[gate] confirmed: {}", ask.title);
        } else if let Ok(mut decision) = decisions.get_mut(hold.0)
            && decision.decide(
                Decider::Person,
                Outcome::refused("user cancelled the order"),
            )
        {
            info!("[gate] refused by person: {}", ask.title);
        }
    }
}

/// 前端：把用户点的选项写进 `Decision`（先到先得）。
fn answer_ask(
    click: On<bevy::picking::events::PointerClick>,
    choices: Query<&rig_niri::transcript::AskChoice>,
    parents: Query<&ChildOf>,
    mut decisions: Query<&mut Decision>,
) {
    let mut entity = click.entity;
    let choice = loop {
        if let Ok(choice) = choices.get(entity) {
            break Some(choice.clone());
        }
        match parents.get(entity) {
            Ok(child_of) => entity = child_of.parent(),
            Err(_) => break None,
        }
    };
    let Some(choice) = choice else { return };
    if let Ok(mut decision) = decisions.get_mut(choice.ask)
        && decision.decide(
            Decider::Person,
            Answer::choice(choice.choice.clone()).outcome(),
        )
    {
        info!("[gate] person answered {:?} on {:?}", choice.choice, choice.ask);
    }
}

pub struct KernelPlugin;

impl Plugin for KernelPlugin {
    fn build(&self, app: &mut App) {
        // 内核插件（AgentPlugin / AskPlugin / PickPlugin）由 rig_harness_in_3d::app() 装好，
        // 这里只注册业务工具与闸门。
        app.add_tool(SearchProducts)
            .add_tool(CreateOrder)
            .add_systems(PreStartup, spawn_kernel)
            .add_systems(
                AgentPass,
                (answer_to_decision, payment_gate).chain().in_set(Stage::Gate),
            )
            .add_observer(answer_ask);
    }
}
