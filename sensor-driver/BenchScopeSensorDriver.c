#include "BenchScopeSensorDriver.h"
#include <intrin.h>

static WDFDEVICE g_ControlDevice = NULL;
static volatile LONG64 g_SnapshotSequence = 0;

#define BENCHSCOPE_MSR_IA32_THERM_STATUS 0x0000019Cu
#define BENCHSCOPE_MSR_IA32_TEMPERATURE_TARGET 0x000001A2u
#define BENCHSCOPE_MSR_RAPL_POWER_UNIT 0x00000606u
#define BENCHSCOPE_MSR_PKG_ENERGY_STATUS 0x00000611u

typedef struct BENCHSCOPE_CPU_TELEMETRY {
    BOOLEAN Supported;
    BOOLEAN HasTemperature;
    BOOLEAN HasThermalLimit;
    BOOLEAN ThermalThrottled;
    BOOLEAN HasEnergy;
    int TemperatureMilliC;
    int ThermalLimitMilliC;
    unsigned long long EnergyMilliJoules;
} BENCHSCOPE_CPU_TELEMETRY;

static VOID BenchScopeSensorCompleteBufferedRequest(
    _In_ WDFREQUEST Request,
    _In_ NTSTATUS Status,
    _In_ size_t Information
    )
{
    WdfRequestCompleteWithInformation(Request, Status, Information);
}

static VOID BenchScopeSensorCopyWideString(
    _Out_writes_(DestinationCount) wchar_t* Destination,
    _In_ size_t DestinationCount,
    _In_z_ const wchar_t* Source
    )
{
    size_t index = 0;

    if (DestinationCount == 0) {
        return;
    }

    for (; index + 1 < DestinationCount && Source[index] != L'\0'; ++index) {
        Destination[index] = Source[index];
    }
    Destination[index] = L'\0';
}

static VOID BenchScopeSensorAppendWideString(
    _Inout_updates_(DestinationCount) wchar_t* Destination,
    _In_ size_t DestinationCount,
    _In_z_ const wchar_t* Suffix
    )
{
    size_t index = 0;
    size_t suffixIndex = 0;

    if (DestinationCount == 0) {
        return;
    }

    while (index + 1 < DestinationCount && Destination[index] != L'\0') {
        ++index;
    }

    while (index + 1 < DestinationCount && Suffix[suffixIndex] != L'\0') {
        Destination[index++] = Suffix[suffixIndex++];
    }
    Destination[index] = L'\0';
}

static VOID BenchScopeSensorFillUnsupportedReading(
    _Out_ BENCHSCOPE_SENSOR_READING* Reading,
    _In_ BENCHSCOPE_SENSOR_KIND Kind,
    _In_z_ const wchar_t* Label,
    _In_z_ const wchar_t* Provider
    )
{
    RtlZeroMemory(Reading, sizeof(*Reading));
    Reading->kind = (unsigned int)Kind;
    Reading->status = (unsigned int)BenchScopeSensorStatusUnsupported;
    Reading->flags = 0;
    Reading->temperatureMilliC = 0;
    Reading->utilizationMilliPercent = 0;
    BenchScopeSensorCopyWideString(Reading->label, RTL_NUMBER_OF(Reading->label), Label);
    BenchScopeSensorCopyWideString(Reading->provider, RTL_NUMBER_OF(Reading->provider), Provider);
}

static VOID BenchScopeSensorFillOkTemperatureReading(
    _Out_ BENCHSCOPE_SENSOR_READING* Reading,
    _In_ BENCHSCOPE_SENSOR_KIND Kind,
    _In_z_ const wchar_t* Label,
    _In_z_ const wchar_t* Provider,
    _In_ int TemperatureMilliC
    )
{
    RtlZeroMemory(Reading, sizeof(*Reading));
    Reading->kind = (unsigned int)Kind;
    Reading->status = (unsigned int)BenchScopeSensorStatusOk;
    Reading->flags = BENCHSCOPE_SENSOR_READING_HAS_TEMPERATURE;
    Reading->temperatureMilliC = TemperatureMilliC;
    Reading->utilizationMilliPercent = 0;
    BenchScopeSensorCopyWideString(Reading->label, RTL_NUMBER_OF(Reading->label), Label);
    BenchScopeSensorCopyWideString(Reading->provider, RTL_NUMBER_OF(Reading->provider), Provider);
}

