//! GPU-accelerated sumcheck polynomial construction - V2
//!
//! This version scales to n=20, n=24, n=28 by eliminating massive buffer allocations.
//!
//! Architecture:
//! - GPU: Computes 148 dot products in parallel → 148 contributions (2.4 KB)
//! - CPU: Accumulates contributions into basis_poly (reuses single temp buffer)
//!
//! Memory usage: O(num_queries) instead of O(num_queries × 2^n)
//! - n=20: 2.4 KB instead of 2.4 GB
//! - n=24: 2.4 KB instead of 38 GB

use binary_fields::BinaryFieldElement;
use super::device::GpuDevice;
use super::shaders;
use wgpu::{Buffer, BufferUsages, ComputePipeline, BindGroup, BindGroupLayout};
use bytemuck::{Pod, Zeroable};

/// Sumcheck parameters
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct SumcheckParams {
    n: u32,
    num_queries: u32,
    k: u32,
    row_size: u32,
}

unsafe impl Pod for SumcheckParams {}
unsafe impl Zeroable for SumcheckParams {}

/// GPU sumcheck - scalable hybrid architecture
pub struct GpuSumcheck {
    device: GpuDevice,
    contribution_pipeline: Option<ComputePipeline>,
    bind_group_layout: Option<BindGroupLayout>,
}

impl GpuSumcheck {
    pub fn new(device: GpuDevice) -> Self {
        Self {
            device,
            contribution_pipeline: None,
            bind_group_layout: None,
        }
    }

    async fn init_pipelines(&mut self) -> Result<(), String> {
        if self.contribution_pipeline.is_some() {
            return Ok(());
        }

        // Load shader (hybrid GPU+CPU architecture)
        let shader_source = format!(
            "{}\n\n{}",
            shaders::BINARY_FIELD_SHADER,
            include_str!("shaders/sumcheck.wgsl")
        );

        let shader_module = self.device.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sumcheck Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Create bind group layout
        let bind_group_layout = self.device.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sumcheck Bind Group Layout"),
            entries: &[
                // 0: opened_rows (read)
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
                // 1: v_challenges (read)
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
                // 2: alpha_pows (read)
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
                // 3: sorted_queries (read)
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 4: contributions (write) - SMALL BUFFER!
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 5: params (uniform)
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
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

        let pipeline_layout = self.device.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sumcheck V2 Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let contribution_pipeline = self.device.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Sumcheck Contribution Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader_module,
            entry_point: "sumcheck_contribution",
            compilation_options: Default::default(),
        });

        self.contribution_pipeline = Some(contribution_pipeline);
        self.bind_group_layout = Some(bind_group_layout);

