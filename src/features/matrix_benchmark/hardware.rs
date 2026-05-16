const MATMUL_SHADER: &str = r#"
const TILE: u32 = 16u;

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
var<workgroup> tile_b: array<array<f32, 16>, 16>;

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let row = gid.y + params.row_offset;
    let col = gid.x;
    let row_in_chunk = gid.y < params.row_count;
    var sum = 0.0;

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

        if (b_row < params.n && col < params.n) {
            tile_b[lid.y][lid.x] = b[b_row * params.n + col];
        } else {
            tile_b[lid.y][lid.x] = 0.0;
        }

        workgroupBarrier();

        for (var k = 0u; k < TILE; k = k + 1u) {
            sum = sum + tile_a[lid.y][k] * tile_b[k][lid.x];
        }

        workgroupBarrier();
        tile = tile + TILE;
    }

    if (row_in_chunk && row < params.n && col < params.n) {
        c[row * params.n + col] = sum;
    }
}
"#;

const BLOCKED_MATMUL_SHADER: &str = r#"
const TILE: u32 = 16u;

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
var<workgroup> tile_b: array<array<f32, 16>, 16>;

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let row = gid.y;
    let col = gid.x;
    var sum = 0.0;

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

        if (b_row < params.n && col < params.cols) {
            tile_b[lid.y][lid.x] = b[b_row * params.cols + col];
        } else {
            tile_b[lid.y][lid.x] = 0.0;
        }

        workgroupBarrier();

        for (var k = 0u; k < TILE; k = k + 1u) {
            sum = sum + tile_a[lid.y][k] * tile_b[k][lid.x];
        }

        workgroupBarrier();
        tile = tile + TILE;
    }

    if (row < params.rows && col < params.cols) {
        c[row * params.cols + col] = sum;
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

