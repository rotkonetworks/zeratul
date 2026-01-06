//! 2d poker table renderer
//!
//! minimal, clean 2d table view optimized for multi-tabling
//! - oval table with player positions
//! - card display (hole cards + community)
//! - action buttons with hotkeys
//! - bet sizing slider
//! - player stacks and current bets

use bevy_egui::egui;

/// card suit
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suit {
    Hearts,
    Diamonds,
    Clubs,
    Spades,
}

impl Suit {
    pub fn symbol(&self) -> &'static str {
        match self {
            Suit::Hearts => "♥",
            Suit::Diamonds => "♦",
            Suit::Clubs => "♣",
            Suit::Spades => "♠",
        }
    }

    pub fn color(&self) -> egui::Color32 {
        match self {
            Suit::Hearts | Suit::Diamonds => egui::Color32::from_rgb(220, 50, 50),
            Suit::Clubs | Suit::Spades => egui::Color32::from_rgb(30, 30, 30),
        }
    }
}

/// card rank
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rank {
    Two, Three, Four, Five, Six, Seven, Eight, Nine, Ten,
    Jack, Queen, King, Ace,
}

impl Rank {
    pub fn symbol(&self) -> &'static str {
        match self {
            Rank::Two => "2",
            Rank::Three => "3",
            Rank::Four => "4",
            Rank::Five => "5",
            Rank::Six => "6",
            Rank::Seven => "7",
            Rank::Eight => "8",
            Rank::Nine => "9",
            Rank::Ten => "10",
            Rank::Jack => "J",
            Rank::Queen => "Q",
            Rank::King => "K",
            Rank::Ace => "A",
        }
    }
}

/// a playing card
#[derive(Clone, Copy, Debug)]
pub struct Card {
    pub rank: Rank,
    pub suit: Suit,
}

impl Card {
    pub fn new(rank: Rank, suit: Suit) -> Self {
        Self { rank, suit }
    }
}

/// player at table
#[derive(Clone, Debug)]
pub struct Player {
    pub name: String,
    pub stack: u64,
    pub current_bet: u64,
    pub is_folded: bool,
    pub is_all_in: bool,
    pub is_dealer: bool,
    pub is_sb: bool,
    pub is_bb: bool,
    pub is_active: bool,
    pub hole_cards: Option<(Card, Card)>,
    pub show_cards: bool,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            name: String::new(),
            stack: 0,
            current_bet: 0,
            is_folded: false,
            is_all_in: false,
            is_dealer: false,
            is_sb: false,
            is_bb: false,
            is_active: false,
            hole_cards: None,
            show_cards: false,
        }
    }
}

/// table state for rendering
#[derive(Clone, Debug, Default)]
pub struct TableState {
    pub players: Vec<Option<Player>>,
    pub community_cards: Vec<Card>,
    pub pot: u64,
    pub side_pots: Vec<u64>,
    pub current_bet: u64,
    pub min_raise: u64,
    pub our_seat: usize,
    pub is_our_turn: bool,
    pub time_remaining: f32,
    pub phase: GamePhase,
    /// animation state
    pub animation: AnimationState,
}

/// animation state for table
#[derive(Clone, Debug, Default)]
pub struct AnimationState {
    /// current animation type
    pub kind: AnimationKind,
    /// animation progress (0.0 - 1.0)
    pub progress: f32,
    /// cards being dealt (for deal animation)
    pub dealing_to: Option<usize>,
    /// shuffle rotation angles per card
    pub shuffle_rotations: Vec<f32>,
}

/// animation types
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnimationKind {
    #[default]
    None,
    Shuffle,
    Deal,
    Flip,
    ChipsMove,
}

/// game phase
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GamePhase {
    #[default]
    Waiting,
    Preflop,
    Flop,
    Turn,
    River,
    Showdown,
}

/// action from player
#[derive(Clone, Debug)]
pub enum PlayerAction {
    Fold,
    Check,
    Call(u64),
    Bet(u64),
    Raise(u64),
    AllIn,
}

