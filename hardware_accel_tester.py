from __future__ import annotations

import argparse
import ctypes
import math
import queue
import sys
import threading
import time
import traceback
import uuid
from dataclasses import dataclass
from typing import Any, Optional

try:
    import numpy as np
except ImportError as exc:  # pragma: no cover - startup guard
    raise SystemExit("NumPy is required. Install it with: python -m pip install numpy") from exc


APP_TITLE = "Hardware Acceleration Tester"
DEFAULT_SIZES = [128, 256, 512, 1024, 2048]
REPEAT_SECONDS = 60.0
TILE_SIZE = 16


UINT = ctypes.c_uint
HRESULT = ctypes.c_long
SIZE_T = ctypes.c_size_t
BOOL = ctypes.c_int
ULONG = ctypes.c_ulong

S_OK = 0
S_FALSE = 1
DXGI_ERROR_NOT_FOUND = ctypes.c_long(0x887A0002).value

D3D_DRIVER_TYPE_UNKNOWN = 0
D3D11_SDK_VERSION = 7
D3D_FEATURE_LEVEL_11_1 = 0xB100
D3D_FEATURE_LEVEL_11_0 = 0xB000

D3D11_USAGE_DEFAULT = 0
D3D11_USAGE_STAGING = 3
D3D11_BIND_CONSTANT_BUFFER = 0x4
D3D11_BIND_SHADER_RESOURCE = 0x8
D3D11_BIND_UNORDERED_ACCESS = 0x80
D3D11_CPU_ACCESS_READ = 0x20000
D3D11_RESOURCE_MISC_BUFFER_STRUCTURED = 0x40
D3D11_SRV_DIMENSION_BUFFER = 1
D3D11_UAV_DIMENSION_BUFFER = 1
D3D11_MAP_READ = 1
D3D11_QUERY_TIMESTAMP = 2
D3D11_QUERY_TIMESTAMP_DISJOINT = 3

DXGI_FORMAT_UNKNOWN = 0
DXGI_ADAPTER_FLAG_SOFTWARE = 2


class DxError(RuntimeError):
    pass


def hresult_hex(hr: int) -> str:
    return f"0x{ctypes.c_uint32(hr).value:08X}"


def check_hr(hr: int, action: str) -> None:
    if hr < 0:
        raise DxError(f"{action} failed with HRESULT {hresult_hex(hr)}")


class GUID(ctypes.Structure):
    _fields_ = [
        ("Data1", ctypes.c_uint32),
        ("Data2", ctypes.c_uint16),
        ("Data3", ctypes.c_uint16),
        ("Data4", ctypes.c_ubyte * 8),
    ]

    @classmethod
    def from_string(cls, value: str) -> "GUID":
        return cls.from_buffer_copy(uuid.UUID(value).bytes_le)


class LUID(ctypes.Structure):
    _fields_ = [
        ("LowPart", ctypes.c_uint32),
        ("HighPart", ctypes.c_int32),
    ]


class DXGI_ADAPTER_DESC1(ctypes.Structure):
    _fields_ = [
        ("Description", ctypes.c_wchar * 128),
        ("VendorId", UINT),
        ("DeviceId", UINT),
        ("SubSysId", UINT),
        ("Revision", UINT),
        ("DedicatedVideoMemory", SIZE_T),
        ("DedicatedSystemMemory", SIZE_T),
        ("SharedSystemMemory", SIZE_T),
        ("AdapterLuid", LUID),
        ("Flags", UINT),
    ]


class D3D11_BUFFER_DESC(ctypes.Structure):
    _fields_ = [
        ("ByteWidth", UINT),
        ("Usage", UINT),
        ("BindFlags", UINT),
        ("CPUAccessFlags", UINT),
        ("MiscFlags", UINT),
        ("StructureByteStride", UINT),
    ]


class D3D11_SUBRESOURCE_DATA(ctypes.Structure):
    _fields_ = [
        ("pSysMem", ctypes.c_void_p),
        ("SysMemPitch", UINT),
        ("SysMemSlicePitch", UINT),
    ]


class D3D11_SHADER_RESOURCE_VIEW_DESC(ctypes.Structure):
    _fields_ = [
        ("Format", UINT),
        ("ViewDimension", UINT),
        ("FirstElement", UINT),
        ("NumElements", UINT),
        ("_unused0", UINT),
        ("_unused1", UINT),
    ]


class D3D11_UNORDERED_ACCESS_VIEW_DESC(ctypes.Structure):
    _fields_ = [
        ("Format", UINT),
        ("ViewDimension", UINT),
        ("FirstElement", UINT),
        ("NumElements", UINT),
        ("Flags", UINT),
        ("_unused0", UINT),
    ]


class D3D11_QUERY_DESC(ctypes.Structure):
    _fields_ = [
        ("Query", UINT),
        ("MiscFlags", UINT),
    ]


class D3D11_QUERY_DATA_TIMESTAMP_DISJOINT(ctypes.Structure):
    _fields_ = [
        ("Frequency", ctypes.c_uint64),
        ("Disjoint", BOOL),
    ]


class D3D11_MAPPED_SUBRESOURCE(ctypes.Structure):
    _fields_ = [
        ("pData", ctypes.c_void_p),
        ("RowPitch", UINT),
        ("DepthPitch", UINT),
    ]


@dataclass(frozen=True)
class AdapterInfo:
    index: int
    name: str
    vendor_id: int
    device_id: int
    dedicated_vram_mb: float
    shared_memory_mb: float
    flags: int
    kind: str
    usable: bool
    error: str = ""

    @property
    def label(self) -> str:
        usable = "" if self.usable else " (unavailable)"
        return (
            f"{self.name} - {self.kind} - "
            f"{self.dedicated_vram_mb:.0f} MB dedicated{usable}"
        )


@dataclass
class GpuTiming:
    compute_ms: Optional[float]
    total_ms: float
    transfer_sync_ms: Optional[float]
    output: np.ndarray


@dataclass
class BenchmarkResult:
    size: int
    adapter_label: str
    cpu_ms: float
    gpu_compute_ms: Optional[float]
    gpu_total_ms: float
    transfer_sync_ms: Optional[float]
    speedup: float
    validation: str


@dataclass
class RepeatProgress:
    mode: str
    size: int
    elapsed_s: float
    iterations: int
    latest_ms: float
    average_ms: float
    average_compute_ms: Optional[float]
    canceled: bool = False


