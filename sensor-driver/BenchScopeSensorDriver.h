#pragma once

#include <ntddk.h>
#include <wdf.h>

#include "include/BenchScopeSensorIoctl.h"

#define BENCHSCOPE_SENSOR_NT_DEVICE_NAME L"\\Device\\BenchScopeSensor"
#define BENCHSCOPE_SENSOR_SYMBOLIC_NAME L"\\DosDevices\\BenchScopeSensor"

DRIVER_INITIALIZE DriverEntry;

EVT_WDF_DRIVER_UNLOAD BenchScopeSensorEvtDriverUnload;
EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL BenchScopeSensorEvtIoDeviceControl;

