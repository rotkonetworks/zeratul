//! table graphics - cards, avatars, chips rendered on 3d table
//!
//! syncs with game state to show visual representation

use bevy::prelude::*;
use zk_shuffle::poker::{Card, Rank, Suit};
use std::f32::consts::PI;

use crate::poker_table::{SEAT_POSITIONS, TABLE_HEIGHT, TABLE_THICKNESS};
use crate::ui::GameState;

pub struct TableGraphicsPlugin;

impl Plugin for TableGraphicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_table_graphics)
            .add_systems(Update, (
                sync_community_cards,
                sync_player_cards,
                sync_player_chips,
                sync_pot_chips,
                sync_avatars,
                sync_dealer_button,
            ));
    }
}

/// table surface height for placing objects
const SURFACE_Y: f32 = TABLE_HEIGHT + TABLE_THICKNESS / 2.0 + 0.01;

/// card dimensions
const CARD_WIDTH: f32 = 0.12;
const CARD_HEIGHT: f32 = 0.002;
const CARD_DEPTH: f32 = 0.17;

/// avatar size
const AVATAR_SIZE: f32 = 0.3;

// component markers
#[derive(Component)]
pub struct CommunityCard(pub usize);

#[derive(Component)]
pub struct PlayerCard {
    pub seat: usize,
    pub card_idx: usize,
}

#[derive(Component)]
pub struct PlayerChipStack(pub usize);

#[derive(Component)]
pub struct PlayerBetChips(pub usize);

#[derive(Component)]
pub struct PotChips;

#[derive(Component)]
pub struct PlayerAvatar(pub usize);

#[derive(Component)]
pub struct DealerButton;

#[derive(Component)]
pub struct CardBack;

/// resources for card textures/materials
#[derive(Resource)]
pub struct CardMaterials {
    pub back: Handle<StandardMaterial>,
    pub faces: Vec<Handle<StandardMaterial>>, // 52 cards
}

#[derive(Resource)]
pub struct AvatarMaterials {
    pub bot: Handle<StandardMaterial>,
    pub player: Handle<StandardMaterial>,
}

