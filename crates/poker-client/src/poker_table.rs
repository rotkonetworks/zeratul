//! poker table and chairs
//!
//! classic oval poker table with player positions

use bevy::prelude::*;
use std::f32::consts::PI;

pub struct PokerTablePlugin;

impl Plugin for PokerTablePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_table);
    }
}

/// maximum players at table
pub const MAX_PLAYERS: usize = 10;

/// table dimensions (larger for 10 players)
const TABLE_RADIUS_X: f32 = 3.2;
const TABLE_RADIUS_Z: f32 = 2.0;
pub const TABLE_HEIGHT: f32 = 0.8;
pub const TABLE_THICKNESS: f32 = 0.1;

/// colors
const TABLE_FELT: Color = Color::srgb(0.1, 0.45, 0.25); // green felt
const TABLE_RAIL: Color = Color::srgb(0.35, 0.2, 0.1); // dark wood rail
const CHAIR_WOOD: Color = Color::srgb(0.4, 0.25, 0.15); // chair wood
const CHAIR_CUSHION: Color = Color::srgb(0.6, 0.15, 0.15); // red cushion

/// marker for table entity
#[derive(Component)]
pub struct PokerTable;

/// marker for chair with seat index
#[derive(Component)]
pub struct Chair(pub usize);

/// player seat positions (10 players around oval table)
pub const SEAT_POSITIONS: [(f32, f32); 10] = [
    (0.0, -2.8),    // seat 0: front center (dealer typical)
    (-1.8, -2.2),   // seat 1: front left
    (-2.8, -1.0),   // seat 2: left front
    (-3.0, 0.5),    // seat 3: left back
    (-2.0, 1.8),    // seat 4: back left
    (0.0, 2.5),     // seat 5: back center
    (2.0, 1.8),     // seat 6: back right
    (3.0, 0.5),     // seat 7: right back
    (2.8, -1.0),    // seat 8: right front
    (1.8, -2.2),    // seat 9: front right
];

