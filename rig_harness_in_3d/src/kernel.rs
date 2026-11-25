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
///
/// 拒绝有两种：选项里没有"确认"（例如 `[2] 取消` 的 Answer），以及**人直接
/// 拒绝问题本身**（Esc / Alt+0 走 [`rig_niri::editor::refuse_ask`]，不产生
/// `Answer`，只有 `Outcome::refused`）。后一种是键盘作答新增的路径，但它和
/// `[2] 取消` 是同一个用户意图——取消被这个闸门按住的下单——所以这里一并
/// 把被按住的调用拒绝掉；否则问题被拒、hold 释放后，没决定的调用会照常开跑，
/// 下单反而通过了（见 `refusing_the_ask_cancels_the_held_order`）。
///
/// 只看 `Decider::Person` 的拒绝：turn 被停、问题被别的插件取消等没有人的
/// 决定时，这里不动被按住的调用。
fn answer_to_decision(
    asks: Query<(&Ask, &Outcome, &HoldOf, &Decision), Added<Outcome>>,
    // Ask 自己的 `Decision` 在 `asks` 里只读；这里只写被按住的调用（不是 Ask），
    // `Without<Ask>` 让两个查询在 Bevy 看来互斥。
    mut decisions: Query<&mut Decision, Without<Ask>>,
) {
    for (ask, outcome, hold, answered) in asks.iter() {
        if let Some(answer) = Answer::of(outcome) {
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
        } else if matches!(answered.by(), Some(Decider::Person))
            && let Ok(mut decision) = decisions.get_mut(hold.0)
            && decision.decide(
                Decider::Person,
                Outcome::refused("user closed the question"),
            )
        {
            info!("[gate] question refused by person: {}", ask.title);
        }
    }
}

