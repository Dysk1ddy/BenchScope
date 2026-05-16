fn matrix_buffers_bytes(size: usize, matrix_count: u64) -> Option<u64> {
    let size = size as u64;
    size.checked_mul(size)?
        .checked_mul(std::mem::size_of::<f32>() as u64)?
        .checked_mul(matrix_count)
}

fn gpu_working_set_bytes(size: usize) -> Option<u64> {
    matrix_buffers_bytes(size, 4)
}

fn estimate_gpu_seconds(size: usize, adapter: &AdapterInfo, gpu_intensity: GpuIntensity) -> f64 {
    let n = size as f64;
    let flops = 2.0 * n * n * n;
    let throughput_flops = match adapter.device_type {
        wgpu::DeviceType::DiscreteGpu => 8.0e12,
        wgpu::DeviceType::IntegratedGpu => 7.0e11,
        wgpu::DeviceType::VirtualGpu => 5.0e11,
        wgpu::DeviceType::Cpu => 1.0e11,
        wgpu::DeviceType::Other => 1.0e12,
    };
    let bandwidth_bytes = match adapter.device_type {
        wgpu::DeviceType::DiscreteGpu => 12.0e9,
        wgpu::DeviceType::IntegratedGpu => 25.0e9,
        wgpu::DeviceType::VirtualGpu => 10.0e9,
        wgpu::DeviceType::Cpu => 8.0e9,
        wgpu::DeviceType::Other => 12.0e9,
    };
    let transfer_s = gpu_working_set_bytes(size)
        .map(|bytes| bytes as f64 / bandwidth_bytes)
        .unwrap_or(0.0);
    let compute_s = flops / throughput_flops;
    let safety_factor = match gpu_intensity {
        GpuIntensity::Safe => 1.8,
        GpuIntensity::Balanced => 1.25,
        GpuIntensity::High => 1.0,
    };
    (compute_s * safety_factor + transfer_s).max(0.2)
}

fn adapter_memory_limit_bytes(adapter: &AdapterInfo) -> Option<(u64, &'static str)> {
    let dedicated = adapter.dedicated_vram_bytes.unwrap_or(0);
    let shared = adapter.shared_system_memory_bytes.unwrap_or(0);
    match adapter.device_type {
        wgpu::DeviceType::DiscreteGpu if dedicated > 0 => Some((dedicated, "dedicated VRAM")),
        wgpu::DeviceType::IntegratedGpu
        | wgpu::DeviceType::Cpu
        | wgpu::DeviceType::VirtualGpu
        | wgpu::DeviceType::Other
            if dedicated + shared > 0 =>
        {
            Some((dedicated + shared, "reported GPU/shared memory"))
        }
        _ if dedicated > 0 => Some((dedicated, "dedicated VRAM")),
        _ => None,
    }
}

