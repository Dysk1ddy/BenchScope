const MATMUL_SHADER: &str = r#"
const TILE: u32 = 16u;
const COLS_PER_THREAD: u32 = 4u;
const TILE_COLS: u32 = TILE * COLS_PER_THREAD;

struct Params {
    n: u32,
    row_offset: u32,
    row_count: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> params: Params;

var<workgroup> tile_a: array<array<f32, 16>, 16>;
var<workgroup> tile_b: array<array<f32, 64>, 16>;

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let local_row = wid.y * TILE + lid.y;
    let row = local_row + params.row_offset;
    let base_col = wid.x * TILE_COLS + lid.x;
    let col0 = base_col;
    let col1 = base_col + TILE;
    let col2 = base_col + TILE * 2u;
    let col3 = base_col + TILE * 3u;
    let row_in_chunk = local_row < params.row_count;
    var sum0 = 0.0;
    var sum1 = 0.0;
    var sum2 = 0.0;
    var sum3 = 0.0;

    var tile = 0u;
    loop {
        if (tile >= params.n) {
            break;
        }

        let a_col = tile + lid.x;
        let b_row = tile + lid.y;

        if (row_in_chunk && row < params.n && a_col < params.n) {
            tile_a[lid.y][lid.x] = a[row * params.n + a_col];
        } else {
            tile_a[lid.y][lid.x] = 0.0;
        }

        if (b_row < params.n && col0 < params.n) {
            tile_b[lid.y][lid.x] = b[b_row * params.n + col0];
        } else {
            tile_b[lid.y][lid.x] = 0.0;
        }
        if (b_row < params.n && col1 < params.n) {
            tile_b[lid.y][lid.x + TILE] = b[b_row * params.n + col1];
        } else {
            tile_b[lid.y][lid.x + TILE] = 0.0;
        }
        if (b_row < params.n && col2 < params.n) {
            tile_b[lid.y][lid.x + TILE * 2u] = b[b_row * params.n + col2];
        } else {
            tile_b[lid.y][lid.x + TILE * 2u] = 0.0;
        }
        if (b_row < params.n && col3 < params.n) {
            tile_b[lid.y][lid.x + TILE * 3u] = b[b_row * params.n + col3];
        } else {
            tile_b[lid.y][lid.x + TILE * 3u] = 0.0;
        }

        workgroupBarrier();

        for (var k = 0u; k < TILE; k = k + 1u) {
            let a_value = tile_a[lid.y][k];
            sum0 = sum0 + a_value * tile_b[k][lid.x];
            sum1 = sum1 + a_value * tile_b[k][lid.x + TILE];
            sum2 = sum2 + a_value * tile_b[k][lid.x + TILE * 2u];
            sum3 = sum3 + a_value * tile_b[k][lid.x + TILE * 3u];
        }

        workgroupBarrier();
        tile = tile + TILE;
    }

    if (row_in_chunk && row < params.n && col0 < params.n) {
        c[row * params.n + col0] = sum0;
    }
    if (row_in_chunk && row < params.n && col1 < params.n) {
        c[row * params.n + col1] = sum1;
    }
    if (row_in_chunk && row < params.n && col2 < params.n) {
        c[row * params.n + col2] = sum2;
    }
    if (row_in_chunk && row < params.n && col3 < params.n) {
        c[row * params.n + col3] = sum3;
    }
}
"#;

const BLOCKED_MATMUL_SHADER: &str = r#"
const TILE: u32 = 16u;
const COLS_PER_THREAD: u32 = 4u;
const TILE_COLS: u32 = TILE * COLS_PER_THREAD;

struct BlockParams {
    n: u32,
    rows: u32,
    cols: u32,
    _pad: u32,
}

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> params: BlockParams;

