# BenchScope Sensor Driver Attestation Submission Runbook

This runbook starts after the Release x64 driver package builds locally and before the Microsoft Hardware Dashboard upload.

Do not store EV certificate PINs, passwords, token details, or private submission records in this repository.

## 1. Build And Stage The CAB

From the repository root:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\New-SensorDriverAttestationPackage.ps1
```

Expected output:

- `artifacts\attestation\BenchScopeSensorDriver-attestation.cab`
- `artifacts\attestation\BenchScopeSensorDriver-attestation.hashes.txt`
- `artifacts\attestation\BenchScopeSensorDriver-attestation.inf`
- `artifacts\attestation\BenchScopeSensorDriver-attestation.rpt`
- `artifacts\attestation\stage\BenchScopeSensorDriver\...`

Confirm the CAB manifest lists files under `BenchScopeSensorDriver\`:

```powershell
Get-Content .\artifacts\attestation\BenchScopeSensorDriver-attestation.inf
```

## 2. Complete Security Review

Complete:

```text
sensor-driver\SECURITY_REVIEW_CHECKLIST.md
```

Keep completed checklist evidence in private release records if it includes machine names, certificate details, Partner Center IDs, or logs that should not be public.

## 3. Sign The CAB With The EV Certificate

Use the certificate provider's supported signing flow. A typical `signtool` command shape is:

```powershell
signtool sign /fd SHA256 /td SHA256 /tr <timestamp-url> /a .\artifacts\attestation\BenchScopeSensorDriver-attestation.cab
```

If `signtool` is not on `PATH`, use the x64 copy from the Windows Kits `bin\<version>\x64` directory.

If multiple certificates are available, use the provider's documented selector such as `/sha1 <thumbprint>` or certificate-store parameters. Do not commit those values.

Verify the signed CAB:

```powershell
signtool verify /pa /v .\artifacts\attestation\BenchScopeSensorDriver-attestation.cab
```

Record in private release notes:

- Source commit.
- CAB SHA-256.
- EV certificate subject and thumbprint.
- Timestamp URL.
- Signing command transcript.

## 4. Upload To Microsoft Hardware Dashboard

In Partner Center / Hardware Dashboard:

1. Create a new driver signing submission.
2. Choose the attestation signing path.
3. Upload the EV-signed CAB.
4. Select the intended Windows client target versions.
5. Do not select test-signing options for production release.
6. Submit and monitor processing.
7. Download the Microsoft-signed result package.

Record in private release notes:

- Product name.
- Product ID.
- Submission ID.
- Downloaded package SHA-256.
- Processing status and any dashboard warnings.

## 5. Verify Microsoft-Signed Output

Run signature verification on the returned package contents:

```powershell
signtool verify /pa /ph /v /d <path-to-signed-cat-or-sys>
signtool verify /kp /v <path-to-signed-cat-or-sys>
```

Expected result:

- Signature verification succeeds.
- The catalog is Microsoft-signed.
- No Code Integrity signature errors appear during install or driver start.

## 6. Clean-Machine Validation

On a clean Windows 11 machine:

1. Enable Secure Boot.
2. Enable Memory Integrity / HVCI.
3. Confirm Windows test-signing is off.
4. Install the Microsoft-signed driver package.
5. Start `BenchScopeSensorDriver`.
6. Run `benchscope_sensor_probe.exe`.
7. Run `benchscope_sensor_service.exe --stream --interval-ms 1000`.
8. Launch BenchScope and confirm sensors show driver telemetry or clear unsupported status.
9. Run uninstall and reinstall.
10. Check Event Viewer for Code Integrity, WHEA, driver, or service errors.

## 7. Release Guardrails

- Do not describe attestation signing as WHQL, HLK, Windows Certification, or Windows Update distribution.
- Do not ship the dev test-signing scripts as a normal user setup path.
- Do not ask users to disable Secure Boot, Memory Integrity, Defender, or vulnerable-driver block rules.
- Keep the driver optional; BenchScope benchmarks must continue when the driver is absent.
