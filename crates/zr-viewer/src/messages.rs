//! In-game text messages: the `InGameUI` `m_uiMessages` queue drawn at the top-left.
//!
//! Ported from `GeneralsMD/.../GameClient/InGameUI.cpp`: `addMessageText` (newest at index 0,
//! the array shifts and the oldest falls off at `MAX_UI_MESSAGES`), the alternating
//! `MessageColor1` / `MessageColor2` assignment, the `postDraw` layout (oldest at the top, each
//! line one font-height down) and the expiry/fade in `update` (`MessageDelayMS` then an alpha
//! decrement of `(frame - timestamp) * 0.01` per frame, accumulating). The `InGameUI.ini` fields
//! (`MessagePosition` / `MessageColor1` / `MessageColor2` / `MessagePointSize` / `MessageBold` /
//! `MessageDelayMS`) are read from the merged INI; the TheSuperHackers `validate()` bugfix
//! (`MessageDelayMS == 75000 -> 7500`, `#if ENABLE_GUI_HACKS`) is applied.
//!
//! The client only reads sim state; the queue is presentation. Other client systems request a
//! line through [`MessageRequests`], which is drained once per frame.

use bevy::prelude::*;

use crate::sim_view::SimRes;

/// `InGameUI::MAX_UI_MESSAGES`.
const MAX_UI_MESSAGES: usize = 6;

/// One requested line: already-resolved text and an optional explicit colour
/// (`InGameUI::messageColor`).
#[derive(Clone, Debug)]
pub struct MessageRequest {
    pub text: String,
    pub color: Option<[u8; 4]>,
}

/// Client systems push messages here; [`messages_update`] drains them.
#[derive(Resource, Default)]
pub struct MessageRequests(pub Vec<MessageRequest>);

impl MessageRequests {
    /// `TheInGameUI->message( label )`: resolve a `GameText` label (no format args).
    pub fn push_label(&mut self, sim: &SimRes, label: &str) {
        self.0.push(MessageRequest { text: resolve(sim, label), color: None });
    }

    /// `TheInGameUI->message( format, arg )`: substitute the first `%s`/`%d` with `arg`.
    pub fn push_formatted(&mut self, sim: &SimRes, label: &str, arg: &str) {
        let text = resolve(sim, label).replacen("%s", arg, 1).replacen("%d", arg, 1);
        self.0.push(MessageRequest { text, color: None });
    }
}

fn resolve(sim: &SimRes, label: &str) -> String {
    sim.game.text().get(label).unwrap_or(label).to_owned()
}

/// One live line in the queue.
struct Line {
    text: String,
    timestamp: u32,
    /// `Color` alpha (0..255), faded after the timeout.
    alpha: f32,
    color: [u8; 4],
}

/// `InGameUI.ini` fields (with the C++ constructor defaults).
#[derive(Clone, Copy)]
struct MsgConfig {
    pos: (f32, f32),
    color1: [u8; 4],
    color2: [u8; 4],
    point_size: f32,
    bold: bool,
    delay_ms: i32,
}

impl Default for MsgConfig {
    fn default() -> Self {
        MsgConfig { pos: (10.0, 10.0), color1: [255, 255, 255, 255], color2: [180, 180, 180, 255], point_size: 10.0, bold: false, delay_ms: 5000 }
    }
}

/// The message queue and its built line entities.
#[derive(Resource)]
pub struct MessageQueue {
    lines: Vec<Line>,
    cfg: MsgConfig,
    /// `MessageDelayMS` in logic frames (`ConvertDurationFromMsecsToFrames`, ceil).
    timeout_frames: u32,
    entities: Vec<Entity>,
    built: bool,
}

impl Default for MessageQueue {
    fn default() -> Self {
        MessageQueue { lines: Vec::new(), cfg: MsgConfig::default(), timeout_frames: 150, entities: Vec::new(), built: false }
    }
}

#[derive(Component)]
struct MessageLine;