class ComObject:
    def __init__(self, ptr: Optional[ctypes.c_void_p] = None) -> None:
        self.ptr = ptr or ctypes.c_void_p()

    @property
    def value(self) -> int:
        return int(self.ptr.value or 0)

    def release(self) -> None:
        if self.ptr and self.ptr.value:
            fn = com_method(self.ptr, 2, ULONG)
            fn(self.ptr)
            self.ptr = ctypes.c_void_p()

    def __enter__(self) -> "ComObject":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.release()


def com_method(
    ptr: ctypes.c_void_p,
    index: int,
    restype: Any,
    *argtypes: Any,
) -> Any:
    if not ptr or not ptr.value:
        raise DxError("Attempted to call a COM method on a null pointer")
    vtbl = ctypes.cast(ptr, ctypes.POINTER(ctypes.POINTER(ctypes.c_void_p))).contents
    addr = vtbl[index]
    return ctypes.WINFUNCTYPE(restype, ctypes.c_void_p, *argtypes)(addr)


def create_dxgi_factory() -> ComObject:
    dxgi = ctypes.WinDLL("dxgi.dll")
    factory = ComObject()
    iid_factory1 = GUID.from_string("770aae78-f26f-4dba-a829-253c83d1b387")
    create_factory = dxgi.CreateDXGIFactory1
    create_factory.argtypes = [ctypes.POINTER(GUID), ctypes.POINTER(ctypes.c_void_p)]
    create_factory.restype = HRESULT
    check_hr(create_factory(ctypes.byref(iid_factory1), ctypes.byref(factory.ptr)), "CreateDXGIFactory1")
    return factory


def get_adapter_pointer(factory: ComObject, adapter_index: int) -> ComObject:
    adapter = ComObject()
    enum_adapters = com_method(
        factory.ptr,
        12,
        HRESULT,
        UINT,
        ctypes.POINTER(ctypes.c_void_p),
    )
    hr = enum_adapters(factory.ptr, adapter_index, ctypes.byref(adapter.ptr))
    check_hr(hr, f"EnumAdapters1({adapter_index})")
    return adapter


def adapter_desc(adapter: ComObject) -> DXGI_ADAPTER_DESC1:
    desc = DXGI_ADAPTER_DESC1()
    get_desc1 = com_method(
        adapter.ptr,
        10,
        HRESULT,
        ctypes.POINTER(DXGI_ADAPTER_DESC1),
    )
    check_hr(get_desc1(adapter.ptr, ctypes.byref(desc)), "IDXGIAdapter1.GetDesc1")
    return desc


def classify_adapter(desc: DXGI_ADAPTER_DESC1) -> str:
    if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE:
        return "Software"
    if desc.DedicatedVideoMemory >= 512 * 1024 * 1024:
        return "Discrete GPU"
    return "Integrated/shared GPU"


def create_d3d11_device(adapter: ComObject) -> tuple[ComObject, ComObject, int]:
    d3d11 = ctypes.WinDLL("d3d11.dll")
    create_device = d3d11.D3D11CreateDevice
    create_device.argtypes = [
        ctypes.c_void_p,
        UINT,
        ctypes.c_void_p,
        UINT,
        ctypes.POINTER(UINT),
        UINT,
        UINT,
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.POINTER(UINT),
        ctypes.POINTER(ctypes.c_void_p),
    ]
    create_device.restype = HRESULT

    device = ComObject()
    context = ComObject()
    selected_level = UINT()
    levels = (UINT * 2)(D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0)
    hr = create_device(
        adapter.ptr,
        D3D_DRIVER_TYPE_UNKNOWN,
        None,
        0,
        levels,
        len(levels),
        D3D11_SDK_VERSION,
        ctypes.byref(device.ptr),
        ctypes.byref(selected_level),
        ctypes.byref(context.ptr),
    )
    if hr == ctypes.c_long(0x80070057).value:
        levels_110 = (UINT * 1)(D3D_FEATURE_LEVEL_11_0)
        hr = create_device(
            adapter.ptr,
            D3D_DRIVER_TYPE_UNKNOWN,
            None,
            0,
            levels_110,
            len(levels_110),
            D3D11_SDK_VERSION,
            ctypes.byref(device.ptr),
            ctypes.byref(selected_level),
            ctypes.byref(context.ptr),
        )
    check_hr(hr, "D3D11CreateDevice")
    return device, context, int(selected_level.value)


def adapter_is_usable(index: int) -> tuple[bool, str]:
    try:
        with create_dxgi_factory() as factory:
            with get_adapter_pointer(factory, index) as adapter:
                device, context, feature_level = create_d3d11_device(adapter)
                try:
                    if feature_level < D3D_FEATURE_LEVEL_11_0:
                        return False, "Feature level below 11.0"
                    return True, ""
                finally:
                    context.release()
                    device.release()
    except Exception as exc:
        return False, str(exc)


def enumerate_adapters() -> list[AdapterInfo]:
    adapters: list[AdapterInfo] = []
    with create_dxgi_factory() as factory:
        enum_adapters = com_method(
            factory.ptr,
            12,
            HRESULT,
            UINT,
            ctypes.POINTER(ctypes.c_void_p),
        )
        index = 0
        while True:
            adapter = ComObject()
            hr = enum_adapters(factory.ptr, index, ctypes.byref(adapter.ptr))
            if hr == DXGI_ERROR_NOT_FOUND:
                break
            check_hr(hr, f"EnumAdapters1({index})")
            try:
                desc = adapter_desc(adapter)
                usable, error = adapter_is_usable(index)
                adapters.append(
                    AdapterInfo(
                        index=index,
                        name=desc.Description.strip("\x00").strip(),
                        vendor_id=int(desc.VendorId),
                        device_id=int(desc.DeviceId),
                        dedicated_vram_mb=desc.DedicatedVideoMemory / (1024 * 1024),
                        shared_memory_mb=desc.SharedSystemMemory / (1024 * 1024),
                        flags=int(desc.Flags),
                        kind=classify_adapter(desc),
                        usable=usable,
                        error=error,
                    )
                )
            finally:
                adapter.release()
            index += 1
    return adapters


