namespace BenchScope.SensorHelper;

public sealed record SensorSnapshot(
    DateTimeOffset TimestampUtc,
    bool IsElevated,
    SensorReading? Cpu,
    SensorReading? Gpu,
    SensorReading? Drive,
    string[] Diagnostics)
{
    public static SensorSnapshot Error(bool isElevated, string message)
    {
        var diagnostics = new[] { message };
        return new SensorSnapshot(
            DateTimeOffset.UtcNow,
            isElevated,
            SensorReading.Error("CPU", message),
            SensorReading.Error("GPU", message),
            SensorReading.Error("SSD", message),
            diagnostics);
    }
}

public sealed record SensorReading(
    string Label,
    float? TemperatureC,
    string Provider,
    string Status,
    float? UtilizationPercent = null,
    string? Message = null)
{
    public static SensorReading Ok(string label, float temperatureC, float? utilizationPercent = null)
    {
        return new SensorReading(label, temperatureC, "LibreHardwareMonitor", "ok", utilizationPercent);
    }

    public static SensorReading Unsupported(string label, string message)
    {
        return new SensorReading(label, null, "LibreHardwareMonitor", "unsupported", null, message);
    }

    public static SensorReading Error(string label, string message)
    {
        return new SensorReading(label, null, "LibreHardwareMonitor", "error", null, message);
    }

    public SensorReading WithUtilization(float? utilizationPercent)
    {
        return this with { UtilizationPercent = utilizationPercent };
    }
}