static VOID BenchScopeSensorFillAdvancedUnsupported(
    _Out_ BENCHSCOPE_SENSOR_ADVANCED_READING* Reading,
    _In_ BENCHSCOPE_SENSOR_KIND Kind,
    _In_z_ const wchar_t* Label,
    _In_z_ const wchar_t* Provider,
    _In_z_ const wchar_t* Detail
    )
{
    RtlZeroMemory(Reading, sizeof(*Reading));
    Reading->kind = (unsigned int)Kind;
    Reading->status = (unsigned int)BenchScopeSensorStatusUnsupported;
    Reading->flags = 0;
    BenchScopeSensorCopyWideString(Reading->label, RTL_NUMBER_OF(Reading->label), Label);
    BenchScopeSensorCopyWideString(Reading->provider, RTL_NUMBER_OF(Reading->provider), Provider);
    BenchScopeSensorCopyWideString(Reading->detail, RTL_NUMBER_OF(Reading->detail), Detail);
}

static VOID BenchScopeSensorFillAdvancedCpu(
    _Out_ BENCHSCOPE_SENSOR_ADVANCED_READING* Reading,
    _In_ const BENCHSCOPE_CPU_TELEMETRY* Telemetry
    )
{
    RtlZeroMemory(Reading, sizeof(*Reading));
    Reading->kind = (unsigned int)BenchScopeSensorKindCpu;
    Reading->status = Telemetry->Supported
        ? (unsigned int)BenchScopeSensorStatusOk
        : (unsigned int)BenchScopeSensorStatusUnsupported;
    Reading->temperatureMilliC = Telemetry->TemperatureMilliC;
    Reading->thermalLimitMilliC = Telemetry->ThermalLimitMilliC;
    Reading->energyMilliJoules = Telemetry->EnergyMilliJoules;
    BenchScopeSensorCopyWideString(Reading->label, RTL_NUMBER_OF(Reading->label), L"CPU package");
    BenchScopeSensorCopyWideString(
        Reading->provider,
        RTL_NUMBER_OF(Reading->provider),
        L"BenchScope CPU telemetry driver");

    if (Telemetry->HasTemperature) {
        Reading->flags |= BENCHSCOPE_SENSOR_ADVANCED_HAS_TEMPERATURE;
    }
    if (Telemetry->HasThermalLimit) {
        Reading->flags |= BENCHSCOPE_SENSOR_ADVANCED_HAS_THERMAL_LIMIT;
    }
    if (Telemetry->ThermalThrottled) {
        Reading->flags |= BENCHSCOPE_SENSOR_ADVANCED_THERMAL_THROTTLED;
    }
    if (Telemetry->HasEnergy) {
        Reading->flags |= BENCHSCOPE_SENSOR_ADVANCED_HAS_ENERGY;
    }

    if (Telemetry->Supported) {
        BenchScopeSensorCopyWideString(
            Reading->detail,
            RTL_NUMBER_OF(Reading->detail),
            L"Intel family 6 CPU telemetry; package power is derived by the service from energy deltas.");
    } else {
        BenchScopeSensorCopyWideString(
            Reading->detail,
            RTL_NUMBER_OF(Reading->detail),
            L"CPU model or telemetry MSRs are not enabled by the driver allowlist.");
    }
}

static NTSTATUS BenchScopeSensorReadMsr(
    _In_ unsigned long Register,
    _Out_ unsigned __int64* Value
    )
{
    NTSTATUS status = STATUS_SUCCESS;

    __try {
        *Value = __readmsr(Register);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        status = GetExceptionCode();
    }

    return status;
}