HLSL_SOURCE = f"""
#define TILE {TILE_SIZE}

StructuredBuffer<float> A : register(t0);
StructuredBuffer<float> B : register(t1);
RWStructuredBuffer<float> C : register(u0);

cbuffer Params : register(b0)
{{
    uint N;
    uint Pad0;
    uint Pad1;
    uint Pad2;
}};

groupshared float TileA[TILE][TILE];
groupshared float TileB[TILE][TILE];

[numthreads(TILE, TILE, 1)]
void main(uint3 dispatch_id : SV_DispatchThreadID,
          uint3 group_thread_id : SV_GroupThreadID)
{{
    uint row = dispatch_id.y;
    uint col = dispatch_id.x;
    float sum = 0.0f;

    for (uint tile = 0; tile < N; tile += TILE)
    {{
        uint a_col = tile + group_thread_id.x;
        uint b_row = tile + group_thread_id.y;

        TileA[group_thread_id.y][group_thread_id.x] =
            (row < N && a_col < N) ? A[row * N + a_col] : 0.0f;
        TileB[group_thread_id.y][group_thread_id.x] =
            (b_row < N && col < N) ? B[b_row * N + col] : 0.0f;

        GroupMemoryBarrierWithGroupSync();

        [unroll]
        for (uint k = 0; k < TILE; ++k)
        {{
            sum += TileA[group_thread_id.y][k] * TileB[k][group_thread_id.x];
        }}

        GroupMemoryBarrierWithGroupSync();
    }}

    if (row < N && col < N)
    {{
        C[row * N + col] = sum;
    }}
}}
"""


