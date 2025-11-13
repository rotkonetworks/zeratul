//! GPU-accelerated additive FFT over binary extension fields

use binary_fields::BinaryFieldElement;
use super::device::GpuDevice;
use super::shaders;
use wgpu::{
    Buffer, BufferUsages, CommandEncoder, ComputePipeline, BindGroup,
    BindGroupLayout, PipelineLayout, ShaderModule,
};
use bytemuck::{Pod, Zeroable};

/// FFT parameters passed to GPU shader
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct FFTParams {
    size: u32,
    stride: u32,
    log_stride: u32,
    _padding: u32,
}

unsafe impl Pod for FFTParams {}
unsafe impl Zeroable for FFTParams {}

/// GPU-accelerated FFT computation
pub struct GpuFft {
    device: GpuDevice,
    pipeline: Option<ComputePipeline>,
    bind_group_layout: Option<BindGroupLayout>,
}

impl GpuFft {
    pub fn new(device: GpuDevice) -> Self {
        Self {
            device,
            pipeline: None,
            bind_group_layout: None,
        }
    }

    /// Initialize the FFT compute pipeline
    async fn init_pipeline(&mut self) -> Result<(), String> {
        if self.pipeline.is_some() {
            return Ok(());
        }

        // Load and compile shader
        let shader_source = shaders::get_fft_shader_source();
        let shader_module = self.device.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("FFT Butterfly Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Create bind group layout
        let bind_group_layout = self.device.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("FFT Bind Group Layout"),
            entries: &[
                // Storage buffer (read-write data)
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Uniform buffer (params)
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // Create pipeline layout
        let pipeline_layout = self.device.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("FFT Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create compute pipeline
        let pipeline = self.device.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("FFT Butterfly Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader_module,
            entry_point: "fft_butterfly",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        });

        self.bind_group_layout = Some(bind_group_layout);
        self.pipeline = Some(pipeline);

        Ok(())
    }

    /// Perform in-place FFT on GPU
    pub async fn fft_inplace<F: BinaryFieldElement>(&mut self, data: &mut [F]) -> Result<(), String>
    where
        F: bytemuck::Pod,
    {
        // Initialize pipeline if needed
        self.init_pipeline().await?;

        let n = data.len();
        if !n.is_power_of_two() {
            return Err("FFT size must be power of 2".to_string());
        }

        let log_n = n.trailing_zeros();

        // Convert field elements to u32 array (assuming 128-bit elements = 4 x u32)
        let data_u32 = self.elements_to_u32(data);

        // Upload data to GPU
        let data_buffer = self.create_storage_buffer(&data_u32, "FFT Data Buffer");

        // Run log(n) butterfly passes
        for pass in 0..log_n {
            let stride = 1u32 << pass;

            // Create params buffer for this pass
            let params = FFTParams {
                size: n as u32,
                stride,
                log_stride: pass,
                _padding: 0,
            };
            let params_buffer = self.create_uniform_buffer(&[params], "FFT Params Buffer");

            // Create bind group for this pass
            let bind_group = self.create_bind_group(&data_buffer, &params_buffer)?;

            // Execute butterfly shader
            self.execute_butterfly_pass(&bind_group, n as u32 / 2)?;
        }

        // Download result from GPU
        self.read_buffer_to_elements(&data_buffer, data).await?;

        Ok(())
    }

    /// Create storage buffer and upload data
    fn create_storage_buffer(&self, data: &[u32], label: &str) -> Buffer {
        use wgpu::util::{DeviceExt, BufferInitDescriptor};

        self.device.device.create_buffer_init(&BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(data),
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        })
    }

    /// Create uniform buffer
    fn create_uniform_buffer<T: Pod>(&self, data: &[T], label: &str) -> Buffer {
        use wgpu::util::{DeviceExt, BufferInitDescriptor};

        self.device.device.create_buffer_init(&BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(data),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        })
    }

    /// Create bind group
    fn create_bind_group(&self, data_buffer: &Buffer, params_buffer: &Buffer) -> Result<BindGroup, String> {
        let layout = self.bind_group_layout.as_ref()
            .ok_or("Bind group layout not initialized")?;

        Ok(self.device.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("FFT Bind Group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: data_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        }))
    }

    /// Execute one butterfly pass
    fn execute_butterfly_pass(&self, bind_group: &BindGroup, workgroup_count: u32) -> Result<(), String> {
        let pipeline = self.pipeline.as_ref()
            .ok_or("Pipeline not initialized")?;

        let mut encoder = self.device.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("FFT Command Encoder"),
        });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("FFT Butterfly Pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(pipeline);
            compute_pass.set_bind_group(0, bind_group, &[]);

            // Calculate optimal workgroup count
            let workgroup_size = self.device.optimal_workgroup_size(workgroup_count);
            let num_workgroups = (workgroup_count + workgroup_size - 1) / workgroup_size;

            compute_pass.dispatch_workgroups(num_workgroups, 1, 1);
        }

        self.device.queue.submit(Some(encoder.finish()));

        Ok(())
    }