static BOOLEAN BenchScopeSensorCpuIsAllowedIntelFamily6(VOID)
{
    int registers[4] = { 0 };
    char vendor[13] = { 0 };
    unsigned int family = 0;
    unsigned int extendedFamily = 0;

    __cpuid(registers, 0);
    *((int*)&vendor[0]) = registers[1];
    *((int*)&vendor[4]) = registers[3];
    *((int*)&vendor[8]) = registers[2];
    if (RtlCompareMemory(vendor, "GenuineIntel", 12) != 12) {
        return FALSE;
    }

    __cpuid(registers, 1);
    family = ((unsigned int)registers[0] >> 8) & 0xFu;
    extendedFamily = ((unsigned int)registers[0] >> 20) & 0xFFu;
    if (family == 0xFu) {
        family += extendedFamily;
    }

    return family == 6;
}

static BENCHSCOPE_CPU_TELEMETRY BenchScopeSensorReadCpuTelemetry(VOID)
{
    BENCHSCOPE_CPU_TELEMETRY telemetry;
    unsigned __int64 thermStatus = 0;
    unsigned __int64 tempTarget = 0;
    unsigned __int64 raplUnits = 0;
    unsigned __int64 packageEnergy = 0;
    unsigned int tjMaxC = 0;
    unsigned int deltaToTjMaxC = 0;
    unsigned int energyUnitExponent = 0;
    unsigned long long divisor = 1;

    RtlZeroMemory(&telemetry, sizeof(telemetry));

    if (!BenchScopeSensorCpuIsAllowedIntelFamily6()) {
        return telemetry;
    }

    if (!NT_SUCCESS(BenchScopeSensorReadMsr(BENCHSCOPE_MSR_IA32_TEMPERATURE_TARGET, &tempTarget)) ||
        !NT_SUCCESS(BenchScopeSensorReadMsr(BENCHSCOPE_MSR_IA32_THERM_STATUS, &thermStatus))) {
        return telemetry;
    }

    tjMaxC = (unsigned int)((tempTarget >> 16) & 0xFFu);
    deltaToTjMaxC = (unsigned int)((thermStatus >> 16) & 0x7Fu);
    if ((thermStatus & (1ULL << 31)) == 0 ||
        tjMaxC < 50 ||
        tjMaxC > 125 ||
        deltaToTjMaxC > tjMaxC) {
        return telemetry;
    }

    telemetry.Supported = TRUE;
    telemetry.HasTemperature = TRUE;
    telemetry.HasThermalLimit = TRUE;
    telemetry.TemperatureMilliC = ((int)tjMaxC - (int)deltaToTjMaxC) * 1000;
    telemetry.ThermalLimitMilliC = (int)tjMaxC * 1000;
    telemetry.ThermalThrottled = (thermStatus & 0x1ULL) != 0;

    if (NT_SUCCESS(BenchScopeSensorReadMsr(BENCHSCOPE_MSR_RAPL_POWER_UNIT, &raplUnits)) &&
        NT_SUCCESS(BenchScopeSensorReadMsr(BENCHSCOPE_MSR_PKG_ENERGY_STATUS, &packageEnergy))) {
        energyUnitExponent = (unsigned int)((raplUnits >> 8) & 0x1Fu);
        if (energyUnitExponent < 32) {
            divisor = 1ULL << energyUnitExponent;
            telemetry.EnergyMilliJoules =
                (((unsigned long long)(packageEnergy & 0xFFFFFFFFULL)) * 1000ULL) / divisor;
            telemetry.HasEnergy = TRUE;
        }
    }

    return telemetry;
}