/// 前端：把用户点的选项写进 `Decision`（先到先得）。
///
/// 实际作答落在 [`rig_niri::editor::answer_ask`]：键盘 Alt+1..9 走的是同一个
/// 函数，所以点击和键盘产生完全相同的 rig-harness 效果，不维护两份语义。
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
    if rig_niri::editor::answer_ask(choice.ask, &choice.choice, &mut decisions) {
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

#[cfg(test)]
mod tests {
    //! 作答路径的回归：点击确认行与键盘 Alt+1 必须落到同一个 rig-harness
    //! 效果（Ask 的 `Decision` 上是同一个 `Answer`、同是 `Decider::Person`）。
    //! 无窗口：不渲染、不截图，断言全在内核状态上。

    use super::*;
    use bevy::app::TaskPoolPlugin;
    use bevy::camera::NormalizedRenderTarget;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::picking::backend::HitData;
    use bevy::picking::events::{Pointer, PointerClick};
    use bevy::picking::pointer::{Location, PointerButton, PointerId};
    use rig_niri::editor::EditorPlugin;
    use rig_niri::panels::PanelRows;
    use rig_niri::theme::Theme;
    use rig_niri::transcript::AskChoice;

    /// 点击观察者 + 键盘输入都装上的最小 App（真内核，不建窗口）。
    fn answer_app() -> App {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.add_plugins(rig_ecs::AgentPlugin);
        app.add_plugins(EditorPlugin);
        app.init_resource::<Theme>()
            .init_resource::<PanelRows>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_message::<KeyboardInput>();
        // 和 `KernelPlugin` 同一个观察者、同一条回答路径。
        app.add_observer(answer_ask);
        app
    }

    /// spawn 一个两选项的确认问题，等内核挂上 `Decision`。
    fn spawn_ask(app: &mut App) -> Entity {
        let ask = app
            .world_mut()
            .spawn(
                Ask::new("确认下单并支付？")
                    .body("商品：Nike Air Zoom Pegasus 40\n合计：469")
                    .choice("confirm", "确认支付")
                    .choice("cancel", "取消"),
            )
            .id();
        app.update();
        assert!(
            app.world().get::<Decision>(ask).is_some(),
            "Ask 的 Effect 应带上 Decision"
        );
        ask
    }

    /// 像真实点击那样：行实体挂 [`AskChoice`]，点它内部的文字子实体，让观察者
    /// 从子节点往上找到行。
    fn click_choice(app: &mut App, ask: Entity, choice: &str) {
        let row = app
            .world_mut()
            .spawn(AskChoice {
                ask,
                choice: choice.to_owned(),
            })
            .id();
        let text = app.world_mut().spawn(ChildOf(row)).id();
        app.world_mut().entity_mut(text).trigger(|entity| PointerClick {
            entity,
            pointer: Pointer::new(
                PointerId::default(),
                Location {
                    target: NormalizedRenderTarget::None {
                        width: 1,
                        height: 1,
                    },
                    position: Vec2::ZERO,
                },
            ),
            button: PointerButton::Primary,
            hit: HitData {
                camera: Entity::PLACEHOLDER,
                depth: 0.0,
                position: None,
                normal: None,
                extra: None,
            },
            duration: std::time::Duration::ZERO,
            count: 1,
        });
    }

    /// 用户按 Alt+1。
    fn alt_one(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::AltLeft);
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Digit1,
            logical_key: Key::Character("1".into()),
            state: ButtonState::Pressed,
            text: Some("1".into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    /// Ask 收到的选项答案。
    fn answer_of(app: &App, ask: Entity) -> Option<Answer> {
        app.world()
            .get::<Decision>(ask)
            .and_then(Decision::get)
            .and_then(Answer::of)
    }

    fn answered_by_person(app: &App, ask: Entity) -> bool {
        app.world()
            .get::<Decision>(ask)
            .is_some_and(|decision| matches!(decision.by(), Some(Decider::Person)))
    }

    #[test]
    fn a_click_and_alt_one_make_the_same_rig_harness_answer() {
        // 点击路径：真的经 `answer_ask` 观察者、从行内子节点往上找 `AskChoice`。
        let mut clicked = answer_app();
        let clicked_ask = spawn_ask(&mut clicked);
        click_choice(&mut clicked, clicked_ask, "confirm");
        let click_answer = answer_of(&clicked, clicked_ask).expect("点击应决定 Ask");
        assert!(answered_by_person(&clicked, clicked_ask));

        // 键盘路径：真的按 Alt+1（内联绑定在有问题时启用）。
        let mut typed = answer_app();
        let typed_ask = spawn_ask(&mut typed);
        alt_one(&mut typed);
        let key_answer = answer_of(&typed, typed_ask).expect("Alt+1 应决定 Ask");
        assert!(answered_by_person(&typed, typed_ask));

        assert_eq!(click_answer, Answer::choice("confirm"), "点击答的是选项 id");
        assert_eq!(
            click_answer, key_answer,
            "点击与键盘必须产生完全相同的 rig-harness 效果"
        );
    }

    #[test]
    fn a_click_can_answer_any_choice_and_alt_one_the_first() {
        let mut clicked = answer_app();
        let ask = spawn_ask(&mut clicked);
        click_choice(&mut clicked, ask, "cancel");
        assert_eq!(answer_of(&clicked, ask), Some(Answer::choice("cancel")));

        let mut typed = answer_app();
        let ask = spawn_ask(&mut typed);
        alt_one(&mut typed);
        assert_eq!(answer_of(&typed, ask), Some(Answer::choice("confirm")));
    }

    /// 拒绝问题必须取消被闸门按住的下单：Alt+0 / Esc 的 `Outcome::refused`
    /// 没有 `Answer`，应用层也要把 hold 住的调用拒掉，否则 hold 释放后它会照常
    /// 开跑（下单反而成功）。这条断言的是**内核记录**：`create_order` 在 Start
    /// 之前就被拒（`started_ms` 为 `None`），不是只断言函数被调用。
    #[test]
    fn refusing_the_ask_cancels_the_held_order() {
        /// 每个工具调用的内核记录：名字 / 是否开跑过 / 是否被拒。
        #[derive(Resource, Default)]
        struct Records(Vec<(String, bool, bool)>);

        let mut app = answer_app();
        app.add_plugins(KernelPlugin);
        app.init_resource::<Records>().add_systems(
            Update,
            |mut records: MessageReader<EffectRecorded>, mut seen: ResMut<Records>| {
                for EffectRecorded(record) in records.read() {
                    if let rig_core::effect::EffectKind::ToolCall { name, .. } = &record.kind {
                        seen.0.push((
                            name.clone(),
                            record.started_ms.is_some(),
                            record.outcome.is_err(),
                        ));
                    }
                }
            },
        );
        for _ in 0..8 {
            app.update();
        }
        let ask = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, (With<Ask>, Without<Outcome>)>();
            query.iter(world).next().expect("闸门应问用户")
        };

        // Alt+0 = 拒绝问题本身（不产生 Answer）。
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::AltLeft);
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Digit0,
            logical_key: Key::Character("0".into()),
            state: ButtonState::Pressed,
            text: Some("0".into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(
            app.world()
                .get::<Decision>(ask)
                .is_some_and(Decision::is_decided),
            "Alt+0 应拒绝问题"
        );
        assert!(matches!(
            app.world().get::<Decision>(ask).and_then(Decision::by),
            Some(Decider::Person)
        ));

        for _ in 0..10 {
            app.update();
        }
        let records = app.world().resource::<Records>();
        let order = records
            .0
            .iter()
            .find(|(name, ..)| name == "create_order")
            .unwrap_or_else(|| panic!("应有 create_order 的内核记录：{:?}", records.0));
        assert!(
            !order.1,
            "拒绝问题后下单不能开跑（started 必须是 None）：{:?}",
            records.0
        );
        assert!(
            order.2,
            "被按住的下单应被拒绝（记录是 Err）：{:?}",
            records.0
        );
    }
}
