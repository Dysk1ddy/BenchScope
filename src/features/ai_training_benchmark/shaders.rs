const AI_GEMM_SHADER: &str = r#"
const TILE: u32 = 16u;

struct GemmParams {
    rows: u32,
    cols: u32,
    inner: u32,
    _pad: u32,
}

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> params: GemmParams;

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
        if (tile >= params.inner) {
            break;
        }

        let a_col = tile + lid.x;
        let b_row = tile + lid.y;

        if (row < params.rows && a_col < params.inner) {
            tile_a[lid.y][lid.x] = a[row * params.inner + a_col];
        } else {
            tile_a[lid.y][lid.x] = 0.0;
        }

        if (b_row < params.inner && col < params.cols) {
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

const AI_SGD_UPDATE_SHADER: &str = r#"
struct SgdParams {
    element_count: u32,
    input_dim: u32,
    output_dim: u32,
    start_index: u32,
    learning_rate: f32,
    _pad1: f32,
    _pad2: f32,
    _pad3: f32,
}

@group(0) @binding(0) var<storage, read> gradient: array<f32>;
@group(0) @binding(1) var<storage, read_write> weights: array<f32>;
@group(0) @binding(2) var<storage, read_write> weights_t: array<f32>;
@group(0) @binding(3) var<uniform> params: SgdParams;

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let local_index = gid.x;
    if (local_index >= params.element_count) {
        return;
    }

    let index = params.start_index + local_index;
    let input_index = index / params.output_dim;
    let output_index = index % params.output_dim;
    let updated = weights[index] - params.learning_rate * gradient[index];
    weights[index] = updated;
    weights_t[output_index * params.input_dim + input_index] = updated;
}
"#;