static NTSTATUS BenchScopeSensorWriteVersion(_In_ WDFREQUEST Request)
{
    BENCHSCOPE_SENSOR_VERSION* version = NULL;
    NTSTATUS status = WdfRequestRetrieveOutputBuffer(
        Request,
        sizeof(BENCHSCOPE_SENSOR_VERSION),
        (PVOID*)&version,
        NULL);

    if (!NT_SUCCESS(status)) {
        return status;
    }

    version->protocolVersion = BENCHSCOPE_SENSOR_PROTOCOL_VERSION;
    version->driverMajor = BENCHSCOPE_SENSOR_VERSION_MAJOR;
    version->driverMinor = BENCHSCOPE_SENSOR_VERSION_MINOR;
    version->driverPatch = BENCHSCOPE_SENSOR_VERSION_PATCH;
    BenchScopeSensorCompleteBufferedRequest(Request, STATUS_SUCCESS, sizeof(*version));
    return STATUS_SUCCESS;
}

static NTSTATUS BenchScopeSensorWriteCapabilities(_In_ WDFREQUEST Request)
{
    BENCHSCOPE_SENSOR_CAPABILITIES* capabilities = NULL;
    BENCHSCOPE_CPU_TELEMETRY cpuTelemetry;
    NTSTATUS status = WdfRequestRetrieveOutputBuffer(
        Request,
        sizeof(BENCHSCOPE_SENSOR_CAPABILITIES),
        (PVOID*)&capabilities,
        NULL);

    if (!NT_SUCCESS(status)) {
        return status;
    }

    RtlZeroMemory(capabilities, sizeof(*capabilities));
    cpuTelemetry = BenchScopeSensorReadCpuTelemetry();
    capabilities->protocolVersion = BENCHSCOPE_SENSOR_PROTOCOL_VERSION;
    capabilities->supportsCpuTemperature = cpuTelemetry.HasTemperature ? 1u : 0u;
    capabilities->supportsGpuTemperature = 0;
    capabilities->supportsDriveTemperature = 0;
    capabilities->supportsUtilization = 0;
    capabilities->reserved[0] = BENCHSCOPE_SENSOR_PROVIDER_CPU_TELEMETRY |
        BENCHSCOPE_SENSOR_PROVIDER_STORAGE_HEALTH_USER_MODE;
    if (cpuTelemetry.HasEnergy) {
        capabilities->reserved[1] |= BENCHSCOPE_SENSOR_ADVANCED_HAS_ENERGY;
    }
    BenchScopeSensorCompleteBufferedRequest(Request, STATUS_SUCCESS, sizeof(*capabilities));
    return STATUS_SUCCESS;
}

static NTSTATUS BenchScopeSensorWriteSnapshot(_In_ WDFREQUEST Request)
{
    BENCHSCOPE_SENSOR_SNAPSHOT* snapshot = NULL;
    LARGE_INTEGER qpc = { 0 };
    BENCHSCOPE_CPU_TELEMETRY cpuTelemetry;
    NTSTATUS status = WdfRequestRetrieveOutputBuffer(
        Request,
        sizeof(BENCHSCOPE_SENSOR_SNAPSHOT),
        (PVOID*)&snapshot,
        NULL);

    if (!NT_SUCCESS(status)) {
        return status;
    }

    KeQueryPerformanceCounter(&qpc);
    RtlZeroMemory(snapshot, sizeof(*snapshot));
    snapshot->protocolVersion = BENCHSCOPE_SENSOR_PROTOCOL_VERSION;
    snapshot->readingCount = 4;
    snapshot->sequence = (unsigned long long)InterlockedIncrement64(&g_SnapshotSequence);
    snapshot->timestampQpc = qpc.QuadPart;

    cpuTelemetry = BenchScopeSensorReadCpuTelemetry();
    if (cpuTelemetry.HasTemperature) {
        BenchScopeSensorFillOkTemperatureReading(
            &snapshot->readings[0],
            BenchScopeSensorKindCpu,
            L"CPU package",
            L"BenchScope CPU telemetry driver",
            cpuTelemetry.TemperatureMilliC);
        if (cpuTelemetry.ThermalThrottled) {
            BenchScopeSensorAppendWideString(
                snapshot->readings[0].provider,
                RTL_NUMBER_OF(snapshot->readings[0].provider),
                L" (thermal limit active)");
        }
    } else {
        BenchScopeSensorFillUnsupportedReading(
            &snapshot->readings[0],
            BenchScopeSensorKindCpu,
            L"CPU",
            L"BenchScope CPU telemetry driver");
    }
    BenchScopeSensorFillUnsupportedReading(
        &snapshot->readings[1],
        BenchScopeSensorKindGpu,
        L"GPU",
        L"BenchScope sensor driver prototype");
    BenchScopeSensorFillUnsupportedReading(
        &snapshot->readings[2],
        BenchScopeSensorKindDrive,
        L"SSD",
        L"BenchScope sensor driver prototype");
    BenchScopeSensorFillUnsupportedReading(
        &snapshot->readings[3],
        BenchScopeSensorKindMemory,
        L"RAM",
        L"BenchScope sensor driver prototype");

    BenchScopeSensorCompleteBufferedRequest(Request, STATUS_SUCCESS, sizeof(*snapshot));
    return STATUS_SUCCESS;
}

