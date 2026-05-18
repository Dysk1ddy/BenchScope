# BenchScope Attestation Signing Plan

## Goal

Get the BenchScope sensor driver accepted through Microsoft attestation signing so it can load on supported Windows client systems with Secure Boot enabled, without Windows test-signing mode and without asking users to weaken platform security.

This plan is for direct BenchScope distribution first. It is not the WHCP / HLK certification path and does not target retail Windows Update publication.

## Current Scope

The first attestation candidate is the existing KMDF control-device package. The source INF lives in `sensor-driver/`, and the signed-package inputs are produced by the Release x64 driver build under `sensor-driver/x64/Release/`:

- `sensor-driver/x64/Release/BenchScopeSensorDriver/BenchScopeSensorDriver.inf`
- `sensor-driver/x64/Release/BenchScopeSensorDriver/BenchScopeSensorDriver.sys`
- `sensor-driver/x64/Release/BenchScopeSensorDriver/benchscopesensordriver.cat`
- `sensor-driver/x64/Release/BenchScopeSensorDriver.pdb`

Current package traits:

- Root-enumerated system-class device: `Root\BenchScopeSensor`
- Control device: `\\.\BenchScopeSensor`
- Read-only IOCTL contract.
- Access restricted to LocalSystem and built-in administrators.
- Intel family 6 CPU package temperature / thermal status / energy reads only.
- Unknown CPUs and unsupported MSRs return unsupported status.
- No arbitrary MSR, port, memory, SMBus, EC, fan-control, voltage-control, or write IOCTLs.

## Microsoft Requirements Snapshot

As of the May 2026 planning pass, Microsoft documentation says:

- Windows 10 and later kernel-mode drivers must be submitted through the Windows Hardware Developer Center Dashboard for Microsoft signing.
- A Hardware Developer Program account with an associated EV code-signing certificate is required for attestation signing.
- The attestation CAB must be signed with the organization's EV certificate before upload.
- A typical attestation CAB includes the driver binary, INF, PDB, and catalog. Microsoft regenerates the catalog during signing.
- Driver files must be inside a CAB subfolder, not at the root of the CAB.
- Attestation-signed drivers targeting retail audiences are not published through Windows Update.
- Attestation signing does not provide HLK compatibility/functionality assurance.

References:

- https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-attestation
- https://learn.microsoft.com/en-gb/windows-hardware/drivers/dashboard/code-signing-reqs
- https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/
- https://learn.microsoft.com/windows-hardware/drivers/install/driver-signing

## Route Decision

Use attestation signing as the first production-signing milestone because BenchScope needs a direct-install sensor driver that loads under Secure Boot before it needs Windows Update distribution.

Do not claim WHQL certification, Windows Logo compatibility, or Windows Update eligibility for this milestone.

Move to WHCP / HLK later if BenchScope needs Windows Update distribution, stronger enterprise credibility, or formal compatibility claims.

## External Prerequisites

These cannot be completed only from the repo:

1. Obtain or confirm an EV code-signing certificate for the publishing organization.
2. Register or confirm access to the Microsoft Windows Hardware Developer Program in Partner Center.
3. Associate the EV certificate with the Hardware Dashboard account.
4. Confirm the organization can complete the required legal agreements.
5. Confirm Partner Center accepts this driver category and target OS set for attestation signing.
6. Install the current Windows ADK / signing tooling on the release-signing machine.
7. Define who is allowed to use the EV certificate and how token/HSM access is audited.

## Repo Workstreams

### 1. Release Candidate Hardening

Before upload, the driver must be treated as security-sensitive release code.

- Freeze the initial IOCTL surface.
- Confirm all IOCTLs are output-only and fixed-size.
- Add or document explicit per-IOCTL access validation.
- Confirm every output buffer is fully zeroed before fields are written.
- Confirm all hardware reads are allowlisted and range-checked.
- Keep unsupported hardware paths non-fatal.
- Run `scripts/Test-SensorDriverSecuritySurface.ps1`.
- Run static analysis and fix all actionable warnings.
- Run Driver Verifier on the release candidate.
- Run multi-hour polling tests through `benchscope_sensor_service`.
- Run sleep/resume and service restart tests.
- Run Secure Boot plus Memory Integrity validation on a clean Windows 11 machine.

### 2. Production Package Script

Use `scripts/New-SensorDriverAttestationPackage.ps1` to create a reproducible attestation staging folder and CAB.

Script responsibilities:

- Build Release x64 driver package.
- Verify expected package files exist.
- Copy files into a non-root CAB subfolder, for example:

```text
artifacts/attestation/BenchScopeSensorDriver/
  BenchScopeSensorDriver.inf
  BenchScopeSensorDriver.sys
  BenchScopeSensorDriver.pdb
  BenchScopeSensorDriver.cat
```

- Generate a DDF file for `makecab`.
- Create `BenchScopeSensorDriver-attestation.cab`.
- Reject UNC input paths.
- Emit SHA-256 hashes for the staged files and CAB.
- Leave EV signing as an explicit manual or guarded release step.

