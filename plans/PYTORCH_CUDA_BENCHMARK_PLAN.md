# PyTorch CUDA Benchmark Backend Plan

## Goal

Add an optional PyTorch CUDA backend to the existing AI Training GPU Benchmark.

This backend should measure real PyTorch CUDA training behavior on NVIDIA GPUs:

- PyTorch module forward passes.
- Autograd backward passes.
- Optimizer steps.
- Mixed precision through PyTorch AMP.
- CUDA event timing.
- PyTorch CUDA memory reporting.
- Later: multi-GPU DDP/NCCL communication where supported.

This backend should live beside the portable `wgpu` implementation. The `wgpu` path remains the broad compatibility path. The PyTorch CUDA path is the higher-fidelity NVIDIA training path.

## Why This Backend Exists

The current portable backend uses BenchScope-owned WGSL kernels. That is useful for repeatable training-shaped GPU work, but it does not measure the full PyTorch CUDA stack.

A PyTorch CUDA backend can exercise:

- PyTorch dispatcher and eager execution.
- cuBLAS-backed GEMMs through `torch.matmul` / `nn.Linear`.
- cuDNN-backed kernels where applicable.
- PyTorch autograd scheduling.
- PyTorch optimizers.
- AMP autocast and gradient scaling.
- CUDA allocator behavior.
- DistributedDataParallel gradient synchronization in a future phase.

BenchScope should label these results clearly as `PyTorch CUDA`, not as generic GPU results.

## Design Principles

- Keep PyTorch optional. BenchScope should not require Python or PyTorch to launch.
- Do not install PyTorch automatically during a benchmark run.
- Detect and explain missing dependencies cleanly.
- Run the PyTorch backend out-of-process through a Python script.
- Communicate with the Rust app using line-delimited JSON events.
- Keep the existing `wgpu` result schema where practical so results can be compared.
- Report environment details with every result.
- Prefer honest labels over inflated claims.

## Current BenchScope Integration Points

Existing AI training feature files:

```text
src/features/ai_training_benchmark/
  mod.rs
  model.rs
  runner.rs
  shaders.rs
  ui.rs
```

Planned additions:

```text
src/features/ai_training_benchmark/
  pytorch_cuda.rs
  pytorch_cuda_runner.py
```

The Python script can be embedded with `include_str!()` and written to a temporary file at runtime, or shipped as a repo script during development. Embedding is better for release packaging because the Rust binary can materialize the exact helper it expects.

## Phase 1: PyTorch CUDA Capability Probe

Build the probe first before running benchmarks.

### Rust Model Changes

Add a backend enum:

```rust
enum AiTrainingBackend {
    PortableWgpu,
    PyTorchCuda,
}
```

Add PyTorch environment structs:

```rust
struct PyTorchCudaEnvironment {
    python_executable: String,
    torch_version: String,
    torch_cuda_version: Option<String>,
    cuda_available: bool,
    cudnn_version: Option<String>,
    device_count: usize,
    devices: Vec<PyTorchCudaDevice>,
    distributed_available: bool,
    nccl_available: bool,
    notes: Vec<String>,
}

struct PyTorchCudaDevice {
    index: usize,
    name: String,
    capability_major: i32,
    capability_minor: i32,
    total_memory_bytes: u64,
}
```

### Python Probe

The probe command should run:

```powershell
python benchscope_pytorch_cuda_runner.py --probe-json
```

It should emit one JSON object:

```json
{
  "type": "probe",
  "ok": true,
  "torchVersion": "x.y.z",
  "torchCudaVersion": "12.x",
  "cudaAvailable": true,
  "cudnnVersion": 9000,
  "deviceCount": 1,
  "devices": [
    {
      "index": 0,
      "name": "NVIDIA GeForce RTX ...",
      "computeCapability": [8, 9],
      "totalMemoryBytes": 17179869184
    }
  ],
  "distributedAvailable": true,
  "ncclAvailable": true,
  "notes": []
}
```

### UI Behavior

Add a backend selector:

```text
Backend: Portable wgpu | PyTorch CUDA
```

When PyTorch CUDA is selected:

- Show selected Python executable.
- Show probe status.
- Show detected CUDA devices.
- Show PyTorch, CUDA, cuDNN, and NCCL availability.
- Disable run if CUDA is unavailable.
- Provide a concise message if `torch` is missing.

### CLI Behavior

Add:

```powershell
.\target\release\BenchScope.exe --ai-training-backend pytorch-cuda --probe-pytorch-cuda
```

Add optional Python path:

```powershell
.\target\release\BenchScope.exe --ai-training-backend pytorch-cuda --python C:\path\to\python.exe --probe-pytorch-cuda
```

## Phase 2: Single-GPU PyTorch CUDA Runner

Implement single-GPU training first. This is the smallest useful backend.

### Invocation

Rust should spawn Python with args like:

```powershell
python benchscope_pytorch_cuda_runner.py ^
  --benchmark-json ^
  --device 0 ^
  --workload linear ^
  --precision fp32 ^
  --batch-size 256 ^
  --input-dim 1024 ^
  --output-dim 1024 ^
  --warmup-steps 5 ^
  --measured-steps 20 ^
  --time-limit-s 30
```

### JSON Event Stream

The Python runner should print one JSON object per line:

```json
{"type":"log","message":"Using torch 2.x with CUDA 12.x"}
{"type":"progress","phase":"warmup","completedSteps":2,"totalSteps":25,"elapsedS":1.2}
{"type":"progress","phase":"measured","completedSteps":8,"totalSteps":25,"elapsedS":4.8}
{"type":"result","result":{...}}
```

Rust should parse stdout incrementally and map events to existing `AiTrainingWorkerEvent` variants.

### Cancellation

Cancellation should kill the child Python process.

On Windows, use `Child::kill()` first. Later, consider a job object so child subprocesses from distributed launches are cleaned up together.

## Phase 3: Workloads

Start with workloads that match the existing UI choices.

### Workload 1: Linear Layer Training

PyTorch module:

```python
model = torch.nn.Linear(input_dim, output_dim, bias=False).to(device)
optimizer = torch.optim.SGD(model.parameters(), lr=1e-4)
```

Step:

```python
optimizer.zero_grad(set_to_none=True)
output = model(x)
loss = torch.nn.functional.mse_loss(output, target)
scaler.scale(loss).backward()
scaler.step(optimizer)
scaler.update()
```

For fp32, do the same without scaler/autocast.

Metrics:

- Samples/s.
- Step latency.
- Approximate FLOPs/step.
- Peak allocated CUDA memory.
- Peak reserved CUDA memory.

### Workload 2: MLP Training

PyTorch module:

```python
model = torch.nn.Sequential(
    torch.nn.Linear(hidden_size, expansion_dim, bias=False),
    torch.nn.GELU(),
    torch.nn.Linear(expansion_dim, hidden_size, bias=False),
).to(device)
```

Metrics:

- Samples/s.
- End-to-end training step latency.
- Approximate TFLOP/s.
- Peak memory.

### Workload 3: Transformer Block Training

Initial PyTorch module:

```python
layer = torch.nn.TransformerEncoderLayer(
    d_model=hidden_size,
    nhead=attention_heads,
    dim_feedforward=hidden_size * 4,
    batch_first=True,
    norm_first=True,
).to(device)
```

Synthetic input shape:

```text
[batch_size, sequence_len, hidden_size]
```

Metrics:

- Tokens/s.
- Step latency.
- Approximate TFLOP/s.
- Peak memory.

This path should be labeled as a real PyTorch transformer-layer benchmark, not the same as a full LLM training benchmark.

### Workload 4: Optimizer Stress

Use one or more large `torch.nn.Parameter` tensors and benchmark optimizer update overhead.

Initial variants:

- SGD.
- AdamW.

Metrics:

- Parameters/s.
- Step latency.
- Peak memory.
- Optimizer state bytes estimate.

## Phase 4: Precision Modes

Add precision modes as explicit benchmark settings:

```text
fp32
tf32
amp-fp16
amp-bf16
```

Important behavior:

- `fp32`: no autocast.
- `tf32`: enable the PyTorch CUDA matmul/convolution TF32 settings explicitly and record them.
- `amp-fp16`: use `torch.autocast(device_type="cuda", dtype=torch.float16)` and `torch.amp.GradScaler("cuda")`.
- `amp-bf16`: use `torch.autocast(device_type="cuda", dtype=torch.bfloat16)` and usually no scaler.

The result must record:

- Requested precision.
- Actual dtype path.
- Whether autocast was enabled.
- Whether GradScaler was enabled.
- Matmul precision / TF32 settings.

## Phase 5: Measurement Method

CUDA work is asynchronous, so the runner must not use naive CPU timers alone.

Use CUDA events for GPU elapsed time:

```python
start = torch.cuda.Event(enable_timing=True)
end = torch.cuda.Event(enable_timing=True)

start.record()
train_step()
end.record()
torch.cuda.synchronize(device)
elapsed_ms = start.elapsed_time(end)
```

Also record CPU wall-clock time around the same step:

```python
wall_start = time.perf_counter()
train_step()
torch.cuda.synchronize(device)
wall_ms = (time.perf_counter() - wall_start) * 1000.0
```

Report both:

- `gpu_step_ms`: CUDA event elapsed time.
- `wall_step_ms`: Python-observed end-to-end elapsed time.

The main result table should use CUDA event time for compute latency and wall time for throughput.

### Warmup

Warmup is mandatory because PyTorch may lazily initialize CUDA libraries, load cuBLAS/cuDNN, allocate memory, and compile/cache kernels.

Default:

```text
Quick:    3 warmup, 10 measured
Balanced: 10 warmup, 50 measured
Thorough: 20 warmup, 200 measured
```

The first implementation can map existing BenchScope profiles to these counts or reuse the existing counts for consistency.

### Memory Metrics

Before measured steps:

```python
torch.cuda.reset_peak_memory_stats(device)
torch.cuda.synchronize(device)
```

After measured steps:

```python
allocated = torch.cuda.max_memory_allocated(device)
reserved = torch.cuda.max_memory_reserved(device)
```

Also capture:

```python
free_bytes, total_bytes = torch.cuda.mem_get_info(device)
```

BenchScope should explain that PyTorch memory stats cover PyTorch allocator memory. Some external CUDA allocations, including communication libraries, may not be fully visible through PyTorch allocator stats.

## Phase 6: Result Schema

Extend `AiTrainingResult` without breaking the `wgpu` path.

Suggested additions:

```rust
backend: AiTrainingBackend,
environment: Option<String>,
gpu_step_ms: Option<f64>,
wall_step_ms: Option<f64>,
peak_allocated_bytes: Option<u64>,
peak_reserved_bytes: Option<u64>,
precision_details: Option<String>,
```

For PyTorch CUDA, result notes should include:

- Python path.
- PyTorch version.
- PyTorch CUDA runtime version.
- cuDNN version if available.
- Device name.
- Precision path.
- Whether `torch.compile` was enabled.
- Whether results include DDP communication.

## Phase 7: Multi-GPU Plan

Do this only after single-GPU is reliable.

### Stage 7A: Independent Multi-GPU Replicas

Run one Python worker per selected CUDA device, with no communication.

This answers:

```text
How much total independent PyTorch CUDA training throughput can this system sustain?
```

Report:

- Per-GPU throughput.
- Aggregate throughput.
- Slowest/fastest GPU.
- Scaling efficiency versus the best single-GPU baseline.

Label clearly:

```text
Independent replicas; no gradient synchronization.
```

### Stage 7B: DDP/NCCL Training

Use `torchrun` or `torch.multiprocessing.spawn`, one process per GPU.

Command shape:

```powershell
torchrun --standalone --nproc_per_node 2 benchscope_pytorch_cuda_runner.py --ddp-json ...
```

DDP benchmark should:

- Initialize `torch.distributed`.
- Use one CUDA device per rank.
- Wrap the model in `DistributedDataParallel`.
- Run the same synthetic training step.
- Include gradient all-reduce time in the measured step.

Report:

- Per-rank latency.
- Aggregate throughput.
- Effective samples/s or tokens/s.
- Scaling efficiency.
- Communication overhead estimate.

### Windows Caveat

The current BenchScope workspace is Windows-first. PyTorch documentation notes that Windows supports distributed collective backends except NCCL. Since NCCL is the recommended high-performance GPU backend for PyTorch DDP, the first true NCCL DDP path should target Linux or WSL with NVIDIA CUDA.

On native Windows:

- Support single-GPU PyTorch CUDA.
- Support independent multi-GPU replicas.
- Optionally probe Gloo distributed availability.
- Do not label native Windows Gloo results as NCCL-class multi-GPU training.

## Phase 8: Communication Microbenchmarks

After DDP works, add explicit communication tests:

- `all_reduce` on gradient-sized tensors.
- `broadcast`.
- `all_gather`.
- `reduce_scatter` if available.

Result metrics:

- Message size.
- Latency.
- Effective GB/s.
- Backend name.
- Rank count.

This should be a separate result section because communication benchmarks answer a different question from full training steps.

## Phase 9: UI Changes

In the AI Training GPU Benchmark view:

- Add backend selector.
- Add Python path field or auto-detected Python list.
- Add `Probe PyTorch CUDA` button.
- Show dependency status.
- Show CUDA device list.
- Show precision modes supported by the selected device.
- Keep existing workload controls.
- Add result columns:
  - Backend.
  - CUDA event step.
  - Wall step.
  - Peak allocated.
  - Peak reserved.
  - Environment.

Run button rules:

- `Portable wgpu`: current behavior.
- `PyTorch CUDA`: enabled only after a successful probe with at least one CUDA device.

## Phase 10: CLI Changes

Add:

```powershell
.\target\release\BenchScope.exe --ai-training-backend pytorch-cuda --probe-pytorch-cuda
.\target\release\BenchScope.exe --ai-training-smoke-test --ai-training-backend pytorch-cuda
.\target\release\BenchScope.exe --ai-training-smoke-test --ai-training-backend pytorch-cuda --ai-workload transformer
.\target\release\BenchScope.exe --ai-training-smoke-test --ai-training-backend pytorch-cuda --precision amp-fp16
.\target\release\BenchScope.exe --ai-training-smoke-test --ai-training-backend pytorch-cuda --python C:\path\to\python.exe
```

Later:

```powershell
.\target\release\BenchScope.exe --ai-training-backend pytorch-cuda --devices 0,1 --multi-gpu independent
.\target\release\BenchScope.exe --ai-training-backend pytorch-cuda --devices 0,1 --multi-gpu ddp
```

## Phase 11: Validation

Validation should be lightweight and mostly catches broken runs:

- Check loss is finite.
- Check gradients are finite.
- Check parameters changed after optimizer step.
- For tiny linear shape, compare one PyTorch CPU reference step if practical.
- Detect OOM and report it as a sizing failure, not a crash.

Avoid reading large tensors back to CPU during measured steps.

## Phase 12: Tests

Rust tests:

- Parse probe JSON.
- Parse progress JSON.
- Parse result JSON.
- Convert existing AI config to PyTorch runner args.
- Reject PyTorch CUDA backend when probe reports no CUDA.
- Preserve `wgpu` backend behavior.

Python tests:

- Probe works on CPU-only install and reports `cudaAvailable=false`.
- Argument parser rejects invalid shapes.
- FLOP formulas match Rust for shared workloads.
- JSON result contains required fields.

Integration tests:

- Skip by default if `torch` or CUDA is unavailable.
- If CUDA is available, run tiny linear smoke.
- If multiple CUDA devices and supported OS/backend are available, run independent multi-GPU smoke.

## Implementation Order

1. Add backend enum and UI/backend selection without changing current `wgpu` behavior.
2. Add PyTorch probe script and Rust process wrapper.
3. Add CLI probe command.
4. Add single-GPU linear PyTorch CUDA benchmark.
5. Add result parsing and UI table columns.
6. Add MLP and transformer PyTorch workloads.
7. Add precision modes.
8. Add memory metrics.
9. Add independent multi-GPU replicas.
10. Add Linux/WSL DDP/NCCL path.
11. Add communication microbenchmarks.

## Risks And Mitigations

| Risk | Mitigation |
| --- | --- |
| PyTorch not installed | Probe first; show install guidance; keep `wgpu` available. |
| CPU-only PyTorch install | Report `cudaAvailable=false`; disable PyTorch CUDA run. |
| CUDA wheel/driver mismatch | Surface `torch.version.cuda`, device list, and exception details. |
| Naive timing undercounts CUDA work | Use CUDA events plus explicit synchronization. |
| First-run library loading distorts results | Mandatory warmup. |
| OOM on large presets | Preflight memory estimate and catch CUDA OOM. |
| Windows lacks NCCL backend | Limit native Windows to single-GPU/independent replicas; target Linux/WSL for NCCL DDP. |
| PyTorch version changes | Keep runner small, probe capabilities, and avoid depending on private APIs. |

## Documentation References

- PyTorch CUDA semantics: https://docs.pytorch.org/docs/main/notes/cuda.html
- PyTorch CUDA API: https://docs.pytorch.org/docs/main/cuda.html
- PyTorch benchmark utilities: https://docs.pytorch.org/docs/2.9/benchmark_utils.html
- PyTorch benchmark recipe: https://docs.pytorch.org/tutorials/recipes/recipes/benchmark.html
- PyTorch AMP: https://docs.pytorch.org/docs/stable/amp.html
- PyTorch DistributedDataParallel: https://docs.pytorch.org/docs/stable/generated/torch.nn.parallel.DistributedDataParallel.html
- PyTorch distributed package: https://docs.pytorch.org/docs/2.12/distributed.html
- PyTorch CUDA memory usage: https://docs.pytorch.org/docs/stable/torch_cuda_memory.html
- PyTorch max memory allocated: https://docs.pytorch.org/docs/stable/generated/torch.cuda.max_memory_allocated.html
- NVIDIA NCCL documentation: https://docs.nvidia.com/deeplearning/nccl/index.html