static NTSTATUS BenchScopeSensorWriteAdvancedTelemetry(_In_ WDFREQUEST Request)
{
    BENCHSCOPE_SENSOR_ADVANCED_TELEMETRY* telemetry = NULL;
    LARGE_INTEGER qpc = { 0 };
    BENCHSCOPE_CPU_TELEMETRY cpuTelemetry;
    NTSTATUS status = WdfRequestRetrieveOutputBuffer(
        Request,
        sizeof(BENCHSCOPE_SENSOR_ADVANCED_TELEMETRY),
        (PVOID*)&telemetry,
        NULL);

    if (!NT_SUCCESS(status)) {
        return status;
    }

    KeQueryPerformanceCounter(&qpc);
    RtlZeroMemory(telemetry, sizeof(*telemetry));
    telemetry->protocolVersion = BENCHSCOPE_SENSOR_PROTOCOL_VERSION;
    telemetry->providerMask =
        BENCHSCOPE_SENSOR_PROVIDER_CPU_TELEMETRY |
        BENCHSCOPE_SENSOR_PROVIDER_STORAGE_HEALTH_USER_MODE;
    telemetry->readingCount = 3;
    telemetry->sequence = (unsigned long long)InterlockedIncrement64(&g_SnapshotSequence);
    telemetry->timestampQpc = qpc.QuadPart;

    cpuTelemetry = BenchScopeSensorReadCpuTelemetry();
    BenchScopeSensorFillAdvancedCpu(&telemetry->readings[0], &cpuTelemetry);
    BenchScopeSensorFillAdvancedUnsupported(
        &telemetry->readings[1],
        BenchScopeSensorKindMotherboard,
        L"Motherboard / Super I/O",
        L"BenchScope motherboard telemetry driver",
        L"Not enabled. Super I/O support requires a chip and board allowlist before any port access.");
    BenchScopeSensorFillAdvancedUnsupported(
        &telemetry->readings[2],
        BenchScopeSensorKindStorageHealth,
        L"NVMe / storage health",
        L"BenchScope sensor service",
        L"Implemented as a user-mode storage provider; kernel storage filtering is intentionally not used.");
    telemetry->readings[2].flags = BENCHSCOPE_SENSOR_ADVANCED_USER_MODE_PROVIDER;

    BenchScopeSensorCompleteBufferedRequest(Request, STATUS_SUCCESS, sizeof(*telemetry));
    return STATUS_SUCCESS;
}