fn setup_table(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // table felt (oval top)
    // approximate oval with scaled cylinder
    commands.spawn((
        PbrBundle {
            mesh: meshes.add(Cylinder::new(1.0, TABLE_THICKNESS)),
            material: materials.add(StandardMaterial {
                base_color: TABLE_FELT,
                perceptual_roughness: 0.95,
                ..default()
            }),
            transform: Transform::from_xyz(0.0, TABLE_HEIGHT, 0.0)
                .with_scale(Vec3::new(TABLE_RADIUS_X, 1.0, TABLE_RADIUS_Z)),
            ..default()
        },
        PokerTable,
    ));

    // table rail (wooden edge)
    // use a torus-like shape approximated with segments
    let rail_segments = 32;
    let rail_radius = 0.08;

    for i in 0..rail_segments {
        let angle = (i as f32 / rail_segments as f32) * 2.0 * PI;
        let next_angle = ((i + 1) as f32 / rail_segments as f32) * 2.0 * PI;

        let x = angle.cos() * TABLE_RADIUS_X;
        let z = angle.sin() * TABLE_RADIUS_Z;

        commands.spawn(PbrBundle {
            mesh: meshes.add(Sphere::new(rail_radius)),
            material: materials.add(StandardMaterial {
                base_color: TABLE_RAIL,
                perceptual_roughness: 0.4,
                metallic: 0.1,
                ..default()
            }),
            transform: Transform::from_xyz(x, TABLE_HEIGHT + TABLE_THICKNESS / 2.0 + rail_radius * 0.5, z),
            ..default()
        });
    }

    // table legs (4 corners-ish)
    let leg_positions = [
        (-TABLE_RADIUS_X * 0.6, -TABLE_RADIUS_Z * 0.6),
        (TABLE_RADIUS_X * 0.6, -TABLE_RADIUS_Z * 0.6),
        (-TABLE_RADIUS_X * 0.6, TABLE_RADIUS_Z * 0.6),
        (TABLE_RADIUS_X * 0.6, TABLE_RADIUS_Z * 0.6),
    ];

    let leg_mesh = meshes.add(Cylinder::new(0.08, TABLE_HEIGHT));
    let leg_material = materials.add(StandardMaterial {
        base_color: TABLE_RAIL,
        perceptual_roughness: 0.5,
        ..default()
    });

    for (x, z) in leg_positions {
        commands.spawn(PbrBundle {
            mesh: leg_mesh.clone(),
            material: leg_material.clone(),
            transform: Transform::from_xyz(x, TABLE_HEIGHT / 2.0, z),
            ..default()
        });
    }

    // chairs for each seat
    let chair_leg_mesh = meshes.add(Cylinder::new(0.03, 0.45));
    let chair_seat_mesh = meshes.add(Cuboid::new(0.5, 0.06, 0.45));
    let chair_back_mesh = meshes.add(Cuboid::new(0.5, 0.5, 0.06));
    let cushion_mesh = meshes.add(Cuboid::new(0.44, 0.08, 0.39));

    let wood_material = materials.add(StandardMaterial {
        base_color: CHAIR_WOOD,
        perceptual_roughness: 0.6,
        ..default()
    });
    let cushion_material = materials.add(StandardMaterial {
        base_color: CHAIR_CUSHION,
        perceptual_roughness: 0.8,
        ..default()
    });

    for (seat_idx, (x, z)) in SEAT_POSITIONS.iter().enumerate() {
        // calculate rotation to face table center
        let angle = (-*x).atan2(-*z);

        let chair_transform = Transform::from_xyz(*x, 0.0, *z)
            .with_rotation(Quat::from_rotation_y(angle));

        // chair base entity
        let chair_entity = commands.spawn((
            SpatialBundle {
                transform: chair_transform,
                ..default()
            },
            Chair(seat_idx),
        )).id();

        // chair legs
        let leg_offsets = [
            (-0.2, -0.18),
            (0.2, -0.18),
            (-0.2, 0.18),
            (0.2, 0.18),
        ];

        for (lx, lz) in leg_offsets {
            commands.spawn(PbrBundle {
                mesh: chair_leg_mesh.clone(),
                material: wood_material.clone(),
                transform: Transform::from_xyz(*x + lx * angle.cos() - lz * angle.sin(),
                                               0.225,
                                               *z + lx * angle.sin() + lz * angle.cos()),
                ..default()
            });
        }

        // seat
        commands.spawn(PbrBundle {
            mesh: chair_seat_mesh.clone(),
            material: wood_material.clone(),
            transform: chair_transform.with_translation(Vec3::new(*x, 0.45, *z)),
            ..default()
        });

        // cushion
        commands.spawn(PbrBundle {
            mesh: cushion_mesh.clone(),
            material: cushion_material.clone(),
            transform: chair_transform.with_translation(Vec3::new(*x, 0.52, *z)),
            ..default()
        });

        // chair back
        commands.spawn(PbrBundle {
            mesh: chair_back_mesh.clone(),
            material: wood_material.clone(),
            transform: chair_transform
                .with_translation(Vec3::new(
                    *x - 0.22 * angle.sin(),
                    0.7,
                    *z + 0.22 * angle.cos(),
                )),
            ..default()
        });
    }

    // dealer button placeholder (small chip on table)
    commands.spawn(PbrBundle {
        mesh: meshes.add(Cylinder::new(0.12, 0.04)),
        material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.9, 0.9, 0.85),
            perceptual_roughness: 0.3,
            ..default()
        }),
        transform: Transform::from_xyz(1.5, TABLE_HEIGHT + TABLE_THICKNESS / 2.0 + 0.02, 0.0),
        ..default()
    });

    // chip stacks placeholder (center of table)
    spawn_chip_stack(&mut commands, &mut meshes, &mut materials, Vec3::new(0.0, TABLE_HEIGHT + TABLE_THICKNESS / 2.0, 0.0), 5);
}

fn spawn_chip_stack(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    base_pos: Vec3,
    count: usize,
) {
    let chip_mesh = meshes.add(Cylinder::new(0.15, 0.03));
    let chip_colors = [
        Color::srgb(0.8, 0.1, 0.1),  // red
        Color::srgb(0.1, 0.1, 0.8),  // blue
        Color::srgb(0.1, 0.6, 0.1),  // green
        Color::srgb(0.1, 0.1, 0.1),  // black
        Color::srgb(0.9, 0.9, 0.9),  // white
    ];

    for i in 0..count {
        let color = chip_colors[i % chip_colors.len()];
        commands.spawn(PbrBundle {
            mesh: chip_mesh.clone(),
            material: materials.add(StandardMaterial {
                base_color: color,
                perceptual_roughness: 0.4,
                metallic: 0.2,
                ..default()
            }),
            transform: Transform::from_xyz(
                base_pos.x,
                base_pos.y + 0.015 + (i as f32 * 0.031),
                base_pos.z,
            ),
            ..default()
        });
    }
}
