#!/usr/bin/env pwsh
# Build and boot the real LaplacianOS AArch64 image on QEMU virt.

param(
    [switch]$NoBuild,
    [int]$TimeoutSec = 60,
    [string]$SerialLog = "target\boot-arm64-serial.txt"
)

$ErrorActionPreference = "Stop"
$Workspace = $PSScriptRoot | Split-Path
Set-Location $Workspace

function Fail([string]$Message) {
    Write-Host "[boot-arm64] FAIL: $Message" -ForegroundColor Red
    exit 1
}

function Info([string]$Message) {
    Write-Host "[boot-arm64] $Message" -ForegroundColor Cyan
}

$TargetTriple = "aarch64-unknown-none"
$KernelElf = "target\$TargetTriple\release\laplacianos-kernel"
$KernelImage = "target\laplacianos-arm64.img"
$DataDisk = "target\arm64-data.qcow2"

$Qemu = Get-Command qemu-system-aarch64 -ErrorAction SilentlyContinue
if (-not $Qemu) {
    $Candidate = "C:\Program Files\qemu\qemu-system-aarch64.exe"
    if (Test-Path $Candidate) {
        $Qemu = Get-Item $Candidate
    } else {
        Fail "qemu-system-aarch64 was not found"
    }
}

$QemuImg = Get-Command qemu-img -ErrorAction SilentlyContinue
if (-not $QemuImg) {
    $Candidate = "C:\Program Files\qemu\qemu-img.exe"
    if (Test-Path $Candidate) {
        $QemuImg = Get-Item $Candidate
    } else {
        Fail "qemu-img was not found"
    }
}

if (-not $NoBuild) {
    Info "building AArch64 kernel and protected services (release)"
    & cargo build -p laplacianos-kernel --target $TargetTriple `
        -Z build-std=core,alloc,compiler_builtins `
        -Z build-std-features=compiler-builtins-mem `
        --features freestanding --release
    if ($LASTEXITCODE -ne 0) {
        Fail "AArch64 release build failed"
    }
}

if (-not (Test-Path $KernelElf)) {
    Fail "missing kernel ELF: $KernelElf"
}

$ObjcopyCandidate = Join-Path $env:USERPROFILE ".rustup\toolchains\nightly-x86_64-pc-windows-msvc\lib\rustlib\x86_64-pc-windows-msvc\bin\llvm-objcopy.exe"
if (-not (Test-Path $ObjcopyCandidate)) {
    Fail "nightly llvm-objcopy is missing; install the llvm-tools component"
}
& $ObjcopyCandidate -O binary $KernelElf $KernelImage
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $KernelImage)) {
    Fail "failed to produce raw ARM64 Image"
}

if (-not (Test-Path $DataDisk)) {
    Info "creating persistent virtio block image"
    & $QemuImg.FullName create -f qcow2 $DataDisk 128M | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Fail "failed to create $DataDisk"
    }
}

$SerialPath = [System.IO.Path]::GetFullPath((Join-Path $Workspace $SerialLog))
$ImagePath = [System.IO.Path]::GetFullPath((Join-Path $Workspace $KernelImage))
$DiskPath = [System.IO.Path]::GetFullPath((Join-Path $Workspace $DataDisk))
New-Item -ItemType Directory -Force -Path (Split-Path $SerialPath) | Out-Null
Remove-Item -LiteralPath $SerialPath -Force -ErrorAction SilentlyContinue

$QemuArgs = @(
    "-machine", "virt,gic-version=2",
    "-global", "virtio-mmio.force-legacy=false",
    "-cpu", "cortex-a72",
    "-smp", "2",
    "-m", "1024M",
    "-kernel", $ImagePath,
    "-device", "virtio-rng-device",
    "-device", "virtio-blk-device,drive=data",
    "-drive", "if=none,id=data,file=$DiskPath,format=qcow2",
    "-device", "virtio-net-device,netdev=net0",
    "-netdev", "user,id=net0",
    "-device", "virtio-tablet-device",
    "-device", "virtio-gpu-device",
    "-serial", "file:$SerialPath",
    "-display", "none",
    "-monitor", "none",
    "-no-reboot"
)

Info "booting raw ARM64 Image with modern virtio devices"
$Process = Start-Process -FilePath $Qemu.FullName -ArgumentList $QemuArgs -PassThru -WindowStyle Hidden
$Deadline = (Get-Date).AddSeconds($TimeoutSec)
$Success = $false

try {
    while ((Get-Date) -lt $Deadline) {
        if ($Process.HasExited) {
            break
        }
        Start-Sleep -Milliseconds 200
        if (-not (Test-Path $SerialPath)) {
            continue
        }
        $Log = Get-Content -LiteralPath $SerialPath -Raw -ErrorAction SilentlyContinue
        if ($Log -match "\[aarch64\] sync exception|FATAL:|PANIC|control queue timeout") {
            break
        }
        if ($Log -match "\[desktop\] compositor handoff complete" -and
            $Log -match "\[desktop\] review AI surface autostarted") {
            $Success = $true
            break
        }
    }
}
finally {
    if ($Process -and -not $Process.HasExited) {
        Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
    }
}

if (-not (Test-Path $SerialPath)) {
    Fail "QEMU did not create a serial log"
}

$Log = Get-Content -LiteralPath $SerialPath -Raw
$Tail = Get-Content -LiteralPath $SerialPath | Select-Object -Last 35
if (-not $Success) {
    $Tail | ForEach-Object { Write-Host $_ }
    if ($Log -match "\[aarch64\] sync exception") { Fail "AArch64 synchronous exception" }
    if ($Log -match "FATAL:|PANIC") { Fail "kernel fatal path observed" }
    if ($Log -match "control queue timeout") { Fail "virtio-gpu control queue timed out" }
    Fail "timed out before protected desktop handoff"
}

Info "protected service fabric, compositor handoff, and review surface verified"
$Tail | ForEach-Object { Write-Host $_ }
exit 0
