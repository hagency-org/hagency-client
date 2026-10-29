"""Measure the real attachment example in eight independent processes.

Build once with --locked --release -p hagency-app --example
attachment_memory_soak --features heap-profile,agent_chat. This driver never
builds, never changes accounts, and preserves raw heap, process and file evidence.
"""

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

MIB = 1024 * 1024
PEAK_DELTA_LIMIT = 8 * MIB
ABSOLUTE_HEAP_LIMIT = 32 * MIB
TEMP_BYTES_LIMIT = 4 * 1024 * MIB


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(64 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def private_files(directory):
    """Observe actual filesystem lengths, independently of transfer counters."""
    entries = []
    try:
        for path in directory.rglob("*"):
            try:
                if path.is_symlink():
                    entries.append({"path": str(path.relative_to(directory)), "error": "unexpected symlink"})
                elif path.is_file():
                    entries.append({"path": str(path.relative_to(directory)), "bytes": path.stat().st_size})
            except OSError as error:
                entries.append({"path": str(path), "error": str(error)})
    except OSError as error:
        entries.append({"error": str(error)})
    return entries


def process_memory(pid):
    """OS observations supplement the exact Rust allocator high-water mark."""
    if sys.platform == "win32":
        from ctypes import wintypes

        class Counters(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD)] + [
                (name, ctypes.c_size_t) for name in (
                    "PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage",
                    "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage", "QuotaNonPagedPoolUsage",
                    "PagefileUsage", "PeakPagefileUsage", "PrivateUsage")]

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
        handle = kernel.OpenProcess(0x1000 | 0x10, False, pid)
        if not handle:
            return {"observation_error": "OpenProcess", "os_error": ctypes.get_last_error()}
        try:
            counters = Counters()
            counters.cb = ctypes.sizeof(counters)
            if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
                return {"observation_error": "GetProcessMemoryInfo", "os_error": ctypes.get_last_error()}
            return {"rss_bytes": counters.WorkingSetSize, "peak_rss_bytes": counters.PeakWorkingSetSize,
                    "private_bytes": counters.PrivateUsage}
        finally:
            kernel.CloseHandle(handle)
    if sys.platform.startswith("linux"):
        try:
            fields = dict(line.split(":", 1) for line in Path(f"/proc/{pid}/status").read_text().splitlines() if ":" in line)
            return {"rss_bytes": int(fields["VmRSS"].split()[0]) * 1024,
                    "peak_rss_bytes": int(fields["VmHWM"].split()[0]) * 1024}
        except (OSError, KeyError, ValueError) as error:
            return {"observation_error": str(error)}
    try:
        value = subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)], text=True).strip()
        return {"rss_bytes": int(value) * 1024}
    except (subprocess.CalledProcessError, ValueError) as error:
        return {"observation_error": str(error)}


def run_case(binary, directory, kind, count, size, timeout):
    directory.mkdir(parents=True)
    result_path = directory / "result.json"
    command = [str(binary), "--kind", kind, "--concurrency", str(count), "--bytes", str(size),
               "--output", str(directory)]
    environment = dict(os.environ, HAGENCY_APP_DATA_DIR=str(directory / "isolated-app-data"))
    started = time.monotonic()
    timed_out = False
    with (directory / "stdout.log").open("wb") as stdout, (directory / "stderr.log").open("wb") as stderr, \
            (directory / "process.jsonl").open("w", encoding="utf-8") as samples:
        process = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=environment)
        peak_rss = 0
        peak_private = 0
        peak_file_bytes = 0
        observed = 0
        while process.poll() is None:
            sample = process_memory(process.pid)
            entries = private_files(directory / "private-cache")
            file_bytes = sum(entry.get("bytes", 0) for entry in entries)
            sample.update(private_files=entries, private_file_bytes=file_bytes)
            peak_file_bytes = max(peak_file_bytes, file_bytes)
            sample.update(elapsed_seconds=time.monotonic() - started, pid=process.pid)
            samples.write(json.dumps(sample) + "\n")
            samples.flush()
            if "rss_bytes" in sample:
                observed += 1
                peak_rss = max(peak_rss, sample["rss_bytes"], sample.get("peak_rss_bytes", 0))
            peak_private = max(peak_private, sample.get("private_bytes", 0))
            if time.monotonic() - started > timeout:
                timed_out = True
                process.kill()
                break
            time.sleep(0.05)
        exit_code = process.wait()
    result = json.loads(result_path.read_text()) if result_path.exists() else {}
    result.update(command=command, exit_code=exit_code, timed_out=timed_out,
                  process_samples=observed, sampled_peak_rss_bytes=peak_rss,
                  sampled_peak_private_bytes=peak_private, sampled_peak_file_bytes=peak_file_bytes,
                  final_private_files=private_files(directory / "private-cache"), evidence=str(directory))
    return result