Do not bake EV certificate names, thumbprints, PINs, passwords, or token details into the repo.

### 3. Security Review Checklist

Complete `sensor-driver/SECURITY_REVIEW_CHECKLIST.md` before signing or uploading any CAB.

The checklist covers:

- Scope and user-mode fallback review.
- IOCTL access and buffer handling review.
- Device access control.
- Hardware access limits.
- Static review.
- Runtime validation.
- Secure Boot and HVCI validation.
- Release records and user-facing claims.

### 4. EV Signing Procedure

Use `sensor-driver/ATTESTATION_SUBMISSION_RUNBOOK.md` as the release-only procedure for signing the CAB:

- Use the certificate provider's recommended flow.
- Sign with SHA-256 digest and SHA-256 timestamp.
- Verify the CAB signature before upload.
- Record certificate subject, thumbprint, timestamp URL, command transcript, and CAB hash in a private release record.

The repo can include a placeholder command, but the actual certificate identity belongs in private release documentation.

### 5. Partner Center Submission Procedure

Use `sensor-driver/ATTESTATION_SUBMISSION_RUNBOOK.md` for the manual submission details. The high-level steps are:

1. Sign in to the Partner Center hardware dashboard.
2. Create a new hardware submission for BenchScope Sensor Driver.
3. Upload the EV-signed attestation CAB.
4. Leave test-signing options unchecked.
5. Request the applicable Windows client signatures.
6. Submit and monitor processing.
7. Download the Microsoft-signed package.
8. Archive Partner Center product ID, submission ID, and downloaded package hash.

### 6. Signed Package Validation

After Microsoft signs the driver:

- Verify signatures with `signtool verify /pa /ph /v /d`.
- Confirm the catalog was regenerated and signed by Microsoft.
- Confirm the `.sys` has the expected Microsoft embedded signature behavior.
- Install on a clean Windows 11 machine with Secure Boot enabled.
- Confirm no "Windows can't verify the publisher" prompt.
- Confirm `sc.exe query BenchScopeSensorDriver` reaches `RUNNING`.
- Confirm `benchscope_sensor_probe.exe` opens `\\.\BenchScopeSensor`.
- Confirm BenchScope displays CPU telemetry or clear unsupported status.
- Confirm uninstall removes the service and driver package cleanly.
- Confirm reinstall and upgrade work.

### 7. Installer Integration

Only after validation:

- Split dev/test-signing install scripts from production install scripts.
- Production installer must never enable test-signing or ask users to disable Secure Boot.
- Production installer should install only Microsoft-signed driver packages.
- GUI should show a clear optional-sensors setup path.
- GUI must continue without the driver when install is skipped or unsupported.
- Tooltips should distinguish user-mode telemetry, service telemetry, and driver telemetry.

## Acceptance Gates

### Gate A: Ready To Submit

- Release x64 package builds from a clean checkout.
- Package passes INF/signability checks.
- Security review of IOCTL surface is complete.
- Driver Verifier smoke run is complete.
- Clean-machine test-signed install works on a development machine.
- Attestation CAB contains files in the expected subfolder layout.
- CAB is EV-signed and signature-verified.
- Partner Center account and certificate are ready.

### Gate B: Signed Package Accepted

- Partner Center submission completes successfully.
- Microsoft-signed package is downloaded and archived.
- Microsoft signature validation passes.
- Signed driver installs and loads with Secure Boot enabled.
- No Windows Code Integrity error 577.
- Probe and service can read the device.

### Gate C: Ready For BenchScope Users

- Production installer installs, upgrades, and uninstalls the driver cleanly.
- BenchScope works when the driver is present, absent, stopped, or unsupported.
- User-facing text does not imply WHQL certification.
- Release notes disclose that attestation signing is used and Windows Update distribution is not part of this milestone.
- A rollback path is documented.

## Risks And Mitigations

- **Partner Center policy fit is uncertain.** Confirm attestation eligibility before investing heavily in automation.
- **Security review can reveal redesign work.** Keep the first signed driver narrow and read-only.
- **A signed driver can still be blocked later.** Avoid broad hardware access and vulnerable-driver patterns.
- **EV certificate handling is high-risk.** Keep signing access limited, audited, and outside source control.
- **Attestation is not Windows Update distribution.** Plan WHCP / HLK separately if distribution goals change.
- **Driver changes are expensive after signing.** Batch signing submissions around stable protocol versions.

## Immediate Next Steps

1. Assign an owner for EV certificate / Partner Center setup.
2. Run `scripts/New-SensorDriverAttestationPackage.ps1` and verify the staged CAB contents.
3. Complete `sensor-driver/SECURITY_REVIEW_CHECKLIST.md`.
4. Run Driver Verifier and clean Windows install tests against the current prototype.
5. Decide whether the first attestation submission should include only CPU telemetry or wait for service installer polish.