/// render full table with all elements
pub fn render_table(ui: &mut egui::Ui, state: &TableState, rect: egui::Rect) -> Option<PlayerAction> {
    let mut action = None;

    // table area
    let table_rect = egui::Rect::from_min_size(
        rect.min,
        egui::vec2(rect.width(), rect.height() - 60.0), // leave room for action bar
    );

    // draw table felt (rounded rect to simulate oval)
    let painter = ui.painter();
    let center = table_rect.center();
    let radius = egui::vec2(table_rect.width() * 0.45, table_rect.height() * 0.4);

    // use rounded rect with high rounding for oval effect
    let table_felt = egui::Rect::from_center_size(center, radius * 2.0);
    let rounding = egui::Rounding::same(radius.y.min(radius.x) * 0.8);

    painter.rect_filled(
        table_felt,
        rounding,
        egui::Color32::from_rgb(35, 90, 35), // dark green felt
    );
    painter.rect_stroke(
        table_felt,
        rounding,
        egui::Stroke::new(3.0, egui::Color32::from_rgb(80, 50, 20)), // wood border
    );

    // render pot in center
    render_pot(painter, center, state.pot, &state.side_pots);

    // render community cards
    let card_y = center.y - 15.0;
    render_community_cards(painter, center.x, card_y, &state.community_cards);

    // render players around table
    render_players(painter, center, radius, &state.players, state.our_seat);

    // action bar at bottom
    if state.is_our_turn {
        let action_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x, rect.max.y - 55.0),
            egui::vec2(rect.width(), 50.0),
        );

        action = render_action_bar(ui, action_rect, state);
    }

    action
}

/// render pot display with chip stacks
fn render_pot(painter: &egui::Painter, center: egui::Pos2, pot: u64, side_pots: &[u64]) {
    if pot > 0 {
        // chip stack visual
        render_chip_stack(painter, egui::pos2(center.x, center.y + 45.0), pot);

        // pot amount text below chips
        painter.text(
            egui::pos2(center.x, center.y + 65.0),
            egui::Align2::CENTER_CENTER,
            format_chips(pot),
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgb(255, 220, 100),
        );
    }

    // side pots (smaller, offset to sides)
    for (i, &side_pot) in side_pots.iter().enumerate() {
        let offset_x = if i % 2 == 0 { -60.0 } else { 60.0 };
        let offset_y = 45.0 + (i / 2) as f32 * 35.0;

        render_chip_stack(
            painter,
            egui::pos2(center.x + offset_x, center.y + offset_y),
            side_pot,
        );

        painter.text(
            egui::pos2(center.x + offset_x, center.y + offset_y + 18.0),
            egui::Align2::CENTER_CENTER,
            format_chips(side_pot),
            egui::FontId::proportional(9.0),
            egui::Color32::LIGHT_GRAY,
        );
    }
}

/// render community cards
fn render_community_cards(painter: &egui::Painter, cx: f32, cy: f32, cards: &[Card]) {
    let card_width = 32.0;
    let card_height = 44.0;
    let gap = 4.0;

    let total_width = 5.0 * card_width + 4.0 * gap;
    let start_x = cx - total_width / 2.0;

    for i in 0..5 {
        let x = start_x + i as f32 * (card_width + gap);
        let card_rect = egui::Rect::from_min_size(
            egui::pos2(x, cy - card_height / 2.0),
            egui::vec2(card_width, card_height),
        );

        if i < cards.len() {
            render_card(painter, card_rect, Some(&cards[i]));
        } else {
            // empty slot
            painter.rect_filled(
                card_rect,
                egui::Rounding::same(3.0),
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 30),
            );
        }
    }
}

