using System.Diagnostics;
using System.Security.Principal;
using System.Text.Json;
using System.Text.Json.Serialization;
using BenchScope.SensorHelper;
using LibreHardwareMonitor.Hardware;

var once = args.Any(arg => string.Equals(arg, "--once", StringComparison.OrdinalIgnoreCase));
var outFile = ValueAfter(args, "--out-file");
var parentPidText = ValueAfter(args, "--parent-pid");
var parentPid = int.TryParse(parentPidText, out var parsedParentPid)
    ? parsedParentPid
    : (int?)null;
var options = new JsonSerializerOptions
{
    DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
    PropertyNamingPolicy = JsonNamingPolicy.CamelCase
};

var computer = new Computer
{
    IsControllerEnabled = true,
    IsCpuEnabled = true,
    IsGpuEnabled = true,
    IsMemoryEnabled = true,
    IsMotherboardEnabled = true,
    IsStorageEnabled = true
};

try
{
    computer.Open();
}
catch (Exception ex)
{
    WriteSnapshot(SensorSnapshot.Error(IsElevated(), $"LibreHardwareMonitor failed to open: {ex.Message}"));
    return 2;
}

var visitor = new UpdateVisitor();

do
{
    if (parentPid is int pid && !IsProcessAlive(pid))
    {
        break;
    }

    try
    {
        computer.Accept(visitor);
        WriteSnapshot(SensorMapper.Read(computer, IsElevated()));
    }
    catch (Exception ex)
    {
        WriteSnapshot(SensorSnapshot.Error(IsElevated(), ex.Message));
    }

    if (once)
    {
        break;
    }

    Thread.Sleep(TimeSpan.FromSeconds(1));
} while (true);

computer.Close();
return 0;

void WriteSnapshot(SensorSnapshot snapshot)
{
    var json = JsonSerializer.Serialize(snapshot, options);
    if (!string.IsNullOrWhiteSpace(outFile))
    {
        WriteSnapshotFile(outFile, json);
        return;
    }

    Console.WriteLine(json);
    Console.Out.Flush();
}

static string? ValueAfter(string[] args, string name)
{
    for (var index = 0; index < args.Length - 1; index++)
    {
        if (string.Equals(args[index], name, StringComparison.OrdinalIgnoreCase))
        {
            return args[index + 1];
        }
    }

    return null;
}

static void WriteSnapshotFile(string path, string json)
{
    var directory = Path.GetDirectoryName(path);
    if (!string.IsNullOrWhiteSpace(directory))
    {
        Directory.CreateDirectory(directory);
    }

    var tempPath = path + ".tmp";
    File.WriteAllText(tempPath, json + Environment.NewLine);
    File.Move(tempPath, path, overwrite: true);
}

static bool IsProcessAlive(int processId)
{
    try
    {
        using var process = Process.GetProcessById(processId);
        return !process.HasExited;
    }
    catch
    {
        return false;
    }
}

static bool IsElevated()
{
    if (!OperatingSystem.IsWindows())
    {
        return false;
    }

    using var identity = WindowsIdentity.GetCurrent();
    var principal = new WindowsPrincipal(identity);
    return principal.IsInRole(WindowsBuiltInRole.Administrator);
}

sealed class UpdateVisitor : IVisitor
{
    public void VisitComputer(IComputer computer)
    {
        computer.Traverse(this);
    }

    public void VisitHardware(IHardware hardware)
    {
        hardware.Update();
        foreach (var subHardware in hardware.SubHardware)
        {
            subHardware.Accept(this);
        }
    }

    public void VisitSensor(ISensor sensor)
    {
    }

    public void VisitParameter(IParameter parameter)
    {
    }
}
