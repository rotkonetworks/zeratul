//! pixel-perfect camera setup
//!
//! renders scene to low-res texture then blits to screen with nearest-neighbor scaling
//! gives that chunky habbo hotel pixel aesthetic

use bevy::{
    prelude::*,
    render::{
        camera::RenderTarget,
        render_resource::{
            Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
        },
        view::RenderLayers,
    },
};

/// target render resolution for pixel effect
pub const PIXEL_WIDTH: u32 = 384;
pub const PIXEL_HEIGHT: u32 = 216;

pub struct PixelCameraPlugin;

impl Plugin for PixelCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_pixel_camera);
    }
}

/// marker for the low-res 3d camera
#[derive(Component)]
pub struct PixelCamera;

/// marker for the upscale display camera
#[derive(Component)]
pub struct DisplayCamera;

/// handle to the render texture
#[derive(Resource)]
pub struct PixelTexture(pub Handle<Image>);

fn setup_pixel_camera(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
) {
    // create low-res render target
    let size = Extent3d {
        width: PIXEL_WIDTH,
        height: PIXEL_HEIGHT,
        depth_or_array_layers: 1,
    };

    let mut render_target = Image {
        texture_descriptor: TextureDescriptor {
            label: Some("pixel_render_target"),
            size,
            dimension: TextureDimension::D2,
            format: TextureFormat::Bgra8UnormSrgb,
            mip_level_count: 1,
            sample_count: 1,
            usage: TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_DST
                | TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        },
        ..default()
    };
    render_target.resize(size);

    let render_target_handle = images.add(render_target);

    commands.insert_resource(PixelTexture(render_target_handle.clone()));

    // isometric camera angle (habbo-style 2:1 iso)
    // looking down at ~30 degrees
    let camera_transform = Transform::from_xyz(10.0, 12.0, 10.0)
        .looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y);

    // 3d camera rendering to low-res texture
    commands.spawn((
        Camera3dBundle {
            camera: Camera {
                target: RenderTarget::Image(render_target_handle.clone()),
                order: -1, // render first
                ..default()
            },
            transform: camera_transform,
            ..default()
        },
        PixelCamera,
        RenderLayers::layer(0), // render layer 0 (3d scene)
    ));

    // 2d camera to display the upscaled texture
    // don't clear - we want to see the sprite with render target
    commands.spawn((
        Camera2dBundle {
            camera: Camera {
                order: 0, // render after 3d
                clear_color: bevy::render::camera::ClearColorConfig::None,
                ..default()
            },
            ..default()
        },
        DisplayCamera,
        RenderLayers::layer(1), // render layer 1 (2d overlay)
    ));

    // sprite to display the render texture (fills screen)
    commands.spawn((
        SpriteBundle {
            texture: render_target_handle,
            transform: Transform::from_scale(Vec3::splat(4.0)), // 4x upscale
            ..default()
        },
        RenderLayers::layer(1),
    ));
}