/// render a single card - optimized for quick visual reads
fn render_card(painter: &egui::Painter, rect: egui::Rect, card: Option<&Card>) {
    match card {
        Some(c) => {
            // card background with slight shadow
            painter.rect_filled(
                rect.translate(egui::vec2(1.0, 1.0)),
                egui::Rounding::same(3.0),
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 40),
            );
            painter.rect_filled(
                rect,
                egui::Rounding::same(3.0),
                egui::Color32::WHITE,
            );
            painter.rect_stroke(
                rect,
                egui::Rounding::same(3.0),
                egui::Stroke::new(1.0, egui::Color32::from_gray(180)),
            );

            // determine sizes based on card rect
            let is_large = rect.height() > 40.0;
            let rank_size = if is_large { 14.0 } else { 11.0 };
            let suit_size = if is_large { 20.0 } else { 14.0 };

            // rank in top-left corner
            painter.text(
                egui::pos2(rect.left() + 3.0, rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                c.rank.symbol(),
                egui::FontId::proportional(rank_size),
                c.suit.color(),
            );

            // small suit under rank
            painter.text(
                egui::pos2(rect.left() + 3.0, rect.top() + rank_size + 1.0),
                egui::Align2::LEFT_TOP,
                c.suit.symbol(),
                egui::FontId::proportional(rank_size * 0.8),
                c.suit.color(),
            );

            // large center suit for instant visual recognition
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                c.suit.symbol(),
                egui::FontId::proportional(suit_size),
                c.suit.color(),
            );
        }
        None => {
            // face down card with pattern
            painter.rect_filled(
                rect.translate(egui::vec2(1.0, 1.0)),
                egui::Rounding::same(3.0),
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 40),
            );
            painter.rect_filled(
                rect,
                egui::Rounding::same(3.0),
                egui::Color32::from_rgb(30, 60, 130),
            );
            painter.rect_stroke(
                rect,
                egui::Rounding::same(3.0),
                egui::Stroke::new(1.0, egui::Color32::from_rgb(20, 40, 90)),
            );

            // card back pattern - inner border
            let inner = rect.shrink(3.0);
            painter.rect_stroke(
                inner,
                egui::Rounding::same(2.0),
                egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 100, 180)),
            );
        }
    }
}

/// render players around table
fn render_players(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: egui::Vec2,
    players: &[Option<Player>],
    our_seat: usize,
) {
    let seat_count = players.len().max(2);

    for (i, player_opt) in players.iter().enumerate() {
        // calculate position on ellipse
        // start from bottom (our seat) and go clockwise
        let seat_offset = (i as isize - our_seat as isize) as f32;
        let angle = std::f32::consts::PI / 2.0 + seat_offset * 2.0 * std::f32::consts::PI / seat_count as f32;

        let pos = egui::pos2(
            center.x + radius.x * 1.15 * angle.cos(),
            center.y + radius.y * 1.15 * angle.sin(),
        );

        if let Some(player) = player_opt {
            render_player(painter, pos, player, i == our_seat);
        } else {
            // empty seat
            painter.circle_stroke(
                pos,
                20.0,
                egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
            );
        }
    }
}

