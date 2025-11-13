//! WebGPU device initialization and management

use wgpu::{Adapter, Device, Queue, Instance, DeviceDescriptor, Features, Limits};

/// GPU device with queue for command submission
pub struct GpuDevice {
    pub device: Device,
    pub queue: Queue,
    pub adapter: Adapter,
    pub capabilities: GpuCapabilities,
}

/// GPU capabilities detected at runtime
#[derive(Debug, Clone)]
pub struct GpuCapabilities {
    /// Maximum buffer size (typically 256 MB - 2 GB)
    pub max_buffer_size: u64,
    /// Maximum storage buffer binding size (typically 128 MB)
    pub max_storage_buffer_binding_size: u32,
    /// Maximum compute workgroup size per dimension (typically 256)
    pub max_compute_workgroup_size_x: u32,
    /// Maximum compute invocations per workgroup (typically 256-1024)
    pub max_compute_invocations_per_workgroup: u32,
    /// Adapter name (e.g., "NVIDIA GeForce RTX 3080")
    pub adapter_name: String,
    /// Backend (Vulkan, Metal, DX12, WebGPU)
    pub backend: String,
}

impl GpuDevice {
    /// Initialize WebGPU device
    pub async fn new() -> Result<Self, String> {
        // Create instance
        let instance = Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Request adapter (GPU)
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| "No WebGPU adapter found".to_string())?;

        // Get adapter info
        let info = adapter.get_info();
        let limits = adapter.limits();

        #[cfg(target_arch = "wasm32")]
        {
            web_sys::console::log_1(&format!("WebGPU adapter found: {}", info.name).into());
            web_sys::console::log_1(&format!("  Backend: {:?}", info.backend).into());
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            println!("WebGPU adapter found:");
            println!("  Name: {}", info.name);
            println!("  Backend: {:?}", info.backend);
            println!("  Vendor: 0x{:X}", info.vendor);
            println!("  Device: 0x{:X}", info.device);
            println!("  Type: {:?}", info.device_type);
        }

        // Request device with necessary features
        let (device, queue) = adapter
            .request_device(
                &DeviceDescriptor {
                    label: Some("Ligerito GPU Device"),
                    required_features: Features::empty(),
                    required_limits: Limits::default(),
                },
                None,
            )
            .await
            .map_err(|e| format!("Failed to create device: {}", e))?;

        let capabilities = GpuCapabilities {
            max_buffer_size: limits.max_buffer_size,
            max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size,
            max_compute_workgroup_size_x: limits.max_compute_workgroup_size_x,
            max_compute_invocations_per_workgroup: limits.max_compute_invocations_per_workgroup,
            adapter_name: info.name.clone(),
            backend: format!("{:?}", info.backend),
        };

        #[cfg(target_arch = "wasm32")]
        {
            web_sys::console::log_1(&format!("WebGPU device initialized: {} MB max buffer",
                capabilities.max_buffer_size / (1024 * 1024)).into());
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            println!("WebGPU device initialized:");
            println!("  Max buffer size: {} MB", capabilities.max_buffer_size / (1024 * 1024));
            println!("  Max workgroup size: {}", capabilities.max_compute_workgroup_size_x);
        }

        Ok(Self {
            device,
            queue,
            adapter,
            capabilities,
        })
    }

    /// Check if device can handle a buffer of given size
    pub fn can_handle_buffer(&self, size: u64) -> bool {
        size <= self.capabilities.max_buffer_size
    }

    /// Get optimal workgroup size for given problem size
    pub fn optimal_workgroup_size(&self, problem_size: u32) -> u32 {
        let max_size = self.capabilities.max_compute_workgroup_size_x;

        // Try powers of 2, starting from 256 (common optimal size)
        for size in [256, 128, 64, 32, 16, 8, 4, 2, 1].iter() {
            if *size <= max_size && problem_size >= *size {
                return *size;
            }
        }

        1
    }
}

/// Check if WebGPU is available (without creating device)
pub async fn is_available() -> bool {
    let instance = Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });

    instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_device_initialization() {
        if !is_available().await {
            eprintln!("WebGPU not available, skipping test");
            return;
        }

        let device = GpuDevice::new().await;
        assert!(device.is_ok());

        if let Ok(dev) = device {
            println!("GPU: {}", dev.capabilities.adapter_name);
            println!("Backend: {}", dev.capabilities.backend);
            assert!(dev.capabilities.max_buffer_size > 0);
        }
    }

    #[test]
    fn test_optimal_workgroup_size() {
        let caps = GpuCapabilities {
            max_buffer_size: 1 << 30,
            max_storage_buffer_binding_size: 1 << 27,
            max_compute_workgroup_size_x: 256,
            max_compute_invocations_per_workgroup: 256,
            adapter_name: "Test GPU".to_string(),
            backend: "WebGPU".to_string(),
        };

        let device = GpuDevice {
            device: unsafe { std::mem::zeroed() }, // Mock device (don't use in real code!)
            queue: unsafe { std::mem::zeroed() },
            adapter: unsafe { std::mem::zeroed() },
            capabilities: caps.clone(),
        };

        assert_eq!(device.optimal_workgroup_size(1024), 256);
        assert_eq!(device.optimal_workgroup_size(128), 128);
        assert_eq!(device.optimal_workgroup_size(10), 8);
    }
}