class D3D11MatrixMultiplier:
    def __init__(self, adapter_index: int) -> None:
        self.adapter_index = adapter_index
        self.factory = create_dxgi_factory()
        self.adapter = get_adapter_pointer(self.factory, adapter_index)
        self.device, self.context, self.feature_level = create_d3d11_device(self.adapter)
        self.shader = ComObject()
        self._compile_shader()
        self._warm_up()

    def close(self) -> None:
        self._clear_state()
        self.shader.release()
        self.context.release()
        self.device.release()
        self.adapter.release()
        self.factory.release()

    def __enter__(self) -> "D3D11MatrixMultiplier":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def _compile_shader(self) -> None:
        compiler = ctypes.WinDLL("D3DCompiler_47.dll")
        compile_fn = compiler.D3DCompile
        compile_fn.argtypes = [
            ctypes.c_void_p,
            SIZE_T,
            ctypes.c_char_p,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_char_p,
            ctypes.c_char_p,
            UINT,
            UINT,
            ctypes.POINTER(ctypes.c_void_p),
            ctypes.POINTER(ctypes.c_void_p),
        ]
        compile_fn.restype = HRESULT

        source = HLSL_SOURCE.encode("utf-8")
        source_buffer = ctypes.create_string_buffer(source)
        blob = ComObject()
        error_blob = ComObject()
        hr = compile_fn(
            source_buffer,
            len(source),
            b"matmul.hlsl",
            None,
            None,
            b"main",
            b"cs_5_0",
            0,
            0,
            ctypes.byref(blob.ptr),
            ctypes.byref(error_blob.ptr),
        )
        if hr < 0:
            message = "unknown shader compiler error"
            if error_blob.value:
                message = self._blob_text(error_blob)
            error_blob.release()
            blob.release()
            raise DxError(f"D3DCompile failed with HRESULT {hresult_hex(hr)}: {message}")

        try:
            bytecode_ptr = self._blob_pointer(blob)
            bytecode_size = self._blob_size(blob)
            create_shader = com_method(
                self.device.ptr,
                18,
                HRESULT,
                ctypes.c_void_p,
                SIZE_T,
                ctypes.c_void_p,
                ctypes.POINTER(ctypes.c_void_p),
            )
            check_hr(
                create_shader(
                    self.device.ptr,
                    bytecode_ptr,
                    bytecode_size,
                    None,
                    ctypes.byref(self.shader.ptr),
                ),
                "ID3D11Device.CreateComputeShader",
            )
        finally:
            error_blob.release()
            blob.release()

    def _warm_up(self) -> None:
        a = np.ones((1, 1), dtype=np.float32)
        b = np.ones((1, 1), dtype=np.float32)
        self.multiply(a, b, use_timestamps=False)

    @staticmethod
    def _blob_pointer(blob: ComObject) -> int:
        fn = com_method(blob.ptr, 3, ctypes.c_void_p)
        return int(fn(blob.ptr))

    @staticmethod
    def _blob_size(blob: ComObject) -> int:
        fn = com_method(blob.ptr, 4, SIZE_T)
        return int(fn(blob.ptr))

    def _blob_text(self, blob: ComObject) -> str:
        ptr = self._blob_pointer(blob)
        size = self._blob_size(blob)
        return ctypes.string_at(ptr, size).decode("utf-8", errors="replace").strip()

    def _create_buffer(
        self,
        byte_width: int,
        bind_flags: int,
        misc_flags: int,
        stride: int,
        usage: int = D3D11_USAGE_DEFAULT,
        cpu_access: int = 0,
        initial_ptr: Optional[int] = None,
    ) -> ComObject:
        desc = D3D11_BUFFER_DESC(
            ByteWidth=int(byte_width),
            Usage=usage,
            BindFlags=bind_flags,
            CPUAccessFlags=cpu_access,
            MiscFlags=misc_flags,
            StructureByteStride=stride,
        )
        subresource = None
        subresource_ptr: Any = None
        if initial_ptr is not None:
            subresource = D3D11_SUBRESOURCE_DATA(
                pSysMem=initial_ptr,
                SysMemPitch=0,
                SysMemSlicePitch=0,
            )
            subresource_ptr = ctypes.byref(subresource)
        buffer = ComObject()
        create_buffer = com_method(
            self.device.ptr,
            3,
            HRESULT,
            ctypes.POINTER(D3D11_BUFFER_DESC),
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_void_p),
        )
        check_hr(
            create_buffer(
                self.device.ptr,
                ctypes.byref(desc),
                subresource_ptr,
                ctypes.byref(buffer.ptr),
            ),
            "ID3D11Device.CreateBuffer",
        )
        return buffer

    def _create_srv(self, buffer: ComObject, elements: int) -> ComObject:
        desc = D3D11_SHADER_RESOURCE_VIEW_DESC(
            Format=DXGI_FORMAT_UNKNOWN,
            ViewDimension=D3D11_SRV_DIMENSION_BUFFER,
            FirstElement=0,
            NumElements=elements,
            _unused0=0,
            _unused1=0,
        )
        srv = ComObject()
        create_srv = com_method(
            self.device.ptr,
            7,
            HRESULT,
            ctypes.c_void_p,
            ctypes.POINTER(D3D11_SHADER_RESOURCE_VIEW_DESC),
            ctypes.POINTER(ctypes.c_void_p),
        )
        check_hr(
            create_srv(self.device.ptr, buffer.ptr, ctypes.byref(desc), ctypes.byref(srv.ptr)),
            "ID3D11Device.CreateShaderResourceView",
        )
        return srv

    def _create_uav(self, buffer: ComObject, elements: int) -> ComObject:
        desc = D3D11_UNORDERED_ACCESS_VIEW_DESC(
            Format=DXGI_FORMAT_UNKNOWN,
            ViewDimension=D3D11_UAV_DIMENSION_BUFFER,
            FirstElement=0,
            NumElements=elements,
            Flags=0,
            _unused0=0,
        )
        uav = ComObject()
        create_uav = com_method(
            self.device.ptr,
            8,
            HRESULT,
            ctypes.c_void_p,
            ctypes.POINTER(D3D11_UNORDERED_ACCESS_VIEW_DESC),
            ctypes.POINTER(ctypes.c_void_p),
        )
        check_hr(
            create_uav(self.device.ptr, buffer.ptr, ctypes.byref(desc), ctypes.byref(uav.ptr)),
            "ID3D11Device.CreateUnorderedAccessView",
        )
        return uav

    def _create_query(self, query_type: int) -> Optional[ComObject]:
        desc = D3D11_QUERY_DESC(Query=query_type, MiscFlags=0)
        query_obj = ComObject()
        create_query = com_method(
            self.device.ptr,
            24,
            HRESULT,
            ctypes.POINTER(D3D11_QUERY_DESC),
            ctypes.POINTER(ctypes.c_void_p),
        )
        hr = create_query(self.device.ptr, ctypes.byref(desc), ctypes.byref(query_obj.ptr))
        if hr < 0:
            query_obj.release()
            return None
        return query_obj

    def multiply(self, a: np.ndarray, b: np.ndarray, use_timestamps: bool = True) -> GpuTiming:
        if a.shape != b.shape or a.ndim != 2 or a.shape[0] != a.shape[1]:
            raise ValueError("GPU multiplication requires square matrices with matching shapes")
        n = int(a.shape[0])
        elements = n * n
        byte_width = elements * 4
        a_contig = np.ascontiguousarray(a, dtype=np.float32)
        b_contig = np.ascontiguousarray(b, dtype=np.float32)

        start_total = time.perf_counter()

        a_buffer = self._create_buffer(
            byte_width,
            D3D11_BIND_SHADER_RESOURCE,
            D3D11_RESOURCE_MISC_BUFFER_STRUCTURED,
            4,
            initial_ptr=int(a_contig.ctypes.data),
        )
        b_buffer = self._create_buffer(
            byte_width,
            D3D11_BIND_SHADER_RESOURCE,
            D3D11_RESOURCE_MISC_BUFFER_STRUCTURED,
            4,
            initial_ptr=int(b_contig.ctypes.data),
        )
        c_buffer = self._create_buffer(
            byte_width,
            D3D11_BIND_UNORDERED_ACCESS,
            D3D11_RESOURCE_MISC_BUFFER_STRUCTURED,
            4,
        )
        staging = self._create_buffer(
            byte_width,
            0,
            0,
            0,
            usage=D3D11_USAGE_STAGING,
            cpu_access=D3D11_CPU_ACCESS_READ,
        )
        params = (ctypes.c_uint32 * 4)(n, 0, 0, 0)
        param_buffer = self._create_buffer(
            16,
            D3D11_BIND_CONSTANT_BUFFER,
            0,
            0,
            initial_ptr=ctypes.addressof(params),
        )

        srv_a = self._create_srv(a_buffer, elements)
        srv_b = self._create_srv(b_buffer, elements)
        uav_c = self._create_uav(c_buffer, elements)

        disjoint_query = self._create_query(D3D11_QUERY_TIMESTAMP_DISJOINT) if use_timestamps else None
        start_query = self._create_query(D3D11_QUERY_TIMESTAMP) if disjoint_query else None
        end_query = self._create_query(D3D11_QUERY_TIMESTAMP) if disjoint_query else None
        if disjoint_query and (not start_query or not end_query):
            disjoint_query.release()
            if start_query:
                start_query.release()
            if end_query:
                end_query.release()
            disjoint_query = None
            start_query = None
            end_query = None

        try:
            self._dispatch(
                n,
                srv_a,
                srv_b,
                uav_c,
                param_buffer,
                disjoint_query,
                start_query,
                end_query,
            )
            self._copy_resource(staging, c_buffer)
            output = self._read_staging(staging, elements).reshape((n, n))
            compute_ms = self._query_compute_ms(disjoint_query, start_query, end_query)
            total_ms = (time.perf_counter() - start_total) * 1000.0
            transfer_ms = None if compute_ms is None else max(0.0, total_ms - compute_ms)
            return GpuTiming(
                compute_ms=compute_ms,
                total_ms=total_ms,
                transfer_sync_ms=transfer_ms,
                output=output,
            )
        finally:
            self._clear_state()
            for obj in [
                end_query,
                start_query,
                disjoint_query,
                uav_c,
                srv_b,
                srv_a,
                param_buffer,
                staging,
                c_buffer,
                b_buffer,
                a_buffer,
            ]:
                if obj:
                    obj.release()

    def _dispatch(
        self,
        n: int,
        srv_a: ComObject,
        srv_b: ComObject,
        uav_c: ComObject,
        param_buffer: ComObject,
        disjoint_query: Optional[ComObject],
        start_query: Optional[ComObject],
        end_query: Optional[ComObject],
    ) -> None:
        cs_set_shader = com_method(self.context.ptr, 69, None, ctypes.c_void_p, ctypes.c_void_p, UINT)
        cs_set_srvs = com_method(self.context.ptr, 67, None, UINT, UINT, ctypes.c_void_p)
        cs_set_uavs = com_method(self.context.ptr, 68, None, UINT, UINT, ctypes.c_void_p, ctypes.c_void_p)
        cs_set_constants = com_method(self.context.ptr, 71, None, UINT, UINT, ctypes.c_void_p)
        dispatch = com_method(self.context.ptr, 41, None, UINT, UINT, UINT)
        begin = com_method(self.context.ptr, 27, None, ctypes.c_void_p)
        end = com_method(self.context.ptr, 28, None, ctypes.c_void_p)

        srv_array = (ctypes.c_void_p * 2)(srv_a.ptr.value, srv_b.ptr.value)
        uav_array = (ctypes.c_void_p * 1)(uav_c.ptr.value)
        cb_array = (ctypes.c_void_p * 1)(param_buffer.ptr.value)

        cs_set_shader(self.context.ptr, self.shader.ptr, None, 0)
        cs_set_srvs(self.context.ptr, 0, 2, ctypes.cast(srv_array, ctypes.c_void_p))
        cs_set_uavs(self.context.ptr, 0, 1, ctypes.cast(uav_array, ctypes.c_void_p), None)
        cs_set_constants(self.context.ptr, 0, 1, ctypes.cast(cb_array, ctypes.c_void_p))

        if disjoint_query and start_query and end_query:
            begin(self.context.ptr, disjoint_query.ptr)
            end(self.context.ptr, start_query.ptr)

        groups = math.ceil(n / TILE_SIZE)
        dispatch(self.context.ptr, groups, groups, 1)

        if disjoint_query and start_query and end_query:
            end(self.context.ptr, end_query.ptr)
            end(self.context.ptr, disjoint_query.ptr)

    def _copy_resource(self, destination: ComObject, source: ComObject) -> None:
        copy_resource = com_method(self.context.ptr, 47, None, ctypes.c_void_p, ctypes.c_void_p)
        copy_resource(self.context.ptr, destination.ptr, source.ptr)

    def _read_staging(self, staging: ComObject, elements: int) -> np.ndarray:
        mapped = D3D11_MAPPED_SUBRESOURCE()
        map_fn = com_method(
            self.context.ptr,
            14,
            HRESULT,
            ctypes.c_void_p,
            UINT,
            UINT,
            UINT,
            ctypes.POINTER(D3D11_MAPPED_SUBRESOURCE),
        )
        unmap_fn = com_method(self.context.ptr, 15, None, ctypes.c_void_p, UINT)
        check_hr(
            map_fn(
                self.context.ptr,
                staging.ptr,
                0,
                D3D11_MAP_READ,
                0,
                ctypes.byref(mapped),
            ),
            "ID3D11DeviceContext.Map",
        )
        try:
            result = np.empty(elements, dtype=np.float32)
            ctypes.memmove(int(result.ctypes.data), mapped.pData, elements * 4)
            return result
        finally:
            unmap_fn(self.context.ptr, staging.ptr, 0)

    def _query_compute_ms(
        self,
        disjoint_query: Optional[ComObject],
        start_query: Optional[ComObject],
        end_query: Optional[ComObject],
    ) -> Optional[float]:
        if not disjoint_query or not start_query or not end_query:
            return None

        get_data = com_method(
            self.context.ptr,
            29,
            HRESULT,
            ctypes.c_void_p,
            ctypes.c_void_p,
            UINT,
            UINT,
        )
        flush = com_method(self.context.ptr, 111, None)
        flush(self.context.ptr)

        disjoint = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT()
        deadline = time.perf_counter() + 120.0
        while True:
            hr = get_data(
                self.context.ptr,
                disjoint_query.ptr,
                ctypes.byref(disjoint),
                ctypes.sizeof(disjoint),
                0,
            )
            if hr == S_OK:
                break
            if hr != S_FALSE:
                check_hr(hr, "ID3D11DeviceContext.GetData(timestamp disjoint)")
            if time.perf_counter() > deadline:
                return None
            time.sleep(0.001)

        if disjoint.Disjoint or disjoint.Frequency == 0:
            return None

        start_ticks = ctypes.c_uint64()
        end_ticks = ctypes.c_uint64()
        check_hr(
            get_data(
                self.context.ptr,
                start_query.ptr,
                ctypes.byref(start_ticks),
                ctypes.sizeof(start_ticks),
                0,
            ),
            "ID3D11DeviceContext.GetData(timestamp start)",
        )
        check_hr(
            get_data(
                self.context.ptr,
                end_query.ptr,
                ctypes.byref(end_ticks),
                ctypes.sizeof(end_ticks),
                0,
            ),
            "ID3D11DeviceContext.GetData(timestamp end)",
        )
        return (float(end_ticks.value - start_ticks.value) / float(disjoint.Frequency)) * 1000.0

    def _clear_state(self) -> None:
        if self.context.value:
            clear_state = com_method(self.context.ptr, 110, None)
            flush = com_method(self.context.ptr, 111, None)
            clear_state(self.context.ptr)
            flush(self.context.ptr)