/// render single player with visual hierarchy for fast reads
fn render_player(painter: &egui::Painter, pos: egui::Pos2, player: &Player, is_us: bool) {
    let bg_color = if player.is_active {
        egui::Color32::from_rgb(60, 130, 60)
    } else if player.is_folded {
        egui::Color32::from_gray(35)
    } else if player.is_all_in {
        egui::Color32::from_rgb(140, 60, 60)
    } else {
        egui::Color32::from_gray(55)
    };

    // player box - slightly larger for better visibility
    let box_rect = egui::Rect::from_center_size(pos, egui::vec2(75.0, 48.0));

    // active player gets prominent glow effect
    if player.is_active {
        // outer glow
        painter.rect_filled(
            box_rect.expand(6.0),
            egui::Rounding::same(8.0),
            egui::Color32::from_rgba_unmultiplied(255, 220, 50, 80),
        );
        painter.rect_filled(
            box_rect.expand(3.0),
            egui::Rounding::same(6.0),
            egui::Color32::from_rgba_unmultiplied(255, 240, 100, 120),
        );
    }

    // main box
    painter.rect_filled(box_rect, egui::Rounding::same(4.0), bg_color);

    // border
    let border_color = if player.is_active {
        egui::Color32::from_rgb(255, 230, 50)
    } else if is_us {
        egui::Color32::from_rgb(100, 150, 255)
    } else {
        egui::Color32::from_gray(70)
    };
    painter.rect_stroke(
        box_rect,
        egui::Rounding::same(4.0),
        egui::Stroke::new(if player.is_active { 2.5 } else { 1.0 }, border_color),
    );

    // name - truncated for fit
    let display_name = if player.name.len() > 10 {
        format!("{}...", &player.name[..8])
    } else {
        player.name.clone()
    };
    let name_color = if player.is_folded {
        egui::Color32::GRAY
    } else {
        egui::Color32::WHITE
    };
    painter.text(
        egui::pos2(pos.x, pos.y - 14.0),
        egui::Align2::CENTER_CENTER,
        &display_name,
        egui::FontId::proportional(11.0),
        name_color,
    );

    // stack with color coding for quick read
    // green = healthy (20+ bb), yellow = medium (10-20), orange = short (5-10), red = critical (<5)
    let stack_color = if player.is_all_in {
        egui::Color32::from_rgb(255, 100, 100) // all-in red
    } else if player.stack > 2000 {
        egui::Color32::from_rgb(100, 220, 100) // healthy green
    } else if player.stack > 1000 {
        egui::Color32::from_rgb(220, 220, 100) // medium yellow
    } else if player.stack > 500 {
        egui::Color32::from_rgb(255, 180, 80)  // short orange
    } else {
        egui::Color32::from_rgb(255, 120, 80)  // critical
    };

    painter.text(
        egui::pos2(pos.x, pos.y + 4.0),
        egui::Align2::CENTER_CENTER,
        format_chips(player.stack),
        egui::FontId::proportional(13.0),
        stack_color,
    );

    // SB/BB indicators - small but visible
    if player.is_sb {
        painter.circle_filled(
            egui::pos2(box_rect.left() - 10.0, box_rect.center().y),
            7.0,
            egui::Color32::from_rgb(100, 100, 200),
        );
        painter.text(
            egui::pos2(box_rect.left() - 10.0, box_rect.center().y),
            egui::Align2::CENTER_CENTER,
            "S",
            egui::FontId::proportional(9.0),
            egui::Color32::WHITE,
        );
    }
    if player.is_bb {
        painter.circle_filled(
            egui::pos2(box_rect.left() - 10.0, box_rect.center().y),
            7.0,
            egui::Color32::from_rgb(200, 100, 100),
        );
        painter.text(
            egui::pos2(box_rect.left() - 10.0, box_rect.center().y),
            egui::Align2::CENTER_CENTER,
            "B",
            egui::FontId::proportional(9.0),
            egui::Color32::WHITE,
        );
    }

    // dealer button - prominent white disc with gold border
    if player.is_dealer {
        let btn_pos = egui::pos2(box_rect.right() + 12.0, box_rect.top() + 5.0);
        // glow
        painter.circle_filled(btn_pos, 12.0, egui::Color32::from_rgba_unmultiplied(255, 220, 100, 60));
        // button
        painter.circle_filled(btn_pos, 10.0, egui::Color32::WHITE);
        painter.circle_stroke(btn_pos, 10.0, egui::Stroke::new(2.0, egui::Color32::from_rgb(220, 180, 50)));
        painter.text(
            btn_pos,
            egui::Align2::CENTER_CENTER,
            "D",
            egui::FontId::proportional(12.0),
            egui::Color32::BLACK,
        );
    }

    // current bet with chip stack
    if player.current_bet > 0 {
        let bet_pos = egui::pos2(pos.x, pos.y + 40.0);
        render_chip_stack(painter, bet_pos, player.current_bet);

        // amount label
        painter.text(
            egui::pos2(pos.x, pos.y + 55.0),
            egui::Align2::CENTER_CENTER,
            format_chips(player.current_bet),
            egui::FontId::proportional(9.0),
            egui::Color32::from_rgb(255, 200, 100),
        );
    }

    // hole cards (if showing or is us)
    if is_us || player.show_cards {
        if let Some((c1, c2)) = &player.hole_cards {
            let card_w = 24.0;
            let card_h = 32.0;
            let card_y = pos.y - 55.0;

            render_card(
                painter,
                egui::Rect::from_min_size(
                    egui::pos2(pos.x - card_w - 2.0, card_y),
                    egui::vec2(card_w, card_h),
                ),
                Some(c1),
            );
            render_card(
                painter,
                egui::Rect::from_min_size(
                    egui::pos2(pos.x + 2.0, card_y),
                    egui::vec2(card_w, card_h),
                ),
                Some(c2),
            );
        }
    } else if player.hole_cards.is_some() && !player.is_folded {
        // face down cards
        let card_w = 24.0;
        let card_h = 32.0;
        let card_y = pos.y - 55.0;

        render_card(
            painter,
            egui::Rect::from_min_size(
                egui::pos2(pos.x - card_w - 2.0, card_y),
                egui::vec2(card_w, card_h),
            ),
            None,
        );
        render_card(
            painter,
            egui::Rect::from_min_size(
                egui::pos2(pos.x + 2.0, card_y),
                egui::vec2(card_w, card_h),
            ),
            None,
        );
    }
}