fn setup_table_graphics(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // card mesh (flat box)
    let card_mesh = meshes.add(Cuboid::new(CARD_WIDTH, CARD_HEIGHT, CARD_DEPTH));

    // card back material (zk.bot blue pattern)
    let card_back = materials.add(StandardMaterial {
        base_color: Color::srgb(0.1, 0.2, 0.5),
        perceptual_roughness: 0.3,
        metallic: 0.1,
        ..default()
    });

    // simple card face colors (we'll use colored materials for now)
    // in production you'd use textures
    let mut card_faces = Vec::new();
    for suit_idx in 0..4 {
        let suit_color = match suit_idx {
            0 | 3 => Color::srgb(0.1, 0.1, 0.1), // spades, clubs = black
            _ => Color::srgb(0.8, 0.1, 0.1),     // hearts, diamonds = red
        };
        for _rank in 0..13 {
            card_faces.push(materials.add(StandardMaterial {
                base_color: Color::WHITE,
                perceptual_roughness: 0.2,
                emissive: suit_color.into(),
                ..default()
            }));
        }
    }

    commands.insert_resource(CardMaterials {
        back: card_back,
        faces: card_faces,
    });

    // avatar materials
    let bot_avatar = materials.add(StandardMaterial {
        base_color: Color::srgb(0.3, 0.5, 0.8), // blue bot
        perceptual_roughness: 0.5,
        metallic: 0.3,
        ..default()
    });

    let player_avatar = materials.add(StandardMaterial {
        base_color: Color::srgb(0.8, 0.6, 0.4), // warm human color
        perceptual_roughness: 0.7,
        ..default()
    });

    commands.insert_resource(AvatarMaterials {
        bot: bot_avatar,
        player: player_avatar,
    });

    // spawn community card slots (5 cards in center)
    for i in 0..5 {
        let x = (i as f32 - 2.0) * (CARD_WIDTH + 0.02);
        commands.spawn((
            PbrBundle {
                mesh: card_mesh.clone(),
                material: materials.add(StandardMaterial {
                    base_color: Color::srgba(0.0, 0.0, 0.0, 0.0),
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                }),
                transform: Transform::from_xyz(x, SURFACE_Y, 0.0),
                visibility: Visibility::Hidden,
                ..default()
            },
            CommunityCard(i),
        ));
    }

    // spawn player card slots (2 cards per seat, only for active players)
    for seat in 0..10 {
        let (sx, sz) = SEAT_POSITIONS[seat];
        // position cards closer to table edge (between player and center)
        let card_x = sx * 0.7;
        let card_z = sz * 0.7;
        let angle = (-sx).atan2(-sz);

        for card_idx in 0..2 {
            let offset = (card_idx as f32 - 0.5) * (CARD_WIDTH + 0.01);
            let local_x = offset * angle.cos();
            let local_z = offset * (-angle.sin());

            commands.spawn((
                PbrBundle {
                    mesh: card_mesh.clone(),
                    material: materials.add(StandardMaterial {
                        base_color: Color::srgba(0.0, 0.0, 0.0, 0.0),
                        alpha_mode: AlphaMode::Blend,
                        ..default()
                    }),
                    transform: Transform::from_xyz(card_x + local_x, SURFACE_Y, card_z + local_z)
                        .with_rotation(Quat::from_rotation_y(angle)),
                    visibility: Visibility::Hidden,
                    ..default()
                },
                PlayerCard { seat, card_idx },
            ));
        }
    }

    // spawn avatars at each seat
    let avatar_mesh = meshes.add(Capsule3d::new(AVATAR_SIZE * 0.4, AVATAR_SIZE));

    for seat in 0..10 {
        let (sx, sz) = SEAT_POSITIONS[seat];
        let angle = (-sx).atan2(-sz);

        commands.spawn((
            PbrBundle {
                mesh: avatar_mesh.clone(),
                material: materials.add(StandardMaterial {
                    base_color: Color::srgba(0.5, 0.5, 0.5, 0.3),
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                }),
                transform: Transform::from_xyz(sx, 0.8, sz)
                    .with_rotation(Quat::from_rotation_y(angle)),
                visibility: Visibility::Hidden,
                ..default()
            },
            PlayerAvatar(seat),
        ));
    }

    // spawn pot chips in center
    commands.spawn((
        SpatialBundle {
            transform: Transform::from_xyz(0.0, SURFACE_Y, 0.5),
            visibility: Visibility::Hidden,
            ..default()
        },
        PotChips,
    ));

    // dealer button
    commands.spawn((
        PbrBundle {
            mesh: meshes.add(Cylinder::new(0.08, 0.02)),
            material: materials.add(StandardMaterial {
                base_color: Color::WHITE,
                perceptual_roughness: 0.2,
                ..default()
            }),
            transform: Transform::from_xyz(0.0, SURFACE_Y, 0.0),
            visibility: Visibility::Hidden,
            ..default()
        },
        DealerButton,
    ));
}

/// sync community cards with game state
fn sync_community_cards(
    game_state: Res<GameState>,
    card_materials: Res<CardMaterials>,
    mut query: Query<(&CommunityCard, &mut Visibility, &mut Handle<StandardMaterial>)>,
) {
    for (comm_card, mut visibility, mut material) in query.iter_mut() {
        if comm_card.0 < game_state.community_cards.len() {
            *visibility = Visibility::Visible;
            let card = game_state.community_cards[comm_card.0];
            *material = get_card_material(&card, &card_materials);
        } else {
            *visibility = Visibility::Hidden;
        }
    }
}

