//! isometric room - floor, walls, and lighting
//!
//! habbo-style room with simple geometry

use bevy::prelude::*;

pub struct RoomPlugin;

impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_room);
    }
}

/// room dimensions
const ROOM_WIDTH: f32 = 8.0;
const ROOM_DEPTH: f32 = 8.0;
const WALL_HEIGHT: f32 = 4.0;

/// colors (habbo-ish palette)
const FLOOR_COLOR: Color = Color::srgb(0.44, 0.36, 0.28); // wooden floor
const WALL_COLOR: Color = Color::srgb(0.85, 0.82, 0.75); // cream walls
const WALL_TRIM_COLOR: Color = Color::srgb(0.55, 0.45, 0.35); // darker trim

fn setup_room(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // floor
    commands.spawn(PbrBundle {
        mesh: meshes.add(Plane3d::default().mesh().size(ROOM_WIDTH, ROOM_DEPTH)),
        material: materials.add(StandardMaterial {
            base_color: FLOOR_COLOR,
            perceptual_roughness: 0.9,
            ..default()
        }),
        transform: Transform::from_xyz(0.0, 0.0, 0.0),
        ..default()
    });

    // floor pattern (checkerboard tiles)
    let tile_size = 1.0;
    let tile_mesh = meshes.add(Plane3d::default().mesh().size(tile_size * 0.95, tile_size * 0.95));
    let light_tile = materials.add(StandardMaterial {
        base_color: Color::srgb(0.48, 0.40, 0.32),
        perceptual_roughness: 0.85,
        ..default()
    });
    let dark_tile = materials.add(StandardMaterial {
        base_color: Color::srgb(0.40, 0.32, 0.24),
        perceptual_roughness: 0.85,
        ..default()
    });

    for x in 0..8 {
        for z in 0..8 {
            let is_light = (x + z) % 2 == 0;
            let material = if is_light { light_tile.clone() } else { dark_tile.clone() };

            commands.spawn(PbrBundle {
                mesh: tile_mesh.clone(),
                material,
                transform: Transform::from_xyz(
                    (x as f32 - 3.5) * tile_size,
                    0.01,
                    (z as f32 - 3.5) * tile_size,
                ),
                ..default()
            });
        }
    }

    // back wall (along X axis, at -Z)
    commands.spawn(PbrBundle {
        mesh: meshes.add(Cuboid::new(ROOM_WIDTH, WALL_HEIGHT, 0.2)),
        material: materials.add(StandardMaterial {
            base_color: WALL_COLOR,
            perceptual_roughness: 0.7,
            ..default()
        }),
        transform: Transform::from_xyz(0.0, WALL_HEIGHT / 2.0, -ROOM_DEPTH / 2.0),
        ..default()
    });

    // left wall (along Z axis, at -X)
    commands.spawn(PbrBundle {
        mesh: meshes.add(Cuboid::new(0.2, WALL_HEIGHT, ROOM_DEPTH)),
        material: materials.add(StandardMaterial {
            base_color: WALL_COLOR,
            perceptual_roughness: 0.7,
            ..default()
        }),
        transform: Transform::from_xyz(-ROOM_WIDTH / 2.0, WALL_HEIGHT / 2.0, 0.0),
        ..default()
    });

    // wall trim (baseboard) - back wall
    commands.spawn(PbrBundle {
        mesh: meshes.add(Cuboid::new(ROOM_WIDTH, 0.15, 0.25)),
        material: materials.add(StandardMaterial {
            base_color: WALL_TRIM_COLOR,
            perceptual_roughness: 0.6,
            ..default()
        }),
        transform: Transform::from_xyz(0.0, 0.075, -ROOM_DEPTH / 2.0 + 0.1),
        ..default()
    });

    // wall trim - left wall
    commands.spawn(PbrBundle {
        mesh: meshes.add(Cuboid::new(0.25, 0.15, ROOM_DEPTH)),
        material: materials.add(StandardMaterial {
            base_color: WALL_TRIM_COLOR,
            perceptual_roughness: 0.6,
            ..default()
        }),
        transform: Transform::from_xyz(-ROOM_WIDTH / 2.0 + 0.1, 0.075, 0.0),
        ..default()
    });

    // main light (warm overhead)
    commands.spawn(PointLightBundle {
        point_light: PointLight {
            color: Color::srgb(1.0, 0.95, 0.8),
            intensity: 800_000.0,
            range: 20.0,
            shadows_enabled: true,
            ..default()
        },
        transform: Transform::from_xyz(0.0, 6.0, 0.0),
        ..default()
    });

    // fill light (cool ambient from window direction)
    commands.spawn(PointLightBundle {
        point_light: PointLight {
            color: Color::srgb(0.8, 0.85, 1.0),
            intensity: 200_000.0,
            range: 15.0,
            shadows_enabled: false,
            ..default()
        },
        transform: Transform::from_xyz(5.0, 4.0, 5.0),
        ..default()
    });

    // ambient light
    commands.insert_resource(AmbientLight {
        color: Color::srgb(0.9, 0.85, 0.8),
        brightness: 100.0,
    });
}