var<workgroup> tile_a: array<array<f32, 16>, 16>;
var<workgroup> tile_b: array<array<f32, 64>, 16>;

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let row = wid.y * TILE + lid.y;
    let base_col = wid.x * TILE_COLS + lid.x;
    let col0 = base_col;
    let col1 = base_col + TILE;
    let col2 = base_col + TILE * 2u;
    let col3 = base_col + TILE * 3u;
    var sum0 = 0.0;
    var sum1 = 0.0;
    var sum2 = 0.0;
    var sum3 = 0.0;

    var tile = 0u;
    loop {
        if (tile >= params.n) {
            break;
        }

        let a_col = tile + lid.x;
        let b_row = tile + lid.y;

        if (row < params.rows && a_col < params.n) {
            tile_a[lid.y][lid.x] = a[row * params.n + a_col];
        } else {
            tile_a[lid.y][lid.x] = 0.0;
        }

        if (b_row < params.n && col0 < params.cols) {
            tile_b[lid.y][lid.x] = b[b_row * params.cols + col0];
        } else {
            tile_b[lid.y][lid.x] = 0.0;
        }
        if (b_row < params.n && col1 < params.cols) {
            tile_b[lid.y][lid.x + TILE] = b[b_row * params.cols + col1];
        } else {
            tile_b[lid.y][lid.x + TILE] = 0.0;
        }
        if (b_row < params.n && col2 < params.cols) {
            tile_b[lid.y][lid.x + TILE * 2u] = b[b_row * params.cols + col2];
        } else {
            tile_b[lid.y][lid.x + TILE * 2u] = 0.0;
        }
        if (b_row < params.n && col3 < params.cols) {
            tile_b[lid.y][lid.x + TILE * 3u] = b[b_row * params.cols + col3];
        } else {
            tile_b[lid.y][lid.x + TILE * 3u] = 0.0;
        }

        workgroupBarrier();

        for (var k = 0u; k < TILE; k = k + 1u) {
            let a_value = tile_a[lid.y][k];
            sum0 = sum0 + a_value * tile_b[k][lid.x];
            sum1 = sum1 + a_value * tile_b[k][lid.x + TILE];
            sum2 = sum2 + a_value * tile_b[k][lid.x + TILE * 2u];
            sum3 = sum3 + a_value * tile_b[k][lid.x + TILE * 3u];
        }

        workgroupBarrier();
        tile = tile + TILE;
    }

    if (row < params.rows && col0 < params.cols) {
        c[row * params.cols + col0] = sum0;
    }
    if (row < params.rows && col1 < params.cols) {
        c[row * params.cols + col1] = sum1;
    }
    if (row < params.rows && col2 < params.cols) {
        c[row * params.cols + col2] = sum2;
    }
    if (row < params.rows && col3 < params.cols) {
        c[row * params.cols + col3] = sum3;
    }
}
"#;

const TINY_STRESS_MATMUL_SHADER: &str = r#"
const LANES: u32 = 256u;

struct Params {
    n: u32,
    row_offset: u32,
    row_count: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let lane = lid.y * 16u + lid.x;
    let output_index = wid.x * LANES + lane;
    let cells = params.n * params.n;
    let cell = output_index % cells;
    let row = cell / params.n;
    let col = cell % params.n;
    var total = 0.0;

    for (var round = 0u; round < params.row_count; round = round + 1u) {
        var sum = 0.0;
        for (var k = 0u; k < params.n; k = k + 1u) {
            sum = sum + a[row * params.n + k] * b[k * params.n + col];
        }
        let salt = f32((round & 7u) + 1u) * 0.000001;
        total = total + sum * (1.0 + salt);
    }

    c[output_index] = total;
}
"#;

const SMALL_TILE_MATMUL_SHADER: &str = r#"
const LANES: u32 = 256u;
const MICRO_TILE: u32 = 4u;

struct Params {
    n: u32,
    row_offset: u32,
    row_count: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(256, 1, 1)
fn main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let tile_index = wid.x * LANES + lid.x;
    let tiles_per_row = params.n / MICRO_TILE;
    let tile_count = tiles_per_row * tiles_per_row;
    if (tile_index >= tile_count) {
        return;
    }

    let row_base = (tile_index / tiles_per_row) * MICRO_TILE;
    let col_base = (tile_index % tiles_per_row) * MICRO_TILE;
    var c00 = 0.0;
    var c01 = 0.0;
    var c02 = 0.0;
    var c03 = 0.0;
    var c10 = 0.0;
    var c11 = 0.0;
    var c12 = 0.0;
    var c13 = 0.0;
    var c20 = 0.0;
    var c21 = 0.0;
    var c22 = 0.0;
    var c23 = 0.0;
    var c30 = 0.0;
    var c31 = 0.0;
    var c32 = 0.0;
    var c33 = 0.0;

