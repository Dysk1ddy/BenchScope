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