def generate_matrices(size: int) -> tuple[np.ndarray, np.ndarray]:
    if size <= 0:
        raise ValueError("Matrix size must be positive")
    elements = size * size
    base = np.arange(elements, dtype=np.float32)
    a = ((base % 97.0) / 97.0).reshape((size, size)).astype(np.float32, copy=False)
    b = (((base * 3.0 + 1.0) % 89.0) / 89.0).reshape((size, size)).astype(np.float32, copy=False)
    return np.ascontiguousarray(a), np.ascontiguousarray(b)


def cpu_multiply(a: np.ndarray, b: np.ndarray) -> tuple[np.ndarray, float]:
    start = time.perf_counter()
    result = np.matmul(a, b, dtype=np.float32)
    elapsed_ms = (time.perf_counter() - start) * 1000.0
    return np.ascontiguousarray(result, dtype=np.float32), elapsed_ms


def validate_result(cpu_result: np.ndarray, gpu_result: np.ndarray, size: int) -> str:
    if cpu_result.shape != gpu_result.shape:
        return f"Failed: shape mismatch CPU {cpu_result.shape}, GPU {gpu_result.shape}"
    diff = np.abs(cpu_result - gpu_result)
    max_abs = float(np.max(diff))
    scale = np.maximum(np.abs(cpu_result), np.float32(1.0))
    max_rel = float(np.max(diff / scale))
    abs_tol = max(0.02, size * 0.00005)
    rel_tol = 0.0025
    if max_abs <= abs_tol or max_rel <= rel_tol:
        return f"Passed (max abs {max_abs:.5f}, max rel {max_rel:.5f})"
    return f"Failed (max abs {max_abs:.5f}, max rel {max_rel:.5f})"