def validate(results):
    errors = []
    for result in results:
        case = result["evidence"]
        if result["exit_code"] or result["timed_out"]:
            errors.append(f"{case}: child failed or timed out")
            continue
        if not result.get("process_samples"):
            errors.append(f"{case}: no OS memory observation")
        if not result.get("verified") or result.get("completed") != result.get("concurrency"):
            errors.append(f"{case}: real output verification incomplete")
        if result.get("server_peak_active") != result.get("concurrency"):
            errors.append(f"{case}: fixture did not exercise the requested network concurrency")
        if result.get("authenticated_downloads") != result.get("concurrency"):
            errors.append(f"{case}: authenticated endpoint request count differs from the requested transfer")
        if not result.get("cleanup_confirmed"):
            errors.append(f"{case}: private-file cleanup/quota release unconfirmed")
        if result.get("sampled_peak_file_bytes", 0) > TEMP_BYTES_LIMIT:
            errors.append(f"{case}: actual temporary file lengths exceeded 4GiB")
        if result.get("actual_plaintext_bytes") != result.get("bytes_per_file", 0) * result.get("concurrency", 0):
            errors.append(f"{case}: verified plaintext byte count differs from the requested transfer")
        if any(result.get("final_quota", {}).get(key) != 0 for key in ("active", "queued", "stored_bytes", "cleanup_failures")):
            errors.append(f"{case}: original quota custody remains or cleanup is unknown")
        if any(entry.get("bytes", 0) or "error" in entry for entry in result.get("final_private_files", [])):
            errors.append(f"{case}: private-file cleanup disagrees with actual final filesystem evidence")
        if result.get("held_quota", {}).get("stored_bytes", -1) < result.get("actual_plaintext_bytes", 0):
            errors.append(f"{case}: retained verified files are not charged to temporary storage")
        if not isinstance(result.get("heap_peak_bytes"), int):
            errors.append(f"{case}: missing actual allocator peak")
        elif result["heap_peak_bytes"] > ABSOLUTE_HEAP_LIMIT:
            errors.append(f"{case}: allocator peak {result['heap_peak_bytes']} exceeds fixed 32MiB window budget")
    comparisons = []
    for kind in ("plain", "encrypted"):
        for count in (1, 4):
            matching = [row for row in results if row.get("kind") == kind and row.get("concurrency") == count]
            matching.sort(key=lambda row: row["bytes_per_file"])
            if len(matching) != 2 or any("heap_peak_bytes" not in row for row in matching):
                errors.append(f"{kind}/{count}: both independent size runs are required")
                continue
            delta = matching[1]["heap_peak_bytes"] - matching[0]["heap_peak_bytes"]
            comparisons.append({"kind": kind, "concurrency": count, "heap_peak_growth_bytes": delta,
                                "limit_bytes": PEAK_DELTA_LIMIT})
            if delta > PEAK_DELTA_LIMIT:
                errors.append(f"{kind}/{count}: peak grew by {delta} bytes; fixed limit {PEAK_DELTA_LIMIT}")
    return comparisons, errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=600)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parents[1]
    sources = ["crates/hagency-app/examples/attachment_memory_soak.rs",
               "crates/hagency-app/src/lib.rs",
               "crates/hagency-app/src/attachment_transport.rs",
               "crates/hagency-app/src/attachment_transport/files.rs",
               "crates/hagency-app/src/attachment_transport/io_worker.rs",
               "crates/hagency-app/src/media_cache_reader.rs",
               "crates/hagency-app/src/media_transport.rs",
               "crates/hagency-app/src/transfer_budget.rs",
               "tools/test_attachment_memory_soak.py", "Cargo.toml",
               "crates/hagency-app/Cargo.toml", "Cargo.lock"]
    manifest = {"binary": str(binary), "binary_sha256": file_hash(binary),
                "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
                "source_sha256": {name: file_hash(root / name) for name in sources}}
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    results = []
    for kind in ("plain", "encrypted"):
        for count in (1, 4):
            for mib in (32, 256):
                name = f"{kind}-{count}x{mib}MiB"
                print(f"Measuring {name}", flush=True)
                result = run_case(binary, output / name, kind, count, mib * MIB, args.timeout)
                # Retain attempted-case identity even when the child failed before writing JSON.
                result.setdefault("kind", kind)
                result.setdefault("concurrency", count)
                result.setdefault("bytes_per_file", mib * MIB)
                results.append(result)
    comparisons, errors = validate(results)
    if file_hash(binary) != manifest["binary_sha256"]:
        errors.append("the measured executable changed during the eight independent runs")
    for name, digest in manifest["source_sha256"].items():
        if file_hash(root / name) != digest:
            errors.append(f"source changed during measurement: {name}")
    summary = {"results": results, "comparisons": comparisons, "errors": errors,
               "fixed_heap_growth_limit_bytes": PEAK_DELTA_LIMIT,
               "fixed_absolute_heap_limit_bytes": ABSOLUTE_HEAP_LIMIT,
               "rss_scope": "whole process including SDK, server and profiler; sampled independently, not a 64KiB total-RSS claim"}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    for error in errors:
        print(error, file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