// action bar colors
mod action_theme {
    use bevy_egui::egui::Color32;
    pub const BTN_FOLD: Color32 = Color32::from_rgb(90, 70, 70);
    pub const BTN_CHECK: Color32 = Color32::from_rgb(60, 110, 160);
    pub const BTN_CALL: Color32 = Color32::from_rgb(60, 130, 90);
    pub const BTN_RAISE: Color32 = Color32::from_rgb(80, 120, 180);
    pub const BTN_ALLIN: Color32 = Color32::from_rgb(170, 60, 60);
    pub const TEXT: Color32 = Color32::from_rgb(240, 240, 245);
}

/// render action bar with buttons
fn render_action_bar(ui: &mut egui::Ui, rect: egui::Rect, state: &TableState) -> Option<PlayerAction> {
    let mut action = None;

    // position the action bar
    ui.allocate_ui_at_rect(rect, |ui| {
        ui.horizontal(|ui| {
            // fold button (F)
            let fold_btn = egui::Button::new(
                egui::RichText::new("FOLD")
                    .size(14.0)
                    .color(action_theme::TEXT)
            ).fill(action_theme::BTN_FOLD);
            if ui.add_sized([75.0, 42.0], fold_btn).clicked() {
                action = Some(PlayerAction::Fold);
            }

            ui.add_space(6.0);

            // check/call
            let call_amount = state.current_bet;
            if call_amount == 0 {
                let check_btn = egui::Button::new(
                    egui::RichText::new("CHECK")
                        .size(14.0)
                        .color(action_theme::TEXT)
                ).fill(action_theme::BTN_CHECK);
                if ui.add_sized([85.0, 42.0], check_btn).clicked() {
                    action = Some(PlayerAction::Check);
                }
            } else {
                let label = format!("CALL {}", format_chips(call_amount));
                let call_btn = egui::Button::new(
                    egui::RichText::new(label)
                        .size(14.0)
                        .color(action_theme::TEXT)
                ).fill(action_theme::BTN_CALL);
                if ui.add_sized([100.0, 42.0], call_btn).clicked() {
                    action = Some(PlayerAction::Call(call_amount));
                }
            }

            ui.add_space(6.0);

            // bet/raise presets
            let presets = [
                ("1/2", state.pot / 2),
                ("3/4", state.pot * 3 / 4),
                ("POT", state.pot),
            ];

            for (label, amount) in presets {
                if amount >= state.min_raise {
                    let raise_btn = egui::Button::new(
                        egui::RichText::new(label)
                            .size(13.0)
                            .color(action_theme::TEXT)
                    ).fill(action_theme::BTN_RAISE);
                    if ui.add_sized([55.0, 42.0], raise_btn).clicked() {
                        if state.current_bet == 0 {
                            action = Some(PlayerAction::Bet(amount));
                        } else {
                            action = Some(PlayerAction::Raise(amount));
                        }
                    }
                    ui.add_space(4.0);
                }
            }

            ui.add_space(4.0);

            // all-in button (A)
            let allin_btn = egui::Button::new(
                egui::RichText::new("ALL IN")
                    .size(14.0)
                    .strong()
                    .color(action_theme::TEXT)
            ).fill(action_theme::BTN_ALLIN);
            if ui.add_sized([85.0, 42.0], allin_btn).clicked() {
                action = Some(PlayerAction::AllIn);
            }

            // time remaining indicator
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (time_color, time_bg) = if state.time_remaining < 5.0 {
                    (egui::Color32::WHITE, egui::Color32::from_rgb(180, 50, 50))
                } else if state.time_remaining < 10.0 {
                    (egui::Color32::BLACK, egui::Color32::from_rgb(220, 180, 50))
                } else {
                    (egui::Color32::WHITE, egui::Color32::from_rgb(60, 80, 100))
                };

                egui::Frame::none()
                    .fill(time_bg)
                    .rounding(egui::Rounding::same(6.0))
                    .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(format!("{:.0}s", state.time_remaining))
                            .color(time_color)
                            .size(18.0)
                            .strong());
                    });
            });
        });
    });

    action
}

