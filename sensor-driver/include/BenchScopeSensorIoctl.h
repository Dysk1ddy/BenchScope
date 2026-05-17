#pragma once

#ifndef CTL_CODE
#include <winioctl.h>
#endif

#define BENCHSCOPE_SENSOR_DEVICE_NAME "\\\\.\\BenchScopeSensor"

#define BENCHSCOPE_SENSOR_VERSION_MAJOR 0u
#define BENCHSCOPE_SENSOR_VERSION_MINOR 2u
#define BENCHSCOPE_SENSOR_VERSION_PATCH 0u

#define BENCHSCOPE_SENSOR_PROTOCOL_VERSION 1u

#define FILE_DEVICE_BENCHSCOPE_SENSOR 0x8337u

#define IOCTL_BENCHSCOPE_SENSOR_GET_VERSION                                             \
    CTL_CODE(FILE_DEVICE_BENCHSCOPE_SENSOR, 0x801u, METHOD_BUFFERED, FILE_READ_DATA)

#define IOCTL_BENCHSCOPE_SENSOR_GET_CAPABILITIES                                        \
    CTL_CODE(FILE_DEVICE_BENCHSCOPE_SENSOR, 0x802u, METHOD_BUFFERED, FILE_READ_DATA)

#define IOCTL_BENCHSCOPE_SENSOR_GET_SNAPSHOT                                            \
    CTL_CODE(FILE_DEVICE_BENCHSCOPE_SENSOR, 0x803u, METHOD_BUFFERED, FILE_READ_DATA)

#define IOCTL_BENCHSCOPE_SENSOR_GET_ADVANCED_TELEMETRY                                  \
    CTL_CODE(FILE_DEVICE_BENCHSCOPE_SENSOR, 0x804u, METHOD_BUFFERED, FILE_READ_DATA)

typedef enum BENCHSCOPE_SENSOR_KIND {
    BenchScopeSensorKindCpu = 1,
    BenchScopeSensorKindGpu = 2,
    BenchScopeSensorKindDrive = 3,
    BenchScopeSensorKindMemory = 4,
    BenchScopeSensorKindMotherboard = 5,
    BenchScopeSensorKindFan = 6,
    BenchScopeSensorKindVoltage = 7,
    BenchScopeSensorKindStorageHealth = 8,
} BENCHSCOPE_SENSOR_KIND;

typedef enum BENCHSCOPE_SENSOR_STATUS {
    BenchScopeSensorStatusOk = 0,
    BenchScopeSensorStatusUnsupported = 1,
    BenchScopeSensorStatusPermissionDenied = 2,
    BenchScopeSensorStatusUnavailable = 3,
    BenchScopeSensorStatusError = 4,
} BENCHSCOPE_SENSOR_STATUS;

#define BENCHSCOPE_SENSOR_READING_HAS_TEMPERATURE 0x00000001u
#define BENCHSCOPE_SENSOR_READING_HAS_UTILIZATION 0x00000002u

#define BENCHSCOPE_SENSOR_PROVIDER_CPU_TELEMETRY 0x00000001u
#define BENCHSCOPE_SENSOR_PROVIDER_MOTHERBOARD_SUPER_IO 0x00000002u
#define BENCHSCOPE_SENSOR_PROVIDER_STORAGE_HEALTH_USER_MODE 0x00000004u

#define BENCHSCOPE_SENSOR_ADVANCED_HAS_TEMPERATURE 0x00000001u
#define BENCHSCOPE_SENSOR_ADVANCED_HAS_THERMAL_LIMIT 0x00000002u
#define BENCHSCOPE_SENSOR_ADVANCED_THERMAL_THROTTLED 0x00000004u
#define BENCHSCOPE_SENSOR_ADVANCED_HAS_ENERGY 0x00000008u
#define BENCHSCOPE_SENSOR_ADVANCED_HAS_POWER 0x00000010u
#define BENCHSCOPE_SENSOR_ADVANCED_HAS_FAN_RPM 0x00000020u
#define BENCHSCOPE_SENSOR_ADVANCED_HAS_VOLTAGE 0x00000040u
#define BENCHSCOPE_SENSOR_ADVANCED_USER_MODE_PROVIDER 0x80000000u

typedef struct BENCHSCOPE_SENSOR_VERSION {
    unsigned int protocolVersion;
    unsigned int driverMajor;
    unsigned int driverMinor;
    unsigned int driverPatch;
} BENCHSCOPE_SENSOR_VERSION;

typedef struct BENCHSCOPE_SENSOR_CAPABILITIES {
    unsigned int protocolVersion;
    unsigned int supportsCpuTemperature;
    unsigned int supportsGpuTemperature;
    unsigned int supportsDriveTemperature;
    unsigned int supportsUtilization;
    unsigned int reserved[8];
} BENCHSCOPE_SENSOR_CAPABILITIES;

typedef struct BENCHSCOPE_SENSOR_READING {
    unsigned int kind;
    unsigned int status;
    unsigned int flags;
    int temperatureMilliC;
    int utilizationMilliPercent;
    wchar_t label[64];
    wchar_t provider[64];
} BENCHSCOPE_SENSOR_READING;

typedef struct BENCHSCOPE_SENSOR_SNAPSHOT {
    unsigned int protocolVersion;
    unsigned int readingCount;
    unsigned long long sequence;
    long long timestampQpc;
    BENCHSCOPE_SENSOR_READING readings[4];
} BENCHSCOPE_SENSOR_SNAPSHOT;

typedef struct BENCHSCOPE_SENSOR_ADVANCED_READING {
    unsigned int kind;
    unsigned int status;
    unsigned int flags;
    int temperatureMilliC;
    int thermalLimitMilliC;
    int utilizationMilliPercent;
    int powerMilliWatts;
    unsigned long long energyMilliJoules;
    unsigned int fanRpm;
    int voltageMilliV;
    wchar_t label[64];
    wchar_t provider[64];
    wchar_t detail[128];
} BENCHSCOPE_SENSOR_ADVANCED_READING;

typedef struct BENCHSCOPE_SENSOR_ADVANCED_TELEMETRY {
    unsigned int protocolVersion;
    unsigned int providerMask;
    unsigned int readingCount;
    unsigned int reserved;
    unsigned long long sequence;
    long long timestampQpc;
    BENCHSCOPE_SENSOR_ADVANCED_READING readings[8];
} BENCHSCOPE_SENSOR_ADVANCED_TELEMETRY;
