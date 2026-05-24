#[derive(Clone, Copy, Debug, PartialEq)]
struct MetricRange {
    min: f64,
    max: f64,
}

impl MetricRange {
    const fn new(min: f64, max: f64) -> Self {
        Self { min, max }
    }

    const fn single(value: f64) -> Self {
        Self {
            min: value,
            max: value,
        }
    }

    fn is_single(self) -> bool {
        (self.max - self.min).abs() < 0.05
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TensorCoreThroughputClass {
    Turing,
    AmpereOrNewer,
}

impl TensorCoreThroughputClass {
    fn fp16_fp32_accum_multiplier(self) -> f64 {
        match self {
            TensorCoreThroughputClass::Turing => 4.0,
            TensorCoreThroughputClass::AmpereOrNewer => 2.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct GpuTheoreticalSpec {
    canonical_name: &'static str,
    match_terms: &'static [&'static str],
    throughput_class: TensorCoreThroughputClass,
    cuda_cores_min: u32,
    cuda_cores_max: u32,
    boost_mhz_min: u32,
    boost_mhz_max: u32,
    fp16_tc_fp32_accum_tflops_override: Option<MetricRange>,
}

impl GpuTheoreticalSpec {
    fn matches(self, normalized_adapter_name: &str) -> bool {
        self.match_terms
            .iter()
            .any(|term| normalized_adapter_name.contains(term))
    }

    fn fp16_tc_fp32_accum_tflops(self) -> MetricRange {
        if let Some(override_tflops) = self.fp16_tc_fp32_accum_tflops_override {
            return override_tflops;
        }

        let multiplier = self.throughput_class.fp16_fp32_accum_multiplier();
        MetricRange::new(
            fp16_tc_fp32_accum_tflops(
                self.cuda_cores_min,
                self.boost_mhz_min,
                multiplier,
            ),
            fp16_tc_fp32_accum_tflops(
                self.cuda_cores_max,
                self.boost_mhz_max,
                multiplier,
            ),
        )
    }
}

fn fp16_tc_fp32_accum_tflops(cuda_cores: u32, boost_mhz: u32, multiplier: f64) -> f64 {
    let fp32_tflops = cuda_cores as f64 * boost_mhz as f64 * 2.0 / 1_000_000.0;
    fp32_tflops * multiplier
}

fn theoretical_fp16_tc_fp32_accum_tflops_for_adapter(
    adapter_name: &str,
) -> Option<MetricRange> {
    let normalized_name = normalize_adapter_name(adapter_name);
    GPU_THEORETICAL_SPECS
        .iter()
        .copied()
        .find(|spec| spec.matches(&normalized_name))
        .map(GpuTheoreticalSpec::fp16_tc_fp32_accum_tflops)
}

fn theoretical_gpu_model_name_for_adapter(adapter_name: &str) -> Option<&'static str> {
    let normalized_name = normalize_adapter_name(adapter_name);
    GPU_THEORETICAL_SPECS
        .iter()
        .copied()
        .find(|spec| spec.matches(&normalized_name))
        .map(|spec| spec.canonical_name)
}

// Source data is NVIDIA-published CUDA core counts and boost clocks. The matrix
// stress readout compares float16 torch.mm throughput against peak FP16 Tensor
// Core throughput with FP32 accumulation. Laptop entries keep NVIDIA's published
// boost/core ranges because OEM power limits change the actual peak.
const GPU_THEORETICAL_SPECS: &[GpuTheoreticalSpec] = &[
    // GeForce RTX 50 series laptops.
    spec_range(
        "GeForce RTX 5090 Laptop GPU",
        &["rtx5090laptopgpu", "rtx5090laptop", "rtx5090mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        10_496,
        10_496,
        1_597,
        2_160,
    ),
    spec_range(
        "GeForce RTX 5080 Laptop GPU",
        &["rtx5080laptopgpu", "rtx5080laptop", "rtx5080mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        7_680,
        7_680,
        1_500,
        2_287,
    ),
    spec_range(
        "GeForce RTX 5070 Ti Laptop GPU",
        &[
            "rtx5070tilaptopgpu",
            "rtx5070tilaptop",
            "rtx5070timobile",
        ],
        TensorCoreThroughputClass::AmpereOrNewer,
        5_888,
        5_888,
        1_447,
        2_220,
    ),
    spec_range(
        "GeForce RTX 5070 Laptop GPU",
        &["rtx5070laptopgpu", "rtx5070laptop", "rtx5070mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        4_608,
        4_608,
        1_425,
        2_347,
    ),
    spec_range(
        "GeForce RTX 5060 Laptop GPU",
        &["rtx5060laptopgpu", "rtx5060laptop", "rtx5060mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        3_328,
        3_328,
        1_455,
        2_497,
    ),
    spec_range(
        "GeForce RTX 5050 Laptop GPU",
        &["rtx5050laptopgpu", "rtx5050laptop", "rtx5050mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        2_560,
        2_560,
        1_500,
        2_662,
    ),
    // GeForce RTX 40 series laptops.
    spec_range(
        "GeForce RTX 4090 Laptop GPU",
        &["rtx4090laptopgpu", "rtx4090laptop", "rtx4090mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        9_728,
        9_728,
        1_455,
        2_040,
    ),
    spec_range(
        "GeForce RTX 4080 Laptop GPU",
        &["rtx4080laptopgpu", "rtx4080laptop", "rtx4080mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        7_424,
        7_424,
        1_350,
        2_280,
    ),
    spec_range(
        "GeForce RTX 4070 Laptop GPU",
        &["rtx4070laptopgpu", "rtx4070laptop", "rtx4070mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        4_608,
        4_608,
        1_230,
        2_175,
    ),
    spec_range(
        "GeForce RTX 4060 Laptop GPU",
        &["rtx4060laptopgpu", "rtx4060laptop", "rtx4060mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        3_072,
        3_072,
        1_470,
        2_370,
    ),
    spec_range(
        "GeForce RTX 4050 Laptop GPU",
        &["rtx4050laptopgpu", "rtx4050laptop", "rtx4050mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        2_560,
        2_560,
        1_605,
        2_370,
    ),
    // GeForce RTX 30 series laptops.
    spec_range(
        "GeForce RTX 3080 Ti Laptop GPU",
        &[
            "rtx3080tilaptopgpu",
            "rtx3080tilaptop",
            "rtx3080timobile",
        ],
        TensorCoreThroughputClass::AmpereOrNewer,
        7_424,
        7_424,
        1_125,
        1_590,
    ),
    spec_range(
        "GeForce RTX 3080 Laptop GPU",
        &["rtx3080laptopgpu", "rtx3080laptop", "rtx3080mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        6_144,
        6_144,
        1_245,
        1_710,
    ),
    spec_range(
        "GeForce RTX 3070 Ti Laptop GPU",
        &[
            "rtx3070tilaptopgpu",
            "rtx3070tilaptop",
            "rtx3070timobile",
        ],
        TensorCoreThroughputClass::AmpereOrNewer,
        5_888,
        5_888,
        1_035,
        1_485,
    ),
    spec_range(
        "GeForce RTX 3070 Laptop GPU",
        &["rtx3070laptopgpu", "rtx3070laptop", "rtx3070mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        5_120,
        5_120,
        1_290,
        1_620,
    ),
    spec_range(
        "GeForce RTX 3060 Laptop GPU",
        &["rtx3060laptopgpu", "rtx3060laptop", "rtx3060mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        3_840,
        3_840,
        1_283,
        1_703,
    ),
    spec_range(
        "GeForce RTX 3050 Ti Laptop GPU",
        &[
            "rtx3050tilaptopgpu",
            "rtx3050tilaptop",
            "rtx3050timobile",
        ],
        TensorCoreThroughputClass::AmpereOrNewer,
        2_560,
        2_560,
        1_035,
        1_695,
    ),
    spec_range(
        "GeForce RTX 3050 Laptop GPU",
        &["rtx3050laptopgpu", "rtx3050laptop", "rtx3050mobile"],
        TensorCoreThroughputClass::AmpereOrNewer,
        2_048,
        2_560,
        990,
        1_740,
    ),
    // GeForce RTX 20 series laptops.
    spec_range(
        "GeForce RTX 2080 Super Laptop GPU",
        &[
            "rtx2080superlaptopgpu",
            "rtx2080superlaptop",
            "rtx2080supermobile",
        ],
        TensorCoreThroughputClass::Turing,
        3_072,
        3_072,
        1_080,
        1_560,
    ),
    spec_range(
        "GeForce RTX 2080 Laptop GPU",
        &["rtx2080laptopgpu", "rtx2080laptop", "rtx2080mobile"],
        TensorCoreThroughputClass::Turing,
        2_944,
        2_944,
        1_095,
        1_590,
    ),
    spec_range(
        "GeForce RTX 2070 Super Laptop GPU",
        &[
            "rtx2070superlaptopgpu",
            "rtx2070superlaptop",
            "rtx2070supermobile",
        ],
        TensorCoreThroughputClass::Turing,
        2_560,
        2_560,
        1_155,
        1_380,
    ),
    spec_range(
        "GeForce RTX 2070 Laptop GPU",
        &["rtx2070laptopgpu", "rtx2070laptop", "rtx2070mobile"],
        TensorCoreThroughputClass::Turing,
        2_304,
        2_304,
        1_125,
        1_455,
    ),
    spec_range(
        "GeForce RTX 2060 Laptop GPU",
        &["rtx2060laptopgpu", "rtx2060laptop", "rtx2060mobile"],
        TensorCoreThroughputClass::Turing,
        1_920,
        1_920,
        1_185,
        1_560,
    ),
    spec_range(
        "GeForce RTX 2050 Laptop GPU",
        &["rtx2050laptopgpu", "rtx2050laptop", "rtx2050mobile", "rtx2050"],
        TensorCoreThroughputClass::AmpereOrNewer,
        2_048,
        2_048,
        1_155,
        1_477,
    ),
    // GeForce RTX 50 series desktop cards.
    spec_override(
        "GeForce RTX 5090 D / D v2",
        &["rtx5090dv2", "rtx5090d"],
        TensorCoreThroughputClass::AmpereOrNewer,
        21_760,
        21_760,
        2_410,
        2_410,
        MetricRange::single(148.4),
    ),
    spec_range(
        "GeForce RTX 5090",
        &["rtx5090"],
        TensorCoreThroughputClass::AmpereOrNewer,
        21_760,
        21_760,
        2_410,
        2_410,
    ),
    spec_range(
        "GeForce RTX 5080",
        &["rtx5080"],
        TensorCoreThroughputClass::AmpereOrNewer,
        10_752,
        10_752,
        2_620,
        2_620,
    ),
    spec_range(
        "GeForce RTX 5070 Ti",
        &["rtx5070ti"],
        TensorCoreThroughputClass::AmpereOrNewer,
        8_960,
        8_960,
        2_450,
        2_450,
    ),
    spec_range(
        "GeForce RTX 5070",
        &["rtx5070"],
        TensorCoreThroughputClass::AmpereOrNewer,
        6_144,
        6_144,
        2_510,
        2_510,
    ),
    spec_range(
        "GeForce RTX 5060 Ti",
        &["rtx5060ti"],
        TensorCoreThroughputClass::AmpereOrNewer,
        4_608,
        4_608,
        2_570,
        2_570,
    ),
    spec_range(
        "GeForce RTX 5060",
        &["rtx5060"],
        TensorCoreThroughputClass::AmpereOrNewer,
        3_840,
        3_840,
        2_500,
        2_500,
    ),
    spec_range(
        "GeForce RTX 5050",
        &["rtx5050"],
        TensorCoreThroughputClass::AmpereOrNewer,
        2_560,
        2_560,
        2_570,
        2_570,
    ),
    // GeForce RTX 40 series desktop cards.
    spec_range(
        "GeForce RTX 4090 D",
        &["rtx4090d"],
        TensorCoreThroughputClass::AmpereOrNewer,
        14_592,
        14_592,
        2_520,
        2_520,
    ),
    spec_range(
        "GeForce RTX 4090",
        &["rtx4090"],
        TensorCoreThroughputClass::AmpereOrNewer,
        16_384,
        16_384,
        2_520,
        2_520,
    ),
    spec_range(
        "GeForce RTX 4080 Super",
        &["rtx4080super"],
        TensorCoreThroughputClass::AmpereOrNewer,
        10_240,
        10_240,
        2_550,
        2_550,
    ),
    spec_range(
        "GeForce RTX 4080",
        &["rtx4080"],
        TensorCoreThroughputClass::AmpereOrNewer,
        9_728,
        9_728,
        2_510,
        2_510,
    ),
    spec_range(
        "GeForce RTX 4070 Ti Super",
        &["rtx4070tisuper"],
        TensorCoreThroughputClass::AmpereOrNewer,
        8_448,
        8_448,
        2_610,
        2_610,
    ),
    spec_range(
        "GeForce RTX 4070 Ti",
        &["rtx4070ti"],
        TensorCoreThroughputClass::AmpereOrNewer,
        7_680,
        7_680,
        2_610,
        2_610,
    ),
    spec_range(
        "GeForce RTX 4070 Super",
        &["rtx4070super"],
        TensorCoreThroughputClass::AmpereOrNewer,
        7_168,
        7_168,
        2_480,
        2_480,
    ),
    spec_range(
        "GeForce RTX 4070",
        &["rtx4070"],
        TensorCoreThroughputClass::AmpereOrNewer,
        5_888,
        5_888,
        2_480,
        2_480,
    ),
    spec_range(
        "GeForce RTX 4060 Ti",
        &["rtx4060ti"],
        TensorCoreThroughputClass::AmpereOrNewer,
        4_352,
        4_352,
        2_540,
        2_540,
    ),
    spec_range(
        "GeForce RTX 4060",
        &["rtx4060"],
        TensorCoreThroughputClass::AmpereOrNewer,
        3_072,
        3_072,
        2_460,
        2_460,
    ),
    // GeForce RTX 30 series desktop cards.
    spec_range(
        "GeForce RTX 3090 Ti",
        &["rtx3090ti"],
        TensorCoreThroughputClass::AmpereOrNewer,
        10_752,
        10_752,
        1_860,
        1_860,
    ),
    spec_range(
        "GeForce RTX 3090",
        &["rtx3090"],
        TensorCoreThroughputClass::AmpereOrNewer,
        10_496,
        10_496,
        1_700,
        1_700,
    ),
    spec_range(
        "GeForce RTX 3080 Ti",
        &["rtx3080ti"],
        TensorCoreThroughputClass::AmpereOrNewer,
        10_240,
        10_240,
        1_670,
        1_670,
    ),
    spec_range(
        "GeForce RTX 3080 12GB",
        &["rtx308012gb", "rtx308012g"],
        TensorCoreThroughputClass::AmpereOrNewer,
        8_960,
        8_960,
        1_710,
        1_710,
    ),
    spec_range(
        "GeForce RTX 3080",
        &["rtx3080"],
        TensorCoreThroughputClass::AmpereOrNewer,
        8_704,
        8_704,
        1_710,
        1_710,
    ),
    spec_range(
        "GeForce RTX 3070 Ti",
        &["rtx3070ti"],
        TensorCoreThroughputClass::AmpereOrNewer,
        6_144,
        6_144,
        1_770,
        1_770,
    ),
    spec_range(
        "GeForce RTX 3070",
        &["rtx3070"],
        TensorCoreThroughputClass::AmpereOrNewer,
        5_888,
        5_888,
        1_730,
        1_730,
    ),
    spec_range(
        "GeForce RTX 3060 Ti",
        &["rtx3060ti"],
        TensorCoreThroughputClass::AmpereOrNewer,
        4_864,
        4_864,
        1_670,
        1_670,
    ),
    spec_range(
        "GeForce RTX 3060",
        &["rtx3060"],
        TensorCoreThroughputClass::AmpereOrNewer,
        3_584,
        3_584,
        1_780,
        1_780,
    ),
    spec_range(
        "GeForce RTX 3050 6GB",
        &["rtx30506gb", "rtx30506g"],
        TensorCoreThroughputClass::AmpereOrNewer,
        2_304,
        2_304,
        1_470,
        1_470,
    ),
    spec_range(
        "GeForce RTX 3050 OEM",
        &["rtx3050oem"],
        TensorCoreThroughputClass::AmpereOrNewer,
        2_304,
        2_304,
        1_760,
        1_760,
    ),
    spec_range(
        "GeForce RTX 3050 8GB",
        &["rtx30508gb", "rtx30508g", "rtx3050"],
        TensorCoreThroughputClass::AmpereOrNewer,
        2_560,
        2_560,
        1_780,
        1_780,
    ),
    // GeForce RTX 20 series desktop cards.
    spec_range(
        "GeForce RTX 2080 Ti",
        &["rtx2080ti"],
        TensorCoreThroughputClass::Turing,
        4_352,
        4_352,
        1_640,
        1_640,
    ),
    spec_range(
        "GeForce RTX 2080 Super",
        &["rtx2080super"],
        TensorCoreThroughputClass::Turing,
        3_072,
        3_072,
        1_820,
        1_820,
    ),
    spec_range(
        "GeForce RTX 2080",
        &["rtx2080"],
        TensorCoreThroughputClass::Turing,
        2_944,
        2_944,
        1_800,
        1_800,
    ),
    spec_range(
        "GeForce RTX 2070 Super",
        &["rtx2070super"],
        TensorCoreThroughputClass::Turing,
        2_560,
        2_560,
        1_770,
        1_770,
    ),
    spec_range(
        "GeForce RTX 2070",
        &["rtx2070"],
        TensorCoreThroughputClass::Turing,
        2_304,
        2_304,
        1_710,
        1_710,
    ),
    spec_range(
        "GeForce RTX 2060 Super",
        &["rtx2060super"],
        TensorCoreThroughputClass::Turing,
        2_176,
        2_176,
        1_650,
        1_650,
    ),
    spec_range(
        "GeForce RTX 2060 12GB",
        &["rtx206012gb", "rtx206012g"],
        TensorCoreThroughputClass::Turing,
        2_176,
        2_176,
        1_650,
        1_650,
    ),
    spec_range(
        "GeForce RTX 2060",
        &["rtx2060"],
        TensorCoreThroughputClass::Turing,
        1_920,
        1_920,
        1_680,
        1_680,
    ),
];

const fn spec_range(
    canonical_name: &'static str,
    match_terms: &'static [&'static str],
    throughput_class: TensorCoreThroughputClass,
    cuda_cores_min: u32,
    cuda_cores_max: u32,
    boost_mhz_min: u32,
    boost_mhz_max: u32,
) -> GpuTheoreticalSpec {
    GpuTheoreticalSpec {
        canonical_name,
        match_terms,
        throughput_class,
        cuda_cores_min,
        cuda_cores_max,
        boost_mhz_min,
        boost_mhz_max,
        fp16_tc_fp32_accum_tflops_override: None,
    }
}

const fn spec_override(
    canonical_name: &'static str,
    match_terms: &'static [&'static str],
    throughput_class: TensorCoreThroughputClass,
    cuda_cores_min: u32,
    cuda_cores_max: u32,
    boost_mhz_min: u32,
    boost_mhz_max: u32,
    fp16_tc_fp32_accum_tflops: MetricRange,
) -> GpuTheoreticalSpec {
    GpuTheoreticalSpec {
        canonical_name,
        match_terms,
        throughput_class,
        cuda_cores_min,
        cuda_cores_max,
        boost_mhz_min,
        boost_mhz_max,
        fp16_tc_fp32_accum_tflops_override: Some(fp16_tc_fp32_accum_tflops),
    }
}