/// format chip count
fn format_chips(chips: u64) -> String {
    if chips >= 1_000_000 {
        format!("{:.1}M", chips as f64 / 1_000_000.0)
    } else if chips >= 1_000 {
        format!("{:.1}K", chips as f64 / 1_000.0)
    } else {
        chips.to_string()
    }
}

/// chip denomination colors (casino standard)
const CHIP_COLORS: [(u64, egui::Color32); 7] = [
    (1, egui::Color32::WHITE),                    // $1 white
    (5, egui::Color32::from_rgb(220, 50, 50)),    // $5 red
    (25, egui::Color32::from_rgb(50, 180, 50)),   // $25 green
    (100, egui::Color32::from_rgb(30, 30, 30)),   // $100 black
    (500, egui::Color32::from_rgb(150, 50, 200)), // $500 purple
    (1000, egui::Color32::from_rgb(220, 180, 50)), // $1000 yellow
    (5000, egui::Color32::from_rgb(255, 150, 50)), // $5000 orange
];

/// render chip stack at position
fn render_chip_stack(painter: &egui::Painter, pos: egui::Pos2, amount: u64) {
    if amount == 0 {
        return;
    }

    // break amount into chip denominations
    let mut remaining = amount;
    let mut stacks: Vec<(egui::Color32, u32)> = Vec::new();

    for &(denom, color) in CHIP_COLORS.iter().rev() {
        let count = remaining / denom;
        if count > 0 {
            stacks.push((color, count.min(10) as u32)); // max 10 chips per stack visual
            remaining %= denom;
        }
    }

    // render stacks side by side
    let chip_radius = 8.0;
    let chip_height = 2.5;
    let stack_spacing = 18.0;
    let start_x = pos.x - (stacks.len() as f32 - 1.0) * stack_spacing / 2.0;

    for (i, (color, count)) in stacks.iter().enumerate() {
        let stack_x = start_x + i as f32 * stack_spacing;

        for j in 0..*count {
            let chip_y = pos.y - j as f32 * chip_height;

            // chip shadow
            painter.circle_filled(
                egui::pos2(stack_x + 1.0, chip_y + 1.0),
                chip_radius,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 60),
            );

            // chip body
            painter.circle_filled(
                egui::pos2(stack_x, chip_y),
                chip_radius,
                *color,
            );

            // chip edge highlight
            painter.circle_stroke(
                egui::pos2(stack_x, chip_y),
                chip_radius,
                egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 80)),
            );

            // chip center design (dashes)
            let edge_color = if *color == egui::Color32::WHITE {
                egui::Color32::from_rgb(200, 200, 200)
            } else {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 150)
            };
            painter.circle_stroke(
                egui::pos2(stack_x, chip_y),
                chip_radius * 0.6,
                egui::Stroke::new(1.5, edge_color),
            );
        }
    }
}

