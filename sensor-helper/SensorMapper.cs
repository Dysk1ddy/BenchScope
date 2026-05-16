using LibreHardwareMonitor.Hardware;

namespace BenchScope.SensorHelper;

public static class SensorMapper
{
    public static SensorSnapshot Read(IComputer computer, bool isElevated)
    {
        var allSensors = EnumerateHardware(computer.Hardware)
            .SelectMany(hardware => hardware.Sensors.Select(sensor => new HardwareSensor(hardware, sensor)))
            .Where(item => item.Sensor.Value.HasValue)
            .ToList();
        var temperatureSensors = allSensors
            .Where(item => item.Sensor.SensorType == SensorType.Temperature)
            .ToList();
        var loadSensors = allSensors
            .Where(item => item.Sensor.SensorType == SensorType.Load)
            .ToList();

        var diagnostics = new List<string>();
        if (!isElevated)
        {
            diagnostics.Add("Helper is not elevated; some hardware sensors may be hidden.");
        }

        return new SensorSnapshot(
            DateTimeOffset.UtcNow,
            isElevated,
            PickCpuSensor(temperatureSensors, loadSensors),
            PickGpuSensor(temperatureSensors, loadSensors),
            PickDriveSensor(temperatureSensors, loadSensors),
            PickMemorySensor(temperatureSensors, loadSensors),
            diagnostics.ToArray());
    }

    private static SensorReading PickCpuSensor(
        IReadOnlyCollection<HardwareSensor> temperatureSensors,
        IReadOnlyCollection<HardwareSensor> loadSensors)
    {
        var cpuSensors = temperatureSensors
            .Where(item =>
                item.Hardware.HardwareType == HardwareType.Cpu &&
                !IsIntegratedGpuTemperatureCandidate(item))
            .ToList();
        var utilization = PickUtilization(
            loadSensors,
            item => item.Hardware.HardwareType == HardwareType.Cpu,
            [
                "cpu total",
                "total"
            ]);

        if (cpuSensors.Count == 0)
        {
            return SensorReading
                .Unsupported("CPU", "No CPU temperature sensor found")
                .WithUtilization(utilization);
        }

        return PickPreferred(
            cpuSensors,
            "CPU",
            [
                "package",
                "tctl",
                "tdie",
                "core max",
                "core"
            ])
            .WithUtilization(utilization);
    }

    private static SensorReading PickGpuSensor(
        IReadOnlyCollection<HardwareSensor> temperatureSensors,
        IReadOnlyCollection<HardwareSensor> loadSensors)
    {
        var gpuSensors = temperatureSensors
            .Where(item =>
                item.Hardware.HardwareType == HardwareType.GpuAmd ||
                item.Hardware.HardwareType == HardwareType.GpuIntel ||
                item.Hardware.HardwareType == HardwareType.GpuNvidia)
            .ToList();
        var utilization = PickUtilization(
            loadSensors,
            IsGpuLoadCandidate,
            [
                "gpu core",
                "gpu total",
                "3d",
                "graphics",
                "gfx",
                "d3d",
                "video engine",
                "compute"
            ]);

        if (gpuSensors.Count > 0)
        {
            return PickPreferred(
                gpuSensors,
                "GPU",
                [
                    "hot spot",
                    "hotspot",
                    "core",
                    "gpu"
                ])
                .WithUtilization(utilization);
        }

        var integratedGpuSensors = temperatureSensors
            .Where(IsIntegratedGpuTemperatureCandidate)
            .ToList();

        if (integratedGpuSensors.Count > 0)
        {
            return PickPreferred(
                integratedGpuSensors,
                "iGPU",
                [
                    "gt cores",
                    "gt core",
                    "graphics",
                    "gfx",
                    "igpu",
                    "gpu",
                    "apu"
                ])
                .WithUtilization(utilization);
        }

        var cpuPackage = PickCpuPackageSensor(temperatureSensors);
        if (cpuPackage is not null && HasIntegratedGpu(loadSensors))
        {
            return SensorReading
                .Ok("iGPU shared CPU package", cpuPackage.Sensor.Value!.Value, utilization);
        }

        return SensorReading
            .Unsupported("GPU", "No GPU or iGPU temperature sensor found")
            .WithUtilization(utilization);
    }

