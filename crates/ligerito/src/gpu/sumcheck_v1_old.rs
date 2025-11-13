//! GPU-accelerated sumcheck polynomial construction
//!
//! This module implements parallel sumcheck polynomial induction on GPU.
//! The key optimization is computing 148+ query contributions simultaneously.

use binary_fields::BinaryFieldElement;
use super::device::GpuDevice;
use super::shaders;
use wgpu::{
    Buffer, BufferUsages, ComputePipeline, BindGroup,
    BindGroupLayout,
};
use bytemuck::{Pod, Zeroable};

/// Sumcheck parameters passed to GPU shader
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct SumcheckParams {
    n: u32,           // log size of basis polynomial
    num_queries: u32, // Number of opened rows
    k: u32,           // Number of v_challenges (row width in log space)
    row_size: u32,    // Actual row size = 2^k
}

unsafe impl Pod for SumcheckParams {}
unsafe impl Zeroable for SumcheckParams {}

/// GPU-accelerated sumcheck construction
pub struct GpuSumcheck {
    device: GpuDevice,
    contribution_pipeline: Option<ComputePipeline>,
    reduce_basis_pipeline: Option<ComputePipeline>,
    reduce_contributions_pipeline: Option<ComputePipeline>,
    bind_group_layout: Option<BindGroupLayout>,
}

impl GpuSumcheck {
    pub fn new(device: GpuDevice) -> Self {
        Self {
            device,
            contribution_pipeline: None,
            reduce_basis_pipeline: None,
            reduce_contributions_pipeline: None,
            bind_group_layout: None,
        }
    }

    /// Initialize GPU pipelines
    async fn init_pipelines(&mut self) -> Result<(), String> {
        if self.contribution_pipeline.is_some() {
            return Ok(());
        }

        // Load sumcheck shader (concatenated with binary field ops)
        let shader_source = self.get_sumcheck_shader_source();
        let shader_module = self.device.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sumcheck Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Create bind group layout
        let bind_group_layout = self.device.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sumcheck Bind Group Layout"),
            entries: &[
                // 0: opened_rows (storage, read)
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 1: v_challenges (storage, read)
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 2: alpha_pows (storage, read)
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 3: debug_dots (storage, read_write) - REUSED from sks_vks binding
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 4: sorted_queries (storage, read)
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 5: local_basis (storage, read_write)
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 6: contributions (storage, read_write)
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 7: params (uniform)
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 8: basis_poly_output (storage, read_write)
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // Create pipeline layout
        let pipeline_layout = self.device.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sumcheck Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create three compute pipelines
        let contribution_pipeline = self.device.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Sumcheck Contribution Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader_module,
            entry_point: "sumcheck_contribution",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        });

