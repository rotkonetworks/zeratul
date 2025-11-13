//! GPU buffer management utilities

use wgpu::{Buffer, BufferUsages, Device};

/// Helper for creating and managing GPU buffers
pub struct GpuBufferManager {
    device: Device,
}

impl GpuBufferManager {
    pub fn new(device: Device) -> Self {
        Self { device }
    }

    /// Create a storage buffer for compute shaders
    pub fn create_storage_buffer(&self, size: u64, label: &str) -> Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }

    /// Create a staging buffer for CPU<->GPU transfers
    pub fn create_staging_buffer(&self, size: u64, label: &str) -> Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}