VOID BenchScopeSensorEvtIoDeviceControl(
    _In_ WDFQUEUE Queue,
    _In_ WDFREQUEST Request,
    _In_ size_t OutputBufferLength,
    _In_ size_t InputBufferLength,
    _In_ ULONG IoControlCode
    )
{
    NTSTATUS status = STATUS_INVALID_DEVICE_REQUEST;

    UNREFERENCED_PARAMETER(Queue);
    UNREFERENCED_PARAMETER(OutputBufferLength);
    UNREFERENCED_PARAMETER(InputBufferLength);

    switch (IoControlCode) {
    case IOCTL_BENCHSCOPE_SENSOR_GET_VERSION:
        status = BenchScopeSensorWriteVersion(Request);
        break;
    case IOCTL_BENCHSCOPE_SENSOR_GET_CAPABILITIES:
        status = BenchScopeSensorWriteCapabilities(Request);
        break;
    case IOCTL_BENCHSCOPE_SENSOR_GET_SNAPSHOT:
        status = BenchScopeSensorWriteSnapshot(Request);
        break;
    case IOCTL_BENCHSCOPE_SENSOR_GET_ADVANCED_TELEMETRY:
        status = BenchScopeSensorWriteAdvancedTelemetry(Request);
        break;
    default:
        BenchScopeSensorCompleteBufferedRequest(Request, status, 0);
        break;
    }
}

VOID BenchScopeSensorEvtDriverUnload(_In_ WDFDRIVER Driver)
{
    UNREFERENCED_PARAMETER(Driver);

    if (g_ControlDevice != NULL) {
        WdfObjectDelete(g_ControlDevice);
        g_ControlDevice = NULL;
    }
}

NTSTATUS DriverEntry(
    _In_ PDRIVER_OBJECT DriverObject,
    _In_ PUNICODE_STRING RegistryPath
    )
{
    WDF_DRIVER_CONFIG config;
    WDF_OBJECT_ATTRIBUTES attributes;
    PWDFDEVICE_INIT deviceInit = NULL;
    WDFDRIVER driver = NULL;
    WDFDEVICE device = NULL;
    WDF_IO_QUEUE_CONFIG queueConfig;
    WDFQUEUE queue = NULL;
    NTSTATUS status;

    DECLARE_CONST_UNICODE_STRING(deviceName, BENCHSCOPE_SENSOR_NT_DEVICE_NAME);
    DECLARE_CONST_UNICODE_STRING(symbolicName, BENCHSCOPE_SENSOR_SYMBOLIC_NAME);
    DECLARE_CONST_UNICODE_STRING(sddl, L"D:P(A;;GA;;;SY)(A;;GA;;;BA)");

    WDF_DRIVER_CONFIG_INIT(&config, WDF_NO_EVENT_CALLBACK);
    config.EvtDriverUnload = BenchScopeSensorEvtDriverUnload;

    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    status = WdfDriverCreate(DriverObject, RegistryPath, &attributes, &config, &driver);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    deviceInit = WdfControlDeviceInitAllocate(driver, &sddl);
    if (deviceInit == NULL) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    status = WdfDeviceInitAssignName(deviceInit, &deviceName);
    if (!NT_SUCCESS(status)) {
        WdfDeviceInitFree(deviceInit);
        return status;
    }

    WdfDeviceInitSetDeviceType(deviceInit, FILE_DEVICE_BENCHSCOPE_SENSOR);
    WdfDeviceInitSetCharacteristics(deviceInit, FILE_DEVICE_SECURE_OPEN, TRUE);

    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    status = WdfDeviceCreate(&deviceInit, &attributes, &device);
    if (!NT_SUCCESS(status)) {
        WdfDeviceInitFree(deviceInit);
        return status;
    }

    status = WdfDeviceCreateSymbolicLink(device, &symbolicName);
    if (!NT_SUCCESS(status)) {
        WdfObjectDelete(device);
        return status;
    }

    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig, WdfIoQueueDispatchSequential);
    queueConfig.EvtIoDeviceControl = BenchScopeSensorEvtIoDeviceControl;

    status = WdfIoQueueCreate(device, &queueConfig, WDF_NO_OBJECT_ATTRIBUTES, &queue);
    if (!NT_SUCCESS(status)) {
        WdfObjectDelete(device);
        return status;
    }

    UNREFERENCED_PARAMETER(queue);
    g_ControlDevice = device;
    WdfControlFinishInitializing(device);
    return STATUS_SUCCESS;
}