    private static SensorReading PickDriveSensor(
        IReadOnlyCollection<HardwareSensor> temperatureSensors,
        IReadOnlyCollection<HardwareSensor> loadSensors)
    {
        var driveSensors = temperatureSensors
            .Where(item => item.Hardware.HardwareType == HardwareType.Storage)
            .ToList();
        var utilization = PickUtilization(
            loadSensors,
            item =>
                item.Hardware.HardwareType == HardwareType.Storage &&
                !item.Sensor.Name.Contains("used space", StringComparison.OrdinalIgnoreCase),
            [
                "activity",
                "busy",
                "usage",
                "load"
            ]);

        if (driveSensors.Count == 0)
        {
            return SensorReading
                .Unsupported("SSD", "No drive temperature sensor found")
                .WithUtilization(utilization);
        }

        return PickPreferred(
            driveSensors,
            "SSD",
            [
                "temperature",
                "composite",
                "drive"
            ])
            .WithUtilization(utilization);
    }

    private static SensorReading PickMemorySensor(
        IReadOnlyCollection<HardwareSensor> temperatureSensors,
        IReadOnlyCollection<HardwareSensor> loadSensors)
    {
        var memorySensors = temperatureSensors
            .Where(item => item.Hardware.HardwareType == HardwareType.Memory)
            .ToList();
        var utilization = PickUtilization(
            loadSensors,
            item => item.Hardware.HardwareType == HardwareType.Memory,
            [
                "memory",
                "used",
                "load"
            ]);

        if (memorySensors.Count == 0)
        {
            return SensorReading
                .Unsupported("RAM", "No RAM temperature sensor found")
                .WithUtilization(utilization);
        }

        return PickPreferred(
            memorySensors,
            "RAM",
            [
                "temperature",
                "dimm",
                "memory"
            ])
            .WithUtilization(utilization);
    }

    private static SensorReading PickPreferred(
        IReadOnlyCollection<HardwareSensor> sensors,
        string fallbackLabel,
        string[] preferredNameParts)
    {
        foreach (var preferred in preferredNameParts)
        {
            var match = sensors
                .Where(item => item.Sensor.Name.Contains(preferred, StringComparison.OrdinalIgnoreCase))
                .OrderByDescending(item => item.Sensor.Value ?? float.MinValue)
                .FirstOrDefault();

            if (match is not null)
            {
                return ToReading(match, fallbackLabel);
            }
        }

        var hottest = sensors
            .OrderByDescending(item => item.Sensor.Value ?? float.MinValue)
            .First();
        return ToReading(hottest, fallbackLabel);
    }

    private static SensorReading ToReading(HardwareSensor item, string fallbackLabel)
    {
        var label = !string.IsNullOrWhiteSpace(item.Sensor.Name)
            ? item.Sensor.Name
            : fallbackLabel;
        var hardwareName = item.Hardware.Name;
        if (!string.IsNullOrWhiteSpace(hardwareName) &&
            !label.Contains(hardwareName, StringComparison.OrdinalIgnoreCase))
        {
            label = $"{hardwareName} {label}";
        }

        return SensorReading.Ok(label, item.Sensor.Value!.Value);
    }

    private static float? PickUtilization(
        IReadOnlyCollection<HardwareSensor> sensors,
        Func<HardwareSensor, bool> predicate,
        string[] preferredNameParts)
    {
        var candidates = sensors
            .Where(predicate)
            .ToList();

        if (candidates.Count == 0)
        {
            return null;
        }

        foreach (var preferred in preferredNameParts)
        {
            var match = candidates
                .Where(item => item.Sensor.Name.Contains(preferred, StringComparison.OrdinalIgnoreCase))
                .OrderByDescending(item => item.Sensor.Value ?? float.MinValue)
                .FirstOrDefault();

            if (match is not null)
            {
                return ClampPercent(match.Sensor.Value!.Value);
            }
        }

        return ClampPercent(candidates.Max(item => item.Sensor.Value!.Value));
    }