    for (var k = 0u; k < params.n; k = k + 1u) {
        let a0 = a[(row_base + 0u) * params.n + k];
        let a1 = a[(row_base + 1u) * params.n + k];
        let a2 = a[(row_base + 2u) * params.n + k];
        let a3 = a[(row_base + 3u) * params.n + k];
        let b0 = b[k * params.n + col_base + 0u];
        let b1 = b[k * params.n + col_base + 1u];
        let b2 = b[k * params.n + col_base + 2u];
        let b3 = b[k * params.n + col_base + 3u];

        c00 = fma(a0, b0, c00);
        c01 = fma(a0, b1, c01);
        c02 = fma(a0, b2, c02);
        c03 = fma(a0, b3, c03);
        c10 = fma(a1, b0, c10);
        c11 = fma(a1, b1, c11);
        c12 = fma(a1, b2, c12);
        c13 = fma(a1, b3, c13);
        c20 = fma(a2, b0, c20);
        c21 = fma(a2, b1, c21);
        c22 = fma(a2, b2, c22);
        c23 = fma(a2, b3, c23);
        c30 = fma(a3, b0, c30);
        c31 = fma(a3, b1, c31);
        c32 = fma(a3, b2, c32);
        c33 = fma(a3, b3, c33);
    }

    c[(row_base + 0u) * params.n + col_base + 0u] = c00;
    c[(row_base + 0u) * params.n + col_base + 1u] = c01;
    c[(row_base + 0u) * params.n + col_base + 2u] = c02;
    c[(row_base + 0u) * params.n + col_base + 3u] = c03;
    c[(row_base + 1u) * params.n + col_base + 0u] = c10;
    c[(row_base + 1u) * params.n + col_base + 1u] = c11;
    c[(row_base + 1u) * params.n + col_base + 2u] = c12;
    c[(row_base + 1u) * params.n + col_base + 3u] = c13;
    c[(row_base + 2u) * params.n + col_base + 0u] = c20;
    c[(row_base + 2u) * params.n + col_base + 1u] = c21;
    c[(row_base + 2u) * params.n + col_base + 2u] = c22;
    c[(row_base + 2u) * params.n + col_base + 3u] = c23;
    c[(row_base + 3u) * params.n + col_base + 0u] = c30;
    c[(row_base + 3u) * params.n + col_base + 1u] = c31;
    c[(row_base + 3u) * params.n + col_base + 2u] = c32;
    c[(row_base + 3u) * params.n + col_base + 3u] = c33;
}
"#;

const REGISTER_TINY_STRESS_MATMUL_SHADER: &str = r#"
const LANES: u32 = 256u;
const MICRO_TILE: u32 = 4u;

struct Params {
    n: u32,
    row_offset: u32,
    row_count: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(256, 1, 1)
fn main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let output_index = wid.x * LANES + lid.x;
    let tiles_per_row = params.n / MICRO_TILE;
    let tile_count = tiles_per_row * tiles_per_row;
    let tile_index = output_index % tile_count;
    let row_base = (tile_index / tiles_per_row) * MICRO_TILE;
    let col_base = (tile_index % tiles_per_row) * MICRO_TILE;
    let lane_salt = f32((output_index & 255u) + 1u) * 0.00000003;
    var total = 0.0;