/// render shuffle animation (deck cards rotating/moving)
pub fn render_shuffle_animation(
    painter: &egui::Painter,
    center: egui::Pos2,
    progress: f32,
    rotations: &[f32],
) {
    let card_count = 8; // visual cards in shuffle
    let card_w = 36.0;
    let card_h = 50.0;

    // shuffle has 3 phases: split, riffle, push together
    let phase = (progress * 3.0).floor() as u32;
    let phase_progress = (progress * 3.0).fract();

    for i in 0..card_count {
        let rotation = rotations.get(i).copied().unwrap_or(0.0);
        let is_left_half = i < card_count / 2;

        let (offset_x, offset_y) = match phase {
            0 => {
                // split deck
                let split = phase_progress * 40.0;
                if is_left_half {
                    (-split - (i as f32 * 0.5), 0.0)
                } else {
                    (split + ((i - card_count / 2) as f32 * 0.5), 0.0)
                }
            }
            1 => {
                // riffle - cards alternate and drop
                let drop = phase_progress * 20.0;
                let alternate = if i % 2 == 0 { -1.0 } else { 1.0 };
                let x_base = if is_left_half { -40.0 } else { 40.0 };
                (
                    x_base * (1.0 - phase_progress) + alternate * 2.0,
                    (i as f32) * drop * 0.3 - 10.0,
                )
            }
            _ => {
                // push together
                let compress = (1.0 - phase_progress) * 30.0;
                (
                    if is_left_half { -compress } else { compress },
                    (i as f32 - card_count as f32 / 2.0) * 0.5,
                )
            }
        };

        // add slight wobble based on rotation
        let wobble = rotation.sin() * 2.0;
        let card_pos = egui::pos2(center.x + offset_x + wobble, center.y + offset_y);
        let card_rect = egui::Rect::from_center_size(card_pos, egui::vec2(card_w, card_h));

        // card back
        painter.rect_filled(
            card_rect,
            egui::Rounding::same(3.0),
            egui::Color32::from_rgb(30, 60, 120),
        );
        painter.rect_stroke(
            card_rect,
            egui::Rounding::same(3.0),
            egui::Stroke::new(1.0, egui::Color32::from_rgb(20, 40, 80)),
        );

        // card back pattern
        let pattern_rect = card_rect.shrink(4.0);
        painter.rect_stroke(
            pattern_rect,
            egui::Rounding::same(2.0),
            egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 90, 150)),
        );
    }
}