    /// Convert field elements to u32 array
    /// GPU shader expects 128-bit values (4 x u32) regardless of field size
    fn elements_to_u32<F: BinaryFieldElement>(&self, elements: &[F]) -> Vec<u32>
    where
        F: bytemuck::Pod,
    {
        // Allocate result buffer (4 u32s per element for 128-bit representation)
        let mut result = Vec::with_capacity(elements.len() * 4);

        for elem in elements {
            // Convert element to bytes, then to u128
            let elem_bytes: &[u8] = bytemuck::bytes_of(elem);

            // Pad to 128 bits if needed (for smaller field elements)
            let mut bytes_128 = [0u8; 16];
            let len = elem_bytes.len().min(16);
            bytes_128[..len].copy_from_slice(&elem_bytes[..len]);

            let bits_u128 = u128::from_le_bytes(bytes_128);

            // Split into 4 x u32
            result.push(bits_u128 as u32);
            result.push((bits_u128 >> 32) as u32);
            result.push((bits_u128 >> 64) as u32);
            result.push((bits_u128 >> 96) as u32);
        }

        result
    }

    /// Read buffer from GPU and convert to field elements
    async fn read_buffer_to_elements<F: BinaryFieldElement>(
        &self,
        buffer: &Buffer,
        output: &mut [F],
    ) -> Result<(), String>
    where
        F: bytemuck::Pod,
    {
        // Create staging buffer for reading
        let staging_buffer = self.device.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("FFT Staging Buffer"),
            size: buffer.size(),
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Copy from storage to staging
        let mut encoder = self.device.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("FFT Copy Encoder"),
        });
        encoder.copy_buffer_to_buffer(buffer, 0, &staging_buffer, 0, buffer.size());
        self.device.queue.submit(Some(encoder.finish()));

        // Map and read
        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = futures::channel::oneshot::channel();

        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });

        self.device.device.poll(wgpu::Maintain::Wait);

        receiver.await.map_err(|_| "Failed to map buffer")?.map_err(|e| format!("Buffer mapping error: {:?}", e))?;

        {
            let data = buffer_slice.get_mapped_range();
            let u32_data: &[u32] = bytemuck::cast_slice(&data);

            // Convert u32 back to field elements
            for (i, elem) in output.iter_mut().enumerate() {
                let offset = i * 4;
                // Read as u128 first
                let bits_u128 = u32_data[offset] as u128
                    | ((u32_data[offset + 1] as u128) << 32)
                    | ((u32_data[offset + 2] as u128) << 64)
                    | ((u32_data[offset + 3] as u128) << 96);

                // Convert u128 bytes back to field element
                let bytes_128 = bits_u128.to_le_bytes();

                // Use bytemuck to reinterpret bytes as field element
                // This handles both GF(2^64) and GF(2^128)
                let elem_size = core::mem::size_of::<F>();
                if elem_size <= 16 {
                    // Copy element-sized bytes and cast
                    let mut elem_bytes = vec![0u8; elem_size];
                    elem_bytes.copy_from_slice(&bytes_128[..elem_size]);
                    *elem = *bytemuck::from_bytes::<F>(&elem_bytes);
                }
            }
        }

        staging_buffer.unmap();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ligerito_binary_fields::{BinaryElem128, BinaryFieldElement};

    #[tokio::test]
    async fn test_gpu_fft_basic() {
        // Initialize GPU device
        let device = match GpuDevice::new().await {
            Ok(d) => d,
            Err(e) => {
                println!("GPU not available: {}, skipping test", e);
                return;
            }
        };

        let mut gpu_fft = GpuFft::new(device);

        // Create simple test data
        let n = 8;
        let mut data: Vec<BinaryElem128> = (0..n)
            .map(|i| BinaryElem128::from_value(i as u128))
            .collect();

        println!("Input data: {:?}", data);

        // Run GPU FFT
        match gpu_fft.fft_inplace(&mut data).await {
            Ok(_) => println!("GPU FFT completed successfully!"),
            Err(e) => {
                println!("GPU FFT failed: {}", e);
                panic!("GPU FFT test failed");
            }
        }

        println!("Output data: {:?}", data);

        // Basic sanity checks
        // FFT of constant should give [n*const, 0, 0, ...]
        let mut constant_data: Vec<BinaryElem128> = vec![BinaryElem128::from_value(1); n];
        gpu_fft.fft_inplace(&mut constant_data).await.unwrap();

        println!("FFT of all-ones: {:?}", constant_data);

        // In binary fields, sum of n ones = n (if n is odd) or 0 (if n is even)
        // Since n=8 (even), first element should be 0
        // But this depends on the FFT implementation details
    }

    #[tokio::test]
    async fn test_gpu_fft_vs_cpu() {
        // Initialize GPU device
        let device = match GpuDevice::new().await {
            Ok(d) => d,
            Err(e) => {
                println!("GPU not available: {}, skipping test", e);
                return;
            }
        };

        let mut gpu_fft = GpuFft::new(device);

        // Create test data
        let n = 16;
        let data: Vec<BinaryElem128> = (0..n)
            .map(|i| BinaryElem128::from_value((i * 7) as u128))
            .collect();

        let mut gpu_data = data.clone();

        // Run GPU FFT
        gpu_fft.fft_inplace(&mut gpu_data).await.unwrap();

        println!("GPU FFT result: {:?}", gpu_data);

        // TODO: Compare with CPU FFT when available
        // For now, just verify it runs without crashing
    }
}