pub fn add_systems(app: &mut App) {
    app.init_resource::<MessageRequests>();
    app.init_resource::<MessageQueue>();
    app.add_systems(OnEnter(crate::shell::ShellState::InGame), setup_messages.after(crate::hud::setup_hud));
    app.add_systems(
        Update,
        messages_update.after(crate::sim_view::sim_step).run_if(in_state(crate::shell::ShellState::InGame)),
    );
}

/// Reads `InGameUI.ini` and spawns the (hidden) line text nodes.
fn setup_messages(
    mut commands: Commands,
    mut queue: ResMut<MessageQueue>,
    hud: Option<Res<crate::hud::HudData>>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    let Some(hud) = hud else { return };
    let mut cfg = MsgConfig::default();
    if let Some(b) = hud.ini.get("InGameUI", "") {
        if let Some(v) = b.value("MessagePosition") {
            cfg.pos = parse_xy(v).unwrap_or(cfg.pos);
        }
        if let Some(v) = b.value("MessageColor1") {
            cfg.color1 = parse_rgb(v).unwrap_or(cfg.color1);
        }
        if let Some(v) = b.value("MessageColor2") {
            cfg.color2 = parse_rgb(v).unwrap_or(cfg.color2);
        }
        if let Some(v) = b.value("MessagePointSize") {
            cfg.point_size = v.trim().parse().unwrap_or(cfg.point_size);
        }
        if let Some(v) = b.value("MessageBold") {
            cfg.bold = matches!(v.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "1");
        }
        if let Some(v) = b.value("MessageDelayMS") {
            cfg.delay_ms = v.trim().parse().unwrap_or(cfg.delay_ms);
        }
    }
    // TheSuperHackers `InGameUI::validate` bugfix (`#if ENABLE_GUI_HACKS`).
    if cfg.delay_ms == 75000 {
        cfg.delay_ms = 7500;
    }
    cfg.delay_ms = cfg.delay_ms.max(0);
    // The engine's font is scaled from the 800x600 creation resolution.
    let (win_w, win_h) = window.single().map(|w| (w.width(), w.height())).unwrap_or((1600.0, 900.0));
    let scale = (win_h / 600.0).max(1.0);
    let size = (cfg.point_size * scale).clamp(10.0, 26.0);
    let line_h = size * 1.15;
    let (x, y) = (cfg.pos.0 * scale, cfg.pos.1 * scale);
    queue.cfg = cfg;
    queue.timeout_frames = ms_to_frames(cfg.delay_ms);
    queue.lines.clear();
    queue.entities.clear();
    for i in 0..MAX_UI_MESSAGES {
        let e = commands
            .spawn((
                MessageLine,
                bevy::picking::Pickable::IGNORE,
                Text::new(""),
                TextFont { font_size: FontSize::Px(size), ..default() },
                TextColor(Color::NONE),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(x),
                    top: Val::Px(y + i as f32 * line_h),
                    ..default()
                },
                ZIndex(400),
                Visibility::Hidden,
            ))
            .id();
        queue.entities.push(e);
    }
    let _ = win_w;
    queue.built = true;
}

fn ms_to_frames(ms: i32) -> u32 {
    ((ms as f32) * (zr_sim::LOGICFRAMES_PER_SECOND as f32 / 1000.0)).ceil() as u32
}

/// `R:%d G:%d B:%d` / `R:%d G:%d B:%d A:%d` (`INI::parseColorInt` style); alpha defaults to 255.
fn parse_rgb(v: &str) -> Option<[u8; 4]> {
    let mut c = [255u8; 4];
    let mut seen = false;
    for tok in v.split_whitespace() {
        let (k, val) = tok.split_once(':')?;
        let val: i32 = val.parse().ok()?;
        let val = val.clamp(0, 255) as u8;
        match k.to_ascii_lowercase().as_str() {
            "r" => c[0] = val,
            "g" => c[1] = val,
            "b" => c[2] = val,
            "a" => c[3] = val,
            _ => return None,
        }
        seen = true;
    }
    seen.then_some(c)
}