        let reduce_basis_pipeline = self.device.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Reduce Basis Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader_module,
            entry_point: "reduce_basis",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        });

        let reduce_contributions_pipeline = self.device.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Reduce Contributions Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader_module,
            entry_point: "reduce_contributions",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        });

        self.bind_group_layout = Some(bind_group_layout);
        self.contribution_pipeline = Some(contribution_pipeline);
        self.reduce_basis_pipeline = Some(reduce_basis_pipeline);
        self.reduce_contributions_pipeline = Some(reduce_contributions_pipeline);

        Ok(())
    }

    /// Get combined shader source (binary_field.wgsl + sumcheck.wgsl)
    fn get_sumcheck_shader_source(&self) -> String {
        format!(
            "{}\n\n{}",
            shaders::BINARY_FIELD_SHADER,
            include_str!("shaders/sumcheck.wgsl")
        )
    }

    /// Compute sumcheck polynomial on GPU with automatic CPU fallback
    ///
    /// This intelligently chooses GPU or CPU based on device capabilities:
    /// - GPU: For sizes that fit within device binding limits (fast on mobile)
    /// - CPU: For large sizes exceeding limits (rare, but supported)
    ///
    /// Target: 8GB Android phones (2025 mid-range)
    pub async fn induce_sumcheck_poly<T, U>(
        &mut self,
        n: usize,
        sks_vks: &[T],
        opened_rows: &[Vec<T>],
        v_challenges: &[U],
        sorted_queries: &[usize],
        alpha: U,
    ) -> Result<(Vec<U>, U), String>
    where
        T: BinaryFieldElement + Pod,
        U: BinaryFieldElement + Pod + From<T>,
    {
        let num_queries = opened_rows.len();
        if num_queries == 0 {
            return Ok((vec![U::zero(); 1 << n], U::zero()));
        }

        let k = v_challenges.len();
        let row_size = 1 << k;
        let basis_size = 1 << n;

        // Calculate required buffer sizes
        let local_basis_size = (num_queries * basis_size * 16) as u64; // 16 bytes per GF(2^128) element
        let max_binding = self.device.capabilities.max_storage_buffer_binding_size as u64;

        // Intelligent fallback: Use GPU if buffers fit, otherwise CPU
        if local_basis_size > max_binding {
            #[cfg(not(target_arch = "wasm32"))]
            println!(
                "GPU buffer limit exceeded ({} MB > {} MB), falling back to CPU (PCLMULQDQ-accelerated)",
                local_basis_size / (1024 * 1024),
                max_binding / (1024 * 1024)
            );

            #[cfg(target_arch = "wasm32")]
            web_sys::console::log_1(&format!(
                "GPU buffer limit exceeded, falling back to CPU"
            ).into());

            // CPU fallback (uses hardware PCLMULQDQ on x86_64 or software fallback)
            use crate::sumcheck_polys::induce_sumcheck_poly as cpu_induce;
            return Ok(cpu_induce(n, sks_vks, opened_rows, v_challenges, sorted_queries, alpha));
        }

        // Row size check for shader capability
        if row_size > 128 {
            #[cfg(not(target_arch = "wasm32"))]
            println!(
                "Row size {} exceeds GPU shader limit (128), falling back to CPU",
                row_size
            );

            #[cfg(target_arch = "wasm32")]
            web_sys::console::log_1(&format!(
                "Row size exceeds shader limit, falling back to CPU"
            ).into());

            use crate::sumcheck_polys::induce_sumcheck_poly as cpu_induce;
            return Ok(cpu_induce(n, sks_vks, opened_rows, v_challenges, sorted_queries, alpha));
        }

        // GPU path - Initialize pipelines if needed
        self.init_pipelines().await?;

        // Verify row sizes
        for (i, row) in opened_rows.iter().enumerate() {
            if row.len() != row_size {
                return Err(format!(
                    "Row {} has size {}, expected {}",
                    i,
                    row.len(),
                    row_size
                ));
            }
        }

        // Precompute alpha powers
        let alpha_pows = self.precompute_alpha_powers(alpha, num_queries);

        // DEBUG: Print alpha powers
        eprintln!("\n=== Alpha Powers ===");
        for (i, pow) in alpha_pows.iter().enumerate().take(8) {
            eprintln!("  alpha^{} = {:?}", i, pow);
        }
        eprintln!("");

        // Precompute the actual basis indices by searching for field element matches
        // This matches the CPU implementation's evaluate_scaled_basis_inplace logic
        let basis_indices: Vec<usize> = sorted_queries
            .iter()
            .map(|&query| {
                let query_mod = query % (1 << n);
                let qf = T::from_bits(query_mod as u64);

                // Search for the index where F::from_bits(idx) == qf
                // This is the same search done in utils.rs:evaluate_scaled_basis_inplace
                (0..(1 << n))
                    .find(|&i| T::from_bits(i as u64) == qf)
                    .unwrap_or(0) // Should always find a match for valid queries
            })
            .collect();

        // Flatten opened_rows for GPU upload
        let flattened_rows: Vec<T> = opened_rows.iter().flat_map(|row| row.iter().copied()).collect();

        // Convert data to u32 arrays for GPU
        let rows_u32 = self.elements_to_u32(&flattened_rows);
        let challenges_u32 = self.elements_to_u32(v_challenges);
        let alpha_pows_u32 = self.elements_to_u32(&alpha_pows);

        // Create params
        let params = SumcheckParams {
            n: n as u32,
            num_queries: num_queries as u32,
            k: k as u32,
            row_size: row_size as u32,
        };

        // Upload buffers (TODO: Use buffer utilities from gpu/buffers.rs)
        let rows_buffer = self.create_storage_buffer(&rows_u32, "Opened Rows");
        let challenges_buffer = self.create_storage_buffer(&challenges_u32, "V Challenges");
        let alpha_pows_buffer = self.create_storage_buffer(&alpha_pows_u32, "Alpha Powers");
        let queries_buffer = self.create_storage_buffer_u32(&basis_indices, "Basis Indices");

        // Allocate output buffers
        let basis_size = 1 << n;
        let local_basis_size = num_queries * basis_size * 4; // 4 u32s per element
        let local_basis_buffer = self.create_storage_buffer_zeroed(local_basis_size, "Local Basis");
        let contributions_buffer = self.create_storage_buffer_zeroed(num_queries * 4, "Contributions");
        let basis_poly_output_buffer = self.create_storage_buffer_zeroed(basis_size * 4, "Basis Poly Output");
        let debug_dots_buffer = self.create_storage_buffer_zeroed(num_queries * 4, "Debug Dots");
        let params_buffer = self.create_uniform_buffer(&[params], "Sumcheck Params");

        // Create bind group
        let bind_group = self.create_bind_group(
            &rows_buffer,
            &challenges_buffer,
            &alpha_pows_buffer,
            &debug_dots_buffer,
            &queries_buffer,
            &local_basis_buffer,
            &contributions_buffer,
            &params_buffer,
            &basis_poly_output_buffer,
        )?;

        // Execute three-stage pipeline
        self.execute_contribution_pass(&bind_group, num_queries as u32)?;
        self.execute_reduce_basis_pass(&bind_group, basis_size as u32)?;
        self.execute_reduce_contributions_pass(&bind_group)?;

        // Download results from separate output buffer
        let basis_poly = self.read_buffer_to_elements(&basis_poly_output_buffer, basis_size).await?;
        let mut enforced_sum_vec = self.read_buffer_to_elements::<U>(&contributions_buffer, 1).await?;
        let enforced_sum = enforced_sum_vec.pop().unwrap_or(U::zero());

        // DEBUG: Read and print alpha powers that GPU actually read
        let gpu_alpha_pows = self.read_buffer_to_elements::<U>(&debug_dots_buffer, num_queries).await?;
        eprintln!("\n=== GPU Alpha Powers (as read by GPU from buffer) ===");
        for (i, alpha_pow) in gpu_alpha_pows.iter().enumerate().take(8) {
            eprintln!("  GPU alpha^{} = {:?}", i, alpha_pow);
        }
        eprintln!("");

        // DEBUG: Read contributions (dot * alpha^i) before reduce_contributions modifies them
        let contributions_vec = self.read_buffer_to_elements::<U>(&contributions_buffer, num_queries).await?;
        eprintln!("=== GPU Contributions (dot * alpha^i) ===");
        for (i, contrib) in contributions_vec.iter().enumerate().take(8) {
            eprintln!("  Query {}: {:?}", i, contrib);
        }
        eprintln!("");

        Ok((basis_poly, enforced_sum))
    }

    /// Precompute powers of alpha
    fn precompute_alpha_powers<U: BinaryFieldElement>(&self, alpha: U, n: usize) -> Vec<U> {
        let mut alpha_pows = vec![U::zero(); n];
        if n > 0 {
            alpha_pows[0] = U::one();
            for i in 1..n {
                alpha_pows[i] = alpha_pows[i - 1].mul(&alpha);
            }
        }
        alpha_pows
    }

    // Buffer helper methods (similar to fft.rs)

    fn create_storage_buffer(&self, data: &[u32], label: &str) -> Buffer {
        use wgpu::util::{DeviceExt, BufferInitDescriptor};

        self.device.device.create_buffer_init(&BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(data),
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        })
    }

    fn create_storage_buffer_u32(&self, data: &[usize], label: &str) -> Buffer {
        use wgpu::util::{DeviceExt, BufferInitDescriptor};

        let data_u32: Vec<u32> = data.iter().map(|&x| x as u32).collect();
        self.device.device.create_buffer_init(&BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(&data_u32),
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        })
    }

    fn create_storage_buffer_zeroed(&self, size_u32: usize, label: &str) -> Buffer {
        self.device.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (size_u32 * 4) as u64,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn create_uniform_buffer<T: Pod>(&self, data: &[T], label: &str) -> Buffer {
        use wgpu::util::{DeviceExt, BufferInitDescriptor};

        self.device.device.create_buffer_init(&BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(data),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        })
    }

    fn create_bind_group(
        &self,
        rows: &Buffer,
        challenges: &Buffer,
        alpha_pows: &Buffer,
        debug_dots: &Buffer,
        queries: &Buffer,
        local_basis: &Buffer,
        contributions: &Buffer,
        params: &Buffer,
        basis_poly_output: &Buffer,
    ) -> Result<BindGroup, String> {
        let layout = self
            .bind_group_layout
            .as_ref()
            .ok_or("Bind group layout not initialized")?;

        Ok(self.device.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sumcheck Bind Group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: rows.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: challenges.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: alpha_pows.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: debug_dots.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: queries.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: local_basis.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: contributions.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: basis_poly_output.as_entire_binding(),
                },
            ],
        }))
    }

    fn execute_contribution_pass(&self, bind_group: &BindGroup, num_queries: u32) -> Result<(), String> {
        let pipeline = self
            .contribution_pipeline
            .as_ref()
            .ok_or("Contribution pipeline not initialized")?;

        let mut encoder = self
            .device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Sumcheck Contribution Encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Sumcheck Contribution Pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(pipeline);
            compute_pass.set_bind_group(0, bind_group, &[]);
            compute_pass.dispatch_workgroups(num_queries, 1, 1);
        }

        self.device.queue.submit(Some(encoder.finish()));

        Ok(())
    }

    fn execute_reduce_basis_pass(&self, bind_group: &BindGroup, basis_size: u32) -> Result<(), String> {
        let pipeline = self
            .reduce_basis_pipeline
            .as_ref()
            .ok_or("Reduce basis pipeline not initialized")?;

        let mut encoder = self
            .device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Reduce Basis Encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Reduce Basis Pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(pipeline);
            compute_pass.set_bind_group(0, bind_group, &[]);

            let workgroup_size = 256;
            let num_workgroups = (basis_size + workgroup_size - 1) / workgroup_size;
            compute_pass.dispatch_workgroups(num_workgroups, 1, 1);
        }

        self.device.queue.submit(Some(encoder.finish()));

        Ok(())
    }

    fn execute_reduce_contributions_pass(&self, bind_group: &BindGroup) -> Result<(), String> {
        let pipeline = self
            .reduce_contributions_pipeline
            .as_ref()
            .ok_or("Reduce contributions pipeline not initialized")?;

        let mut encoder = self
            .device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Reduce Contributions Encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Reduce Contributions Pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(pipeline);
            compute_pass.set_bind_group(0, bind_group, &[]);
            compute_pass.dispatch_workgroups(1, 1, 1);
        }

        self.device.queue.submit(Some(encoder.finish()));

        Ok(())
    }

    /// Convert field elements to u32 array (from fft.rs)
    fn elements_to_u32<F: BinaryFieldElement + Pod>(&self, elements: &[F]) -> Vec<u32> {
        let mut result = Vec::with_capacity(elements.len() * 4);

        for elem in elements {
            let elem_bytes: &[u8] = bytemuck::bytes_of(elem);
            let mut bytes_128 = [0u8; 16];
            let len = elem_bytes.len().min(16);
            bytes_128[..len].copy_from_slice(&elem_bytes[..len]);

            let bits_u128 = u128::from_le_bytes(bytes_128);

            result.push(bits_u128 as u32);
            result.push((bits_u128 >> 32) as u32);
            result.push((bits_u128 >> 64) as u32);
            result.push((bits_u128 >> 96) as u32);
        }

        result
    }

    /// Read buffer from GPU and convert to field elements (from fft.rs)
    async fn read_buffer_to_elements<F: BinaryFieldElement + Pod>(
        &self,
        buffer: &Buffer,
        count: usize,
    ) -> Result<Vec<F>, String> {
        let staging_buffer = self.device.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sumcheck Staging Buffer"),
            size: buffer.size(),
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Sumcheck Copy Encoder"),
            });
        encoder.copy_buffer_to_buffer(buffer, 0, &staging_buffer, 0, buffer.size());
        self.device.queue.submit(Some(encoder.finish()));

        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = futures::channel::oneshot::channel();

        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });

        self.device.device.poll(wgpu::Maintain::Wait);

        receiver
            .await
            .map_err(|_| "Failed to map buffer")?
            .map_err(|e| format!("Buffer mapping error: {:?}", e))?;

        let mut result = Vec::with_capacity(count);

        {
            let data = buffer_slice.get_mapped_range();
            let u32_data: &[u32] = bytemuck::cast_slice(&data);

            for i in 0..count {
                let offset = i * 4;
                let bits_u128 = u32_data[offset] as u128
                    | ((u32_data[offset + 1] as u128) << 32)
                    | ((u32_data[offset + 2] as u128) << 64)
                    | ((u32_data[offset + 3] as u128) << 96);

                let bytes_128 = bits_u128.to_le_bytes();
                let elem_size = core::mem::size_of::<F>();
                if elem_size <= 16 {
                    let mut elem_bytes = vec![0u8; elem_size];
                    elem_bytes.copy_from_slice(&bytes_128[..elem_size]);
                    result.push(*bytemuck::from_bytes::<F>(&elem_bytes));
                }
            }
        }

        staging_buffer.unmap();

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ligerito_binary_fields::{BinaryElem128, BinaryFieldElement};
    use crate::sumcheck_polys::induce_sumcheck_poly as cpu_induce_sumcheck_poly;

    fn generate_test_data(
        n: usize,
        num_queries: usize,
        k: usize,
    ) -> (Vec<BinaryElem128>, Vec<Vec<BinaryElem128>>, Vec<BinaryElem128>, Vec<usize>, BinaryElem128) {
        let row_size = 1 << k;

        // Generate sks_vks (n+1 elements for basis polynomial)
        let sks_vks: Vec<BinaryElem128> = (0..=n)
            .map(|i| BinaryElem128::from_value((i as u128).wrapping_mul(0x123456789ABCDEF)))
            .collect();

        // Generate opened rows
        let opened_rows: Vec<Vec<BinaryElem128>> = (0..num_queries)
            .map(|q| {
                (0..row_size)
                    .map(|i| {
                        BinaryElem128::from_value(
                            ((q * 1000 + i) as u128).wrapping_mul(0xFEDCBA987654321)
                        )
                    })
                    .collect()
            })
            .collect();

        // Generate v_challenges
        let v_challenges: Vec<BinaryElem128> = (0..k)
            .map(|i| BinaryElem128::from_value((i as u128).wrapping_mul(0x111111111111111)))
            .collect();

        // Generate sorted queries
        let sorted_queries: Vec<usize> = (0..num_queries)
            .map(|i| i * 17 % (1 << n))
            .collect();

        // Generate alpha
        let alpha = BinaryElem128::from_value(0xABCDEF0123456789);

        (sks_vks, opened_rows, v_challenges, sorted_queries, alpha)
    }

    #[tokio::test]
    async fn test_gpu_sumcheck_vs_cpu() {
        let n = 8; // Small basis polynomial size (2^8 = 256)
        let k = 4; // Small row size (2^4 = 16)
        let num_queries = 16; // Fewer queries for testing

        let (sks_vks, opened_rows, v_challenges, sorted_queries, alpha) =
            generate_test_data(n, num_queries, k);

        // CPU version
        let (cpu_basis_poly, cpu_enforced_sum) = cpu_induce_sumcheck_poly(
            n,
            &sks_vks,
            &opened_rows,
            &v_challenges,
            &sorted_queries,
            alpha,
        );

        // GPU version
        let device = match GpuDevice::new().await {
            Ok(d) => d,
            Err(e) => {
                eprintln!("GPU not available, skipping test: {}", e);
                return;
            }
        };

        let mut gpu_sumcheck = GpuSumcheck::new(device);

        let result = gpu_sumcheck
            .induce_sumcheck_poly(
                n,
                &sks_vks,
                &opened_rows,
                &v_challenges,
                &sorted_queries,
                alpha,
            )
            .await;

        assert!(result.is_ok(), "GPU sumcheck failed: {:?}", result.err());

        let (gpu_basis_poly, gpu_enforced_sum) = result.unwrap();

        // Compare results
        assert_eq!(
            cpu_basis_poly.len(),
            gpu_basis_poly.len(),
            "Basis polynomial lengths differ"
        );

        assert_eq!(
            cpu_enforced_sum, gpu_enforced_sum,
            "Enforced sums differ: CPU={:?}, GPU={:?}",
            cpu_enforced_sum, gpu_enforced_sum
        );

        for (i, (cpu_val, gpu_val)) in cpu_basis_poly.iter().zip(gpu_basis_poly.iter()).enumerate() {
            assert_eq!(
                cpu_val, gpu_val,
                "Basis polynomial coefficient {} differs: CPU={:?}, GPU={:?}",
                i, cpu_val, gpu_val
            );
        }

        println!("✓ GPU sumcheck matches CPU for n={}, k={}, queries={}", n, k, num_queries);
        println!("  Basis poly length: {}", gpu_basis_poly.len());
        println!("  Enforced sum: {:?}", gpu_enforced_sum);
    }

    #[tokio::test]
    async fn test_gpu_sumcheck_larger() {
        let n = 10; // Larger basis (2^10 = 1024)
        let k = 6;  // Larger rows (2^6 = 64)
        let num_queries = 32;

        let (sks_vks, opened_rows, v_challenges, sorted_queries, alpha) =
            generate_test_data(n, num_queries, k);

        // CPU version
        let (cpu_basis_poly, cpu_enforced_sum) = cpu_induce_sumcheck_poly(
            n,
            &sks_vks,
            &opened_rows,
            &v_challenges,
            &sorted_queries,
            alpha,
        );

        // GPU version
        let device = match GpuDevice::new().await {
            Ok(d) => d,
            Err(e) => {
                eprintln!("GPU not available, skipping test: {}", e);
                return;
            }
        };

        let mut gpu_sumcheck = GpuSumcheck::new(device);

        let result = gpu_sumcheck
            .induce_sumcheck_poly(
                n,
                &sks_vks,
                &opened_rows,
                &v_challenges,
                &sorted_queries,
                alpha,
            )
            .await;

        assert!(result.is_ok(), "GPU sumcheck failed: {:?}", result.err());

        let (gpu_basis_poly, gpu_enforced_sum) = result.unwrap();

        // Compare results
        assert_eq!(cpu_enforced_sum, gpu_enforced_sum);
        assert_eq!(cpu_basis_poly, gpu_basis_poly);

        println!("✓ GPU sumcheck matches CPU for n={}, k={}, queries={}", n, k, num_queries);
    }
}