def run_single_benchmark(size: int, adapter: AdapterInfo, validate: bool = True) -> BenchmarkResult:
    a, b = generate_matrices(size)
    cpu_result, cpu_ms = cpu_multiply(a, b)
    with D3D11MatrixMultiplier(adapter.index) as gpu:
        gpu_timing = gpu.multiply(a, b, use_timestamps=True)
    validation = "Skipped"
    if validate:
        validation = validate_result(cpu_result, gpu_timing.output, size)
    speedup = cpu_ms / gpu_timing.total_ms if gpu_timing.total_ms > 0 else float("inf")
    return BenchmarkResult(
        size=size,
        adapter_label=adapter.label,
        cpu_ms=cpu_ms,
        gpu_compute_ms=gpu_timing.compute_ms,
        gpu_total_ms=gpu_timing.total_ms,
        transfer_sync_ms=gpu_timing.transfer_sync_ms,
        speedup=speedup,
        validation=validation,
    )


def run_repeat_test(
    size: int,
    adapter: AdapterInfo,
    mode: str,
    cancel_event: threading.Event,
    progress: Any,
    duration_s: float = REPEAT_SECONDS,
) -> RepeatProgress:
    a, b = generate_matrices(size)
    deadline = time.perf_counter() + duration_s
    start = time.perf_counter()
    iterations = 0
    total_ms = 0.0
    total_compute_ms = 0.0
    compute_count = 0
    latest_ms = 0.0
    last_emit = 0.0

    def emit(canceled: bool = False, force: bool = False) -> RepeatProgress:
        nonlocal last_emit
        elapsed = time.perf_counter() - start
        avg = total_ms / iterations if iterations else 0.0
        avg_compute = total_compute_ms / compute_count if compute_count else None
        update = RepeatProgress(
            mode=mode,
            size=size,
            elapsed_s=min(elapsed, duration_s),
            iterations=iterations,
            latest_ms=latest_ms,
            average_ms=avg,
            average_compute_ms=avg_compute,
            canceled=canceled,
        )
        if force or elapsed - last_emit >= 0.1:
            progress(update)
            last_emit = elapsed
        return update

    if mode == "CPU":
        while time.perf_counter() < deadline and not cancel_event.is_set():
            _, latest_ms = cpu_multiply(a, b)
            total_ms += latest_ms
            iterations += 1
            emit()
        return emit(cancel_event.is_set(), force=True)

    with D3D11MatrixMultiplier(adapter.index) as gpu:
        while time.perf_counter() < deadline and not cancel_event.is_set():
            timing = gpu.multiply(a, b, use_timestamps=True)
            latest_ms = timing.total_ms
            total_ms += latest_ms
            if timing.compute_ms is not None:
                total_compute_ms += timing.compute_ms
                compute_count += 1
            iterations += 1
            emit()
    return emit(cancel_event.is_set(), force=True)


def format_ms(value: Optional[float]) -> str:
    if value is None:
        return "N/A"
    if value >= 1000.0:
        return f"{value:,.1f}"
    return f"{value:,.3f}"


def format_speedup(value: float) -> str:
    if math.isinf(value):
        return "inf"
    return f"{value:.2f}x"