    private static HardwareSensor? PickCpuPackageSensor(IReadOnlyCollection<HardwareSensor> sensors)
    {
        var cpuSensors = sensors
            .Where(item => item.Hardware.HardwareType == HardwareType.Cpu)
            .ToList();

        if (cpuSensors.Count == 0)
        {
            return null;
        }

        foreach (var preferred in new[] { "package", "tctl", "tdie", "core max" })
        {
            var match = cpuSensors
                .Where(item => item.Sensor.Name.Contains(preferred, StringComparison.OrdinalIgnoreCase))
                .OrderByDescending(item => item.Sensor.Value ?? float.MinValue)
                .FirstOrDefault();

            if (match is not null)
            {
                return match;
            }
        }

        return cpuSensors
            .OrderByDescending(item => item.Sensor.Value ?? float.MinValue)
            .FirstOrDefault();
    }

    private static bool HasIntegratedGpu(IReadOnlyCollection<HardwareSensor> sensors)
    {
        return sensors.Any(item =>
            item.Hardware.HardwareType == HardwareType.GpuIntel ||
            item.Hardware.HardwareType == HardwareType.GpuAmd ||
            ContainsAny(
                item.Hardware.Name ?? string.Empty,
                [
                    "iris xe",
                    "intel xe",
                    "uhd graphics",
                    "radeon graphics",
                    "vega",
                    "apu"
                ]));
    }

    private static bool IsGpuLoadCandidate(HardwareSensor item)
    {
        if (item.Hardware.HardwareType == HardwareType.GpuAmd ||
            item.Hardware.HardwareType == HardwareType.GpuIntel ||
            item.Hardware.HardwareType == HardwareType.GpuNvidia)
        {
            return true;
        }

        if (item.Hardware.HardwareType != HardwareType.Cpu)
        {
            return false;
        }

        return ContainsAny(
            item.Sensor.Name ?? string.Empty,
            [
                "gt",
                "graphics",
                "gfx",
                "igpu",
                "gpu"
            ]);
    }

    private static float ClampPercent(float value)
    {
        return MathF.Round(Math.Clamp(value, 0.0f, 100.0f), 1);
    }

    private static bool IsIntegratedGpuTemperatureCandidate(HardwareSensor item)
    {
        var sensorName = item.Sensor.Name ?? string.Empty;
        var hardwareName = item.Hardware.Name ?? string.Empty;

        if (item.Hardware.HardwareType == HardwareType.GpuIntel ||
            item.Hardware.HardwareType == HardwareType.GpuAmd)
        {
            return true;
        }

        if (item.Hardware.HardwareType != HardwareType.Cpu)
        {
            return false;
        }

        return ContainsAny(
            sensorName,
            [
                "gt cores",
                "gt core",
                "graphics",
                "gfx",
                "igpu",
                "gpu"
            ])
            || (ContainsAny(
                    hardwareName,
                    [
                        "iris xe",
                        "intel xe",
                        "uhd graphics",
                        "radeon graphics",
                        "vega",
                        "apu"
                    ])
                && ContainsAny(
                    sensorName,
                    [
                        "temperature",
                        "core",
                        "graphics",
                        "gfx",
                        "gpu"
                    ]));
    }

    private static bool ContainsAny(string value, string[] needles)
    {
        return needles.Any(needle => value.Contains(needle, StringComparison.OrdinalIgnoreCase));
    }

    private static IEnumerable<IHardware> EnumerateHardware(IEnumerable<IHardware> hardware)
    {
        foreach (var item in hardware)
        {
            yield return item;

            foreach (var subHardware in EnumerateHardware(item.SubHardware))
            {
                yield return subHardware;
            }
        }
    }

    private sealed record HardwareSensor(IHardware Hardware, ISensor Sensor);
}