        Ok(())
    }

    /// Compute sumcheck with GPU contributions + CPU accumulation
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

        // Row size check (shader supports up to 256 elements with 4KB buffer)
        if row_size > 256 {
            #[cfg(not(target_arch = "wasm32"))]
            println!(
                "Row size {} exceeds GPU shader limit (256), falling back to CPU",
                row_size
            );

            use crate::sumcheck_polys::induce_sumcheck_poly as cpu_induce;
            return Ok(cpu_induce(n, sks_vks, opened_rows, v_challenges, sorted_queries, alpha));
        }

        self.init_pipelines().await?;

        // Step 1: GPU computes contributions in parallel
        let contributions = self.compute_contributions_gpu(
            n,
            opened_rows,
            v_challenges,
            sorted_queries,
            alpha,
        ).await?;

        // Step 2: CPU accumulates contributions into basis_poly
        // This matches the CPU implementation's efficient memory pattern
        let (basis_poly, enforced_sum) = self.accumulate_contributions_cpu(
            n,
            sks_vks,
            &contributions,
            sorted_queries,
        );

        Ok((basis_poly, enforced_sum))
    }

    /// GPU: Compute 148 contributions in parallel
    async fn compute_contributions_gpu<T, U>(
        &self,
        n: usize,
        opened_rows: &[Vec<T>],
        v_challenges: &[U],
        sorted_queries: &[usize],
        alpha: U,
    ) -> Result<Vec<U>, String>
    where
        T: BinaryFieldElement + Pod,
        U: BinaryFieldElement + Pod + From<T>,
    {
        let num_queries = opened_rows.len();
        let k = v_challenges.len();
        let row_size = 1 << k;

        // Flatten opened_rows for GPU
        let mut flattened_rows: Vec<U> = Vec::with_capacity(num_queries * row_size);
        for row in opened_rows {
            for &elem in row {
                flattened_rows.push(U::from(elem));
            }
        }

        // Precompute alpha powers
        let alpha_pows = crate::sumcheck_polys::precompute_alpha_powers(alpha, num_queries);

        // Create GPU buffers
        use wgpu::util::{DeviceExt, BufferInitDescriptor};

        let opened_rows_buffer = self.device.device.create_buffer_init(&BufferInitDescriptor {
            label: Some("Opened Rows"),
            contents: bytemuck::cast_slice(&flattened_rows),
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        });

        let v_challenges_buffer = self.device.device.create_buffer_init(&BufferInitDescriptor {
            label: Some("V Challenges"),
            contents: bytemuck::cast_slice(v_challenges),
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        });

        let alpha_pows_buffer = self.device.device.create_buffer_init(&BufferInitDescriptor {
            label: Some("Alpha Powers"),
            contents: bytemuck::cast_slice(&alpha_pows),
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        });

        let sorted_queries_u32: Vec<u32> = sorted_queries.iter().map(|&q| q as u32).collect();
        let sorted_queries_buffer = self.device.device.create_buffer_init(&BufferInitDescriptor {
            label: Some("Sorted Queries"),
            contents: bytemuck::cast_slice(&sorted_queries_u32),
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        });

        // Output buffer: SMALL! Only num_queries contributions
        let contributions_buffer = self.device.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Contributions"),
            size: (num_queries * std::mem::size_of::<U>()) as u64,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let params = SumcheckParams {
            n: n as u32,
            num_queries: num_queries as u32,
            k: k as u32,
            row_size: row_size as u32,
        };

        let params_buffer = self.device.device.create_buffer_init(&BufferInitDescriptor {
            label: Some("Params"),
            contents: bytemuck::bytes_of(&params),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });

        // Create bind group
        let bind_group = self.device.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sumcheck V2 Bind Group"),
            layout: self.bind_group_layout.as_ref().unwrap(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: opened_rows_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: v_challenges_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: alpha_pows_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: sorted_queries_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: contributions_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        // Dispatch compute shader
        let mut encoder = self.device.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Sumcheck V2 Encoder"),
        });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Sumcheck Contribution Pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(self.contribution_pipeline.as_ref().unwrap());
            compute_pass.set_bind_group(0, &bind_group, &[]);
            compute_pass.dispatch_workgroups(num_queries as u32, 1, 1);
        }

        // Read back contributions
        let staging_buffer = self.device.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size: (num_queries * std::mem::size_of::<U>()) as u64,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        encoder.copy_buffer_to_buffer(
            &contributions_buffer,
            0,
            &staging_buffer,
            0,
            (num_queries * std::mem::size_of::<U>()) as u64,
        );

        self.device.queue.submit([encoder.finish()]);

        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = futures::channel::oneshot::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).ok();
        });

        self.device.device.poll(wgpu::Maintain::Wait);
        receiver.await.map_err(|_| "Failed to receive mapping".to_string())?
            .map_err(|e| format!("Buffer mapping failed: {:?}", e))?;

        let data = buffer_slice.get_mapped_range();
        let contributions: Vec<U> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging_buffer.unmap();

        Ok(contributions)
    }

    /// CPU: Accumulate contributions into basis_poly (efficient memory pattern)
    fn accumulate_contributions_cpu<T, U>(
        &self,
        n: usize,
        sks_vks: &[T],
        contributions: &[U],
        sorted_queries: &[usize],
    ) -> (Vec<U>, U)
    where
        T: BinaryFieldElement,
        U: BinaryFieldElement + From<T>,
    {
        use crate::utils::evaluate_scaled_basis_inplace;

        let basis_size = 1 << n;
        let mut basis_poly = vec![U::zero(); basis_size];
        let mut enforced_sum = U::zero();

        // Reuse these buffers across iterations (same as CPU version!)
        let mut local_sks_x = vec![T::zero(); sks_vks.len()];
        let mut local_basis = vec![U::zero(); basis_size];

        for (i, (&contribution, &query)) in contributions.iter().zip(sorted_queries.iter()).enumerate() {
            enforced_sum = enforced_sum.add(&contribution);

            let query_mod = query % basis_size;
            let qf = T::from_bits(query_mod as u64);

            // Compute scaled basis (reuses buffers!)
            evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, sks_vks, qf, contribution);

            // Accumulate into basis_poly
            for (j, &val) in local_basis.iter().enumerate() {
                basis_poly[j] = basis_poly[j].add(&val);
            }
        }

        (basis_poly, enforced_sum)
    }
}