class HardwareAccelApp:
    def __init__(self) -> None:
        import tkinter as tk
        from tkinter import ttk

        self.tk = tk
        self.ttk = ttk
        self.root = tk.Tk()
        self.root.title(APP_TITLE)
        self.root.minsize(1080, 700)

        self.queue: queue.Queue[tuple[str, Any]] = queue.Queue()
        self.adapters: list[AdapterInfo] = []
        self.worker: Optional[threading.Thread] = None
        self.cancel_event = threading.Event()

        self.adapter_var = tk.StringVar()
        self.size_var = tk.StringVar(value=str(DEFAULT_SIZES[1]))
        self.validate_var = tk.BooleanVar(value=True)
        self.repeat_mode_var = tk.StringVar(value="GPU")
        self.status_var = tk.StringVar(value="Enumerating GPUs...")
        self.progress_var = tk.DoubleVar(value=0.0)

        self._build_ui()
        self._set_busy(True)
        threading.Thread(target=self._load_adapters_worker, daemon=True).start()
        self.root.after(100, self._poll_queue)

    def run(self) -> None:
        self.root.mainloop()

    def _build_ui(self) -> None:
        tk = self.tk
        ttk = self.ttk

        container = ttk.Frame(self.root, padding=12)
        container.pack(fill=tk.BOTH, expand=True)
        container.columnconfigure(0, weight=1)
        container.rowconfigure(2, weight=1)

        controls = ttk.LabelFrame(container, text="Benchmark controls", padding=10)
        controls.grid(row=0, column=0, sticky="ew")
        for col in range(8):
            controls.columnconfigure(col, weight=0)
        controls.columnconfigure(1, weight=1)

        ttk.Label(controls, text="GPU adapter").grid(row=0, column=0, sticky="w", padx=(0, 8), pady=4)
        self.adapter_combo = ttk.Combobox(
            controls,
            textvariable=self.adapter_var,
            state="readonly",
            width=70,
        )
        self.adapter_combo.grid(row=0, column=1, columnspan=5, sticky="ew", pady=4)
        self.refresh_button = ttk.Button(controls, text="Refresh GPUs", command=self._refresh_adapters)
        self.refresh_button.grid(row=0, column=6, padx=(8, 0), pady=4)

        ttk.Label(controls, text="Matrix size").grid(row=1, column=0, sticky="w", padx=(0, 8), pady=4)
        self.size_combo = ttk.Combobox(
            controls,
            textvariable=self.size_var,
            values=[str(size) for size in DEFAULT_SIZES],
            width=12,
        )
        self.size_combo.grid(row=1, column=1, sticky="w", pady=4)

        self.validate_check = ttk.Checkbutton(
            controls,
            text="Validate GPU output",
            variable=self.validate_var,
        )
        self.validate_check.grid(row=1, column=2, sticky="w", padx=(16, 0), pady=4)

        self.run_button = ttk.Button(controls, text="Run benchmark", command=self._run_benchmark)
        self.run_button.grid(row=1, column=3, padx=(16, 0), pady=4)

        ttk.Label(controls, text="Repeat mode").grid(row=2, column=0, sticky="w", padx=(0, 8), pady=4)
        self.repeat_mode_combo = ttk.Combobox(
            controls,
            textvariable=self.repeat_mode_var,
            values=["GPU", "CPU"],
            state="readonly",
            width=12,
        )
        self.repeat_mode_combo.grid(row=2, column=1, sticky="w", pady=4)
        self.repeat_button = ttk.Button(
            controls,
            text="Start 1-minute repeat",
            command=self._start_repeat,
        )
        self.repeat_button.grid(row=2, column=2, sticky="w", padx=(16, 0), pady=4)
        self.cancel_button = ttk.Button(
            controls,
            text="Cancel repeat",
            command=self._cancel_repeat,
            state=tk.DISABLED,
        )
        self.cancel_button.grid(row=2, column=3, sticky="w", padx=(8, 0), pady=4)

        status = ttk.Frame(container)
        status.grid(row=1, column=0, sticky="ew", pady=(10, 6))
        status.columnconfigure(0, weight=1)
        ttk.Label(status, textvariable=self.status_var).grid(row=0, column=0, sticky="w")
        self.progress = ttk.Progressbar(status, variable=self.progress_var, maximum=100.0)
        self.progress.grid(row=1, column=0, sticky="ew", pady=(5, 0))

        results_frame = ttk.LabelFrame(container, text="Results", padding=10)
        results_frame.grid(row=2, column=0, sticky="nsew")
        results_frame.columnconfigure(0, weight=1)
        results_frame.rowconfigure(0, weight=1)

        columns = (
            "size",
            "cpu",
            "gpu_compute",
            "gpu_total",
            "transfer",
            "speedup",
            "adapter",
            "validation",
        )
        self.results = ttk.Treeview(results_frame, columns=columns, show="headings", height=12)
        headings = {
            "size": "Size",
            "cpu": "CPU ms",
            "gpu_compute": "GPU compute ms",
            "gpu_total": "GPU total ms",
            "transfer": "Transfer/sync ms",
            "speedup": "CPU/GPU total",
            "adapter": "Adapter",
            "validation": "Validation",
        }
        widths = {
            "size": 80,
            "cpu": 100,
            "gpu_compute": 130,
            "gpu_total": 115,
            "transfer": 130,
            "speedup": 110,
            "adapter": 280,
            "validation": 260,
        }
        for column in columns:
            self.results.heading(column, text=headings[column])
            self.results.column(column, width=widths[column], minwidth=70, anchor=tk.W)
        self.results.grid(row=0, column=0, sticky="nsew")
        scrollbar = ttk.Scrollbar(results_frame, orient=tk.VERTICAL, command=self.results.yview)
        scrollbar.grid(row=0, column=1, sticky="ns")
        self.results.configure(yscrollcommand=scrollbar.set)

        log_frame = ttk.LabelFrame(container, text="Log", padding=10)
        log_frame.grid(row=3, column=0, sticky="nsew", pady=(10, 0))
        log_frame.columnconfigure(0, weight=1)
        log_frame.rowconfigure(0, weight=1)
        self.log = tk.Text(log_frame, height=8, wrap=tk.WORD)
        self.log.grid(row=0, column=0, sticky="nsew")
        log_scroll = ttk.Scrollbar(log_frame, orient=tk.VERTICAL, command=self.log.yview)
        log_scroll.grid(row=0, column=1, sticky="ns")
        self.log.configure(yscrollcommand=log_scroll.set)

    def _set_busy(self, busy: bool, repeat: bool = False) -> None:
        state = self.tk.DISABLED if busy else self.tk.NORMAL
        readonly_state = self.tk.DISABLED if busy else "readonly"
        self.adapter_combo.configure(state=readonly_state)
        self.refresh_button.configure(state=state)
        self.size_combo.configure(state=state)
        self.validate_check.configure(state=state)
        self.run_button.configure(state=state)
        self.repeat_mode_combo.configure(state=readonly_state)
        self.repeat_button.configure(state=state)
        self.cancel_button.configure(state=(self.tk.NORMAL if repeat else self.tk.DISABLED))

    def _append_log(self, message: str) -> None:
        self.log.insert(self.tk.END, f"{time.strftime('%H:%M:%S')}  {message}\n")
        self.log.see(self.tk.END)

    def _load_adapters_worker(self) -> None:
        try:
            adapters = enumerate_adapters()
            self.queue.put(("adapters", adapters))
        except Exception:
            self.queue.put(("error", traceback.format_exc()))

    def _refresh_adapters(self) -> None:
        if self.worker and self.worker.is_alive():
            return
        self.status_var.set("Refreshing GPU list...")
        self.progress_var.set(0.0)
        self._set_busy(True)
        threading.Thread(target=self._load_adapters_worker, daemon=True).start()

    def _selected_adapter(self) -> AdapterInfo:
        selected = self.adapter_var.get()
        for adapter in self.adapters:
            if adapter.label == selected:
                if not adapter.usable:
                    raise RuntimeError(f"Selected adapter is unavailable: {adapter.error}")
                return adapter
        usable = [adapter for adapter in self.adapters if adapter.usable]
        if not usable:
            raise RuntimeError("No usable Direct3D 11 GPU adapters were found")
        return usable[0]

    def _selected_size(self) -> int:
        text = self.size_var.get().strip()
        try:
            size = int(text)
        except ValueError as exc:
            raise ValueError("Matrix size must be an integer") from exc
        if size <= 0:
            raise ValueError("Matrix size must be positive")
        if size > 8192:
            raise ValueError("Matrix size is too large for this first version")
        return size

    def _run_benchmark(self) -> None:
        try:
            size = self._selected_size()
            adapter = self._selected_adapter()
        except Exception as exc:
            self.status_var.set(str(exc))
            self._append_log(str(exc))
            return

        self._set_busy(True)
        self.progress_var.set(0.0)
        self.status_var.set(f"Running benchmark for {size} x {size}...")
        self._append_log(f"Starting single benchmark on {adapter.label}")

        def worker() -> None:
            try:
                result = run_single_benchmark(size, adapter, self.validate_var.get())
                self.queue.put(("benchmark_result", result))
            except Exception:
                self.queue.put(("error", traceback.format_exc()))

        self.worker = threading.Thread(target=worker, daemon=True)
        self.worker.start()

    def _start_repeat(self) -> None:
        try:
            size = self._selected_size()
            adapter = self._selected_adapter()
        except Exception as exc:
            self.status_var.set(str(exc))
            self._append_log(str(exc))
            return

        mode = self.repeat_mode_var.get()
        self.cancel_event.clear()
        self._set_busy(True, repeat=True)
        self.progress_var.set(0.0)
        self.status_var.set(f"Running {mode} repeat test for 60 seconds...")
        self._append_log(f"Starting 1-minute {mode} repeat test at {size} x {size}")

        def progress(update: RepeatProgress) -> None:
            self.queue.put(("repeat_progress", update))

        def worker() -> None:
            try:
                result = run_repeat_test(size, adapter, mode, self.cancel_event, progress)
                self.queue.put(("repeat_done", result))
            except Exception:
                self.queue.put(("error", traceback.format_exc()))

        self.worker = threading.Thread(target=worker, daemon=True)
        self.worker.start()

    def _cancel_repeat(self) -> None:
        self.cancel_event.set()
        self.status_var.set("Cancel requested. Waiting for the current iteration to finish...")
        self._append_log("Cancel requested for repeat test")

    def _poll_queue(self) -> None:
        while True:
            try:
                kind, payload = self.queue.get_nowait()
            except queue.Empty:
                break
            if kind == "adapters":
                self._handle_adapters(payload)
            elif kind == "benchmark_result":
                self._handle_benchmark_result(payload)
            elif kind == "repeat_progress":
                self._handle_repeat_progress(payload)
            elif kind == "repeat_done":
                self._handle_repeat_done(payload)
            elif kind == "error":
                self._handle_error(payload)
        self.root.after(100, self._poll_queue)

    def _handle_adapters(self, adapters: list[AdapterInfo]) -> None:
        self.adapters = adapters
        labels = [adapter.label for adapter in adapters]
        self.adapter_combo.configure(values=labels)
        selected = next((adapter.label for adapter in adapters if adapter.usable and adapter.kind != "Software"), "")
        if not selected:
            selected = next((adapter.label for adapter in adapters if adapter.usable), "")
        if selected:
            self.adapter_var.set(selected)
            self.status_var.set(f"Found {len(adapters)} adapter(s). Ready.")
        else:
            self.status_var.set("No usable Direct3D 11 GPU adapters found.")
        self.progress_var.set(0.0)
        self._set_busy(False)
        self._append_log(f"Found {len(adapters)} adapter(s)")
        for adapter in adapters:
            suffix = "" if adapter.usable else f" unavailable: {adapter.error}"
            self._append_log(f"GPU {adapter.index}: {adapter.label}{suffix}")

    def _handle_benchmark_result(self, result: BenchmarkResult) -> None:
        self.results.insert(
            "",
            self.tk.END,
            values=(
                f"{result.size} x {result.size}",
                format_ms(result.cpu_ms),
                format_ms(result.gpu_compute_ms),
                format_ms(result.gpu_total_ms),
                format_ms(result.transfer_sync_ms),
                format_speedup(result.speedup),
                result.adapter_label,
                result.validation,
            ),
        )
        self.progress_var.set(100.0)
        self.status_var.set("Benchmark complete.")
        self._append_log(
            "Benchmark complete: "
            f"CPU {format_ms(result.cpu_ms)} ms, "
            f"GPU total {format_ms(result.gpu_total_ms)} ms, "
            f"GPU compute {format_ms(result.gpu_compute_ms)} ms"
        )
        self._set_busy(False)

    def _handle_repeat_progress(self, update: RepeatProgress) -> None:
        percent = min(100.0, (update.elapsed_s / REPEAT_SECONDS) * 100.0)
        self.progress_var.set(percent)
        compute = format_ms(update.average_compute_ms)
        self.status_var.set(
            f"{update.mode} repeat: {update.elapsed_s:0.1f}s, "
            f"{update.iterations} iteration(s), latest {format_ms(update.latest_ms)} ms, "
            f"avg {format_ms(update.average_ms)} ms, compute avg {compute}"
        )

    def _handle_repeat_done(self, result: RepeatProgress) -> None:
        self.progress_var.set(100.0 if not result.canceled else self.progress_var.get())
        state = "canceled" if result.canceled else "complete"
        self.status_var.set(
            f"Repeat test {state}: {result.iterations} iteration(s), "
            f"avg {format_ms(result.average_ms)} ms"
        )
        self._append_log(
            f"Repeat test {state}: mode={result.mode}, size={result.size}, "
            f"iterations={result.iterations}, avg={format_ms(result.average_ms)} ms, "
            f"compute_avg={format_ms(result.average_compute_ms)} ms"
        )
        self._set_busy(False)

    def _handle_error(self, details: str) -> None:
        last_line = details.strip().splitlines()[-1] if details.strip() else "Unknown error"
        self.status_var.set(last_line)
        self._append_log(details)
        self._set_busy(False)