/// `X:%d Y:%d`.
fn parse_xy(v: &str) -> Option<(f32, f32)> {
    let mut out = (0.0f32, 0.0f32);
    let mut seen = false;
    for tok in v.split_whitespace() {
        let (k, val) = tok.split_once(':')?;
        let val: f32 = val.parse().ok()?;
        match k.to_ascii_lowercase().as_str() {
            "x" => out.0 = val,
            "y" => out.1 = val,
            _ => return None,
        }
        seen = true;
    }
    seen.then_some(out)
}

fn color(c: [u8; 4], alpha: f32) -> Color {
    Color::srgba(c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, (alpha / 255.0).clamp(0.0, 1.0))
}

/// Drains requests, ages the lines and re-lays the text nodes.
fn messages_update(
    mut queue: ResMut<MessageQueue>,
    mut requests: ResMut<MessageRequests>,
    sim: Option<NonSend<SimRes>>,
    mut texts: Query<(&mut Text, &mut TextColor, &mut Visibility), With<MessageLine>>,
) {
    if !queue.built {
        return;
    }
    let frame = sim.as_ref().map_or(0, |s| s.game.frame());
    let cfg = queue.cfg;
    let timeout = queue.timeout_frames;

    for req in requests.0.drain(..) {
        // `addMessageText`: the newest goes to index 0, the oldest falls off the end.
        // `MessageColor`: if the next line already has `color2`, use `color1`, else `color2`.
        let color1 = req.color.unwrap_or(cfg.color1);
        let color2 = req.color.unwrap_or(cfg.color2);
        let use_first = queue.lines.first().is_none_or(|l| l.color == color2);
        let chosen = if use_first { color1 } else { color2 };
        if queue.lines.len() >= MAX_UI_MESSAGES {
            queue.lines.pop();
        }
        queue.lines.insert(0, Line { text: req.text, timestamp: frame, alpha: 255.0, color: chosen });
    }

    // Expiry / fade (`InGameUI::update`).
    queue.lines.retain_mut(|l| {
        let dt = frame.wrapping_sub(l.timestamp);
        if dt > timeout {
            let amount = (dt as f32 * 0.01) as i32 as f32;
            l.alpha = (l.alpha - amount).max(0.0);
        }
        l.alpha > 0.0
    });

    // `postDraw`: index `MAX-1` (oldest) at the top, newest at the bottom.
    let n = queue.lines.len();
    let ents = queue.entities.clone();
    for (slot, &e) in ents.iter().enumerate() {
        let Ok((mut text, mut tcolor, mut vis)) = texts.get_mut(e) else { continue };
        if slot < n {
            // `slot` counts from the top; the oldest line is `lines[n-1]`.
            let line = &queue.lines[n - 1 - slot];
            **text = line.text.clone();
            *tcolor = TextColor(color(line.color, line.alpha));
            *vis = Visibility::Inherited;
        } else {
            **text = String::new();
            *vis = Visibility::Hidden;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_to_frames_matches_engine() {
        assert_eq!(ms_to_frames(5000), 150);
        assert_eq!(ms_to_frames(7500), 225);
        assert_eq!(ms_to_frames(1000), 30);
        assert_eq!(ms_to_frames(0), 0);
        assert_eq!(ms_to_frames(500), 15);
    }

    #[test]
    fn parses_ingameui_colors() {
        assert_eq!(parse_rgb("R:255 G:255 B:255"), Some([255, 255, 255, 255]));
        assert_eq!(parse_rgb("R:180 G:255 B:255"), Some([180, 255, 255, 255]));
        assert_eq!(parse_rgb("R:1 G:2 B:3 A:4"), Some([1, 2, 3, 4]));
        assert_eq!(parse_rgb("garbage"), None);
    }

    #[test]
    fn parses_position() {
        assert_eq!(parse_xy("X:10 Y:10"), Some((10.0, 10.0)));
        assert_eq!(parse_xy("nope"), None);
    }
}