    for (var round = 0u; round < params.row_count; round = round + 1u) {
        let round_salt = f32(((round + (output_index & 255u)) & 255u) + 1u) * 0.00000025 + lane_salt;
        var c00 = 0.0;
        var c01 = 0.0;
        var c02 = 0.0;
        var c03 = 0.0;
        var c10 = 0.0;
        var c11 = 0.0;
        var c12 = 0.0;
        var c13 = 0.0;
        var c20 = 0.0;
        var c21 = 0.0;
        var c22 = 0.0;
        var c23 = 0.0;
        var c30 = 0.0;
        var c31 = 0.0;
        var c32 = 0.0;
        var c33 = 0.0;

        for (var k = 0u; k < params.n; k = k + 1u) {
            let a0 = a[(row_base + 0u) * params.n + k] + round_salt;
            let a1 = a[(row_base + 1u) * params.n + k] - round_salt;
            let a2 = a[(row_base + 2u) * params.n + k] + lane_salt;
            let a3 = a[(row_base + 3u) * params.n + k] - lane_salt;
            let b0 = b[k * params.n + col_base + 0u];
            let b1 = b[k * params.n + col_base + 1u];
            let b2 = b[k * params.n + col_base + 2u];
            let b3 = b[k * params.n + col_base + 3u];

            c00 = fma(a0, b0, c00);
            c01 = fma(a0, b1, c01);
            c02 = fma(a0, b2, c02);
            c03 = fma(a0, b3, c03);
            c10 = fma(a1, b0, c10);
            c11 = fma(a1, b1, c11);
            c12 = fma(a1, b2, c12);
            c13 = fma(a1, b3, c13);
            c20 = fma(a2, b0, c20);
            c21 = fma(a2, b1, c21);
            c22 = fma(a2, b2, c22);
            c23 = fma(a2, b3, c23);
            c30 = fma(a3, b0, c30);
            c31 = fma(a3, b1, c31);
            c32 = fma(a3, b2, c32);
            c33 = fma(a3, b3, c33);
        }

        total = total
            + c00 + c01 + c02 + c03
            + c10 + c11 + c12 + c13
            + c20 + c21 + c22 + c23
            + c30 + c31 + c32 + c33;
    }

    c[output_index] = total;
}
"#;

const PANEL_STRESS_MATMUL_SHADER: &str = r#"
const LANES: u32 = 256u;

struct BlockParams {
    n: u32,
    rows: u32,
    cols: u32,
    rounds: u32,
}

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> params: BlockParams;

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let lane = lid.y * 16u + lid.x;
    let output_index = wid.x * LANES + lane;
    let cells = params.rows * params.cols;
    let cell = output_index % cells;
    let row = cell / params.cols;
    let col = cell % params.cols;
    var total = 0.0;

    for (var round = 0u; round < params.rounds; round = round + 1u) {
        var sum = 0.0;
        for (var k = 0u; k < params.n; k = k + 1u) {
            sum = sum + a[row * params.n + k] * b[k * params.cols + col];
        }
        let salt = f32((round & 7u) + 1u) * 0.000001;
        total = total + sum * (1.0 + salt);
    }

    c[output_index] = total;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    n: u32,
    row_offset: u32,
    row_count: u32,
    _pad2: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlockParams {
    n: u32,
    rows: u32,
    cols: u32,
    _pad: u32,
}

#[derive(Clone, Debug)]
struct AdapterInfo {
    index: usize,
    name: String,
    backend: wgpu::Backend,
    device_type: wgpu::DeviceType,
    vendor: u32,
    device: u32,
    driver: String,
    timestamp_query: bool,
    dedicated_vram_bytes: Option<u64>,
    dedicated_system_memory_bytes: Option<u64>,
    shared_system_memory_bytes: Option<u64>,
}

impl AdapterInfo {
    fn label(&self) -> String {
        format!(
            "{} - {} - {:?}",
            self.name,
            device_type_label(self.device_type),
            self.backend
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

fn adapter_vendor(adapter: &AdapterInfo) -> GpuVendor {
    let name = adapter.name.to_ascii_lowercase();
    match adapter.vendor {
        0x10DE => GpuVendor::Nvidia,
        0x1002 | 0x1022 => GpuVendor::Amd,
        0x8086 => GpuVendor::Intel,
        _ if name.contains("nvidia") || name.contains("geforce") || name.contains("quadro") => {
            GpuVendor::Nvidia
        }
        _ if name.contains("amd") || name.contains("radeon") || name.contains("firepro") => {
            GpuVendor::Amd
        }
        _ if name.contains("intel")
            || name.contains("arc")
            || name.contains("iris")
            || name.contains("uhd graphics") =>
        {
            GpuVendor::Intel
        }
        _ => GpuVendor::Other,
    }
}

#[derive(Clone, Debug)]
struct DxgiMemoryInfo {
    name: String,
    vendor: u32,
    device: u32,
    dedicated_vram_bytes: u64,
    dedicated_system_memory_bytes: u64,
    shared_system_memory_bytes: u64,
}

#[derive(Clone, Debug)]
struct CpuInfo {
    model: String,
    logical_processors: usize,
}

impl CpuInfo {
    fn label(&self) -> String {
        format!(
            "{} ({} logical processor{})",
            self.model,
            self.logical_processors,
            if self.logical_processors == 1 {
                ""
            } else {
                "s"
            }
        )
    }
}