def print_adapters() -> list[AdapterInfo]:
    adapters = enumerate_adapters()
    if not adapters:
        print("No DXGI adapters found.")
        return []
    for adapter in adapters:
        usable = "usable" if adapter.usable else f"unavailable: {adapter.error}"
        print(
            f"[{adapter.index}] {adapter.name} | {adapter.kind} | "
            f"dedicated={adapter.dedicated_vram_mb:.0f} MB | "
            f"shared={adapter.shared_memory_mb:.0f} MB | {usable}"
        )
    return adapters


def run_self_test(size: int, adapter_index: Optional[int]) -> int:
    adapters = print_adapters()
    usable = [adapter for adapter in adapters if adapter.usable]
    if not usable:
        print("Self-test failed: no usable Direct3D 11 adapter.")
        return 2
    adapter = next((item for item in usable if item.index == adapter_index), usable[0])
    print(f"\nRunning self-test on adapter {adapter.index}: {adapter.name}")
    result = run_single_benchmark(size, adapter, validate=True)
    print(f"Size: {size} x {size}")
    print(f"CPU: {format_ms(result.cpu_ms)} ms")
    print(f"GPU compute: {format_ms(result.gpu_compute_ms)} ms")
    print(f"GPU total: {format_ms(result.gpu_total_ms)} ms")
    print(f"Transfer/sync: {format_ms(result.transfer_sync_ms)} ms")
    print(f"Speedup: {format_speedup(result.speedup)}")
    print(f"Validation: {result.validation}")
    return 0 if result.validation.startswith("Passed") else 1


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=APP_TITLE)
    parser.add_argument("--list-gpus", action="store_true", help="List available DXGI adapters and exit")
    parser.add_argument("--self-test", action="store_true", help="Run a small CPU/GPU benchmark and exit")
    parser.add_argument("--size", type=int, default=64, help="Matrix size for --self-test")
    parser.add_argument("--adapter", type=int, default=None, help="Adapter index for --self-test")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    if sys.platform != "win32":
        print("This implementation uses Direct3D 11 and currently runs on Windows only.")
        return 2
    args = parse_args(argv)
    if args.list_gpus:
        print_adapters()
        return 0
    if args.self_test:
        return run_self_test(args.size, args.adapter)
    app = HardwareAccelApp()
    app.run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