/// render deal animation (card flying to player)
pub fn render_deal_animation(
    painter: &egui::Painter,
    deck_pos: egui::Pos2,
    target_pos: egui::Pos2,
    progress: f32,
) {
    // ease out cubic for smooth deceleration
    let t = 1.0 - (1.0 - progress).powi(3);

    let current_pos = egui::pos2(
        deck_pos.x + (target_pos.x - deck_pos.x) * t,
        deck_pos.y + (target_pos.y - deck_pos.y) * t,
    );

    // card shrinks slightly as it travels (perspective)
    let scale = 1.0 - 0.2 * t;
    let card_w = 36.0 * scale;
    let card_h = 50.0 * scale;

    let card_rect = egui::Rect::from_center_size(current_pos, egui::vec2(card_w, card_h));

    // card back
    painter.rect_filled(
        card_rect,
        egui::Rounding::same(3.0),
        egui::Color32::from_rgb(30, 60, 120),
    );
    painter.rect_stroke(
        card_rect,
        egui::Rounding::same(3.0),
        egui::Stroke::new(1.0, egui::Color32::from_rgb(20, 40, 80)),
    );

    // motion blur effect (shadow trail)
    if progress < 0.8 {
        let trail_alpha = ((1.0 - progress) * 100.0) as u8;
        let trail_pos = egui::pos2(
            deck_pos.x + (target_pos.x - deck_pos.x) * (t - 0.1).max(0.0),
            deck_pos.y + (target_pos.y - deck_pos.y) * (t - 0.1).max(0.0),
        );
        painter.rect_filled(
            egui::Rect::from_center_size(trail_pos, egui::vec2(card_w, card_h)),
            egui::Rounding::same(3.0),
            egui::Color32::from_rgba_unmultiplied(30, 60, 120, trail_alpha),
        );
    }
}

/// render card flip animation
pub fn render_card_flip(
    painter: &egui::Painter,
    pos: egui::Pos2,
    card: &Card,
    progress: f32,
) {
    let card_w = 36.0;
    let card_h = 50.0;

    // flip is a horizontal scale from 1 -> 0 -> 1
    let scale_x = (progress * std::f32::consts::PI).cos().abs();
    let showing_face = progress > 0.5;

    let scaled_w = card_w * scale_x.max(0.1);
    let card_rect = egui::Rect::from_center_size(pos, egui::vec2(scaled_w, card_h));

    if showing_face {
        // card face
        painter.rect_filled(
            card_rect,
            egui::Rounding::same(3.0),
            egui::Color32::WHITE,
        );
        painter.rect_stroke(
            card_rect,
            egui::Rounding::same(3.0),
            egui::Stroke::new(1.0, egui::Color32::GRAY),
        );

        // only show rank/suit if wide enough
        if scale_x > 0.3 {
            painter.text(
                card_rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("{}{}", card.rank.symbol(), card.suit.symbol()),
                egui::FontId::proportional(10.0 * scale_x),
                card.suit.color(),
            );
        }
    } else {
        // card back
        painter.rect_filled(
            card_rect,
            egui::Rounding::same(3.0),
            egui::Color32::from_rgb(30, 60, 120),
        );
        painter.rect_stroke(
            card_rect,
            egui::Rounding::same(3.0),
            egui::Stroke::new(1.0, egui::Color32::from_rgb(20, 40, 80)),
        );
    }
}

/// update animation state (call each frame)
pub fn update_animation(state: &mut AnimationState, dt: f32) -> bool {
    if state.kind == AnimationKind::None {
        return false;
    }

    let speed = match state.kind {
        AnimationKind::Shuffle => 2.5,  // fast shuffle
        AnimationKind::Deal => 8.0,     // snappy deal
        AnimationKind::Flip => 6.0,     // quick flip
        AnimationKind::ChipsMove => 4.0,
        AnimationKind::None => 1.0,
    };

    state.progress += dt * speed;

    // update shuffle rotations
    if state.kind == AnimationKind::Shuffle {
        for rot in state.shuffle_rotations.iter_mut() {
            *rot += dt * 15.0;
        }
    }

    if state.progress >= 1.0 {
        state.progress = 0.0;
        state.kind = AnimationKind::None;
        state.dealing_to = None;
        true // animation completed
    } else {
        false
    }
}

/// start shuffle animation
pub fn start_shuffle(state: &mut AnimationState) {
    state.kind = AnimationKind::Shuffle;
    state.progress = 0.0;
    state.shuffle_rotations = (0..8).map(|i| i as f32 * 0.5).collect();
}

/// start deal animation to specific seat
pub fn start_deal(state: &mut AnimationState, seat: usize) {
    state.kind = AnimationKind::Deal;
    state.progress = 0.0;
    state.dealing_to = Some(seat);
}