/// sync player hole cards
fn sync_player_cards(
    game_state: Res<GameState>,
    card_materials: Res<CardMaterials>,
    mut query: Query<(&PlayerCard, &mut Visibility, &mut Handle<StandardMaterial>)>,
) {
    for (player_card, mut visibility, mut material) in query.iter_mut() {
        // find player at this seat
        let player = game_state.players.iter()
            .find(|p| p.seat_index == player_card.seat);

        match player {
            Some(p) if !p.is_folded => {
                if let Some(cards) = p.hole_cards {
                    *visibility = Visibility::Visible;
                    // show face for local player, back for others
                    if player_card.seat == game_state.local_player_seat {
                        let card = cards[player_card.card_idx];
                        *material = get_card_material(&card, &card_materials);
                    } else {
                        *material = card_materials.back.clone();
                    }
                } else {
                    *visibility = Visibility::Hidden;
                }
            }
            _ => {
                *visibility = Visibility::Hidden;
            }
        }
    }
}

/// sync player chip stacks (visual representation of chips)
fn sync_player_chips(
    game_state: Res<GameState>,
    mut query: Query<(&PlayerAvatar, &mut Transform, &mut Visibility)>,
) {
    // avatars already show players - we could add chip stack children
    // for now just ensure avatars are visible for active players
    for (avatar, _transform, mut visibility) in query.iter_mut() {
        let player = game_state.players.iter()
            .find(|p| p.seat_index == avatar.0);

        *visibility = if player.is_some() {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// sync pot chips in center
fn sync_pot_chips(
    game_state: Res<GameState>,
    mut query: Query<&mut Visibility, With<PotChips>>,
) {
    for mut visibility in query.iter_mut() {
        *visibility = if game_state.pot > 0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// sync player avatars with game state
fn sync_avatars(
    game_state: Res<GameState>,
    avatar_materials: Res<AvatarMaterials>,
    mut query: Query<(&PlayerAvatar, &mut Visibility, &mut Handle<StandardMaterial>)>,
) {
    for (avatar, mut visibility, mut material) in query.iter_mut() {
        let player = game_state.players.iter()
            .find(|p| p.seat_index == avatar.0);

        match player {
            Some(p) => {
                *visibility = if p.is_folded {
                    Visibility::Hidden
                } else {
                    Visibility::Visible
                };

                // bot or human avatar
                if avatar.0 == game_state.local_player_seat {
                    *material = avatar_materials.player.clone();
                } else {
                    *material = avatar_materials.bot.clone();
                }
            }
            None => {
                *visibility = Visibility::Hidden;
            }
        }
    }
}

/// sync dealer button position
fn sync_dealer_button(
    game_state: Res<GameState>,
    mut query: Query<(&mut Transform, &mut Visibility), With<DealerButton>>,
) {
    for (mut transform, mut visibility) in query.iter_mut() {
        if game_state.players.is_empty() {
            *visibility = Visibility::Hidden;
            continue;
        }

        *visibility = Visibility::Visible;

        let (sx, sz) = SEAT_POSITIONS[game_state.dealer_seat % SEAT_POSITIONS.len()];
        // position button between dealer and center
        let button_x = sx * 0.5;
        let button_z = sz * 0.5;

        transform.translation = Vec3::new(button_x, SURFACE_Y, button_z);
    }
}

/// get material for a card face
fn get_card_material(card: &Card, materials: &CardMaterials) -> Handle<StandardMaterial> {
    let suit_idx = match card.suit {
        Suit::Spades => 0,
        Suit::Hearts => 1,
        Suit::Diamonds => 2,
        Suit::Clubs => 3,
    };

    let rank_idx = match card.rank {
        Rank::Two => 0,
        Rank::Three => 1,
        Rank::Four => 2,
        Rank::Five => 3,
        Rank::Six => 4,
        Rank::Seven => 5,
        Rank::Eight => 6,
        Rank::Nine => 7,
        Rank::Ten => 8,
        Rank::Jack => 9,
        Rank::Queen => 10,
        Rank::King => 11,
        Rank::Ace => 12,
    };

    let idx = suit_idx * 13 + rank_idx;
    materials.faces.get(idx).cloned().unwrap_or(materials.back.clone())
}
