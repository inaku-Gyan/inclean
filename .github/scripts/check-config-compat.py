#!/usr/bin/env python3
from __future__ import annotations

import filecmp
import functools
import os
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import time
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:
    print("error: Python 3.11+ is required for tomllib", file=sys.stderr)
    sys.exit(1)


REPO = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
TEMP_ROOT = REPO / "tempdir" / "config-compat-test"
BIN_NAME = "inclean"
BUSINESS_EXIT_CODES = {0, 2, 3}


@functools.total_ordering
@dataclass(frozen=True)
class SemVer:
    major: int
    minor: int
    patch: int
    prerelease: tuple[tuple[int, int | str], ...] = ()

    @classmethod
    def parse(cls, value: str) -> "SemVer":
        match = re.fullmatch(
            r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
            r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
            r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?",
            value,
        )
        if not match:
            raise ValueError(f"invalid SemVer: {value!r}")
        pre = tuple(parse_prerelease_part(part) for part in (match.group(4) or "").split(".") if part)
        return cls(int(match.group(1)), int(match.group(2)), int(match.group(3)), pre)

    def __lt__(self, other: "SemVer") -> bool:
        core = (self.major, self.minor, self.patch)
        other_core = (other.major, other.minor, other.patch)
        if core != other_core:
            return core < other_core
        if not self.prerelease and other.prerelease:
            return False
        if self.prerelease and not other.prerelease:
            return True
        return self.prerelease < other.prerelease


def parse_prerelease_part(part: str) -> tuple[int, int | str]:
    if re.fullmatch(r"0|[1-9]\d*", part):
        return (0, int(part))
    return (1, part)


@dataclass(frozen=True)
class GoldenCase:
    name: str
    input_dir: Path
    expected_dir: Path
    min_cli_version: SemVer


@dataclass(frozen=True)
class PipelineCase:
    name: str
    input_dir: Path
    expected_dir: Path | None
    spec: dict[str, Any]
    min_cli_version: SemVer


def main() -> int:
    current_version = read_cargo_version()
    min_cli_version = read_min_compat_cli_version()
    if current_version == min_cli_version:
        print("MIN_COMPAT_CLI_VERSION equals current version; no older CLI is declared compatible.")
        return 0

    run_dir = make_run_dir()
    print(f"compat workdir: {run_dir.relative_to(REPO)}")

    old_cli = resolve_old_cli(run_dir, min_cli_version)
    print(f"old CLI: {old_cli}")

    min_cli = SemVer.parse(min_cli_version)
    golden_cases = [case for case in discover_golden_cases() if case.min_cli_version <= min_cli]
    pipeline_cases = [case for case in discover_pipeline_cases() if case.min_cli_version <= min_cli]
    total = len(golden_cases) + len(pipeline_cases)
    if total == 0:
        print(
            f"error: no fixture declares min_inclean_version <= {min_cli_version}; "
            "add a compatible golden or pipeline fixture",
            file=sys.stderr,
        )
        return 1

    failures: list[str] = []
    for case in golden_cases:
        try:
            run_golden_case(old_cli, run_dir, case)
            print(f"ok golden {case.name}")
        except Exception as err:
            failures.append(f"golden {case.name}: {err}")

    for case in pipeline_cases:
        try:
            run_pipeline_case(old_cli, run_dir, case)
            print(f"ok pipeline {case.name}")
        except Exception as err:
            failures.append(f"pipeline {case.name}: {err}")

    if failures:
        print("\ncompatibility failures:", file=sys.stderr)
        for failure in failures:
            print(f"\n{failure}", file=sys.stderr)
        print(f"\nleft workdir for inspection: {run_dir}", file=sys.stderr)
        return 1

    print(f"checked {total} compatible fixture(s)")
    return 0


def read_cargo_version() -> str:
    text = (REPO / "Cargo.toml").read_text()
    match = re.search(r'(?m)^version = "([^"]+)"', text)
    if not match:
        raise RuntimeError("failed to read Cargo.toml package version")
    return match.group(1)


def read_min_compat_cli_version() -> str:
    text = (REPO / "src" / "profile.rs").read_text()
    match = re.search(r'MIN_COMPAT_CLI_VERSION:\s*&str\s*=\s*"([^"]+)"', text)
    if not match:
        raise RuntimeError("failed to read MIN_COMPAT_CLI_VERSION")
    return match.group(1)


def make_run_dir() -> Path:
    TEMP_ROOT.mkdir(parents=True, exist_ok=True)
    run_dir = TEMP_ROOT / f"{os.getpid()}-{int(time.time())}"
    counter = 0
    while run_dir.exists():
        counter += 1
        run_dir = TEMP_ROOT / f"{os.getpid()}-{int(time.time())}-{counter}"
    run_dir.mkdir(parents=True)
    return run_dir


def resolve_old_cli(run_dir: Path, min_cli_version: str) -> Path:
    explicit = os.environ.get("INCLEAN_COMPAT_BIN")
    if explicit:
        path = Path(explicit)
        if not path.is_file():
            raise RuntimeError(f"INCLEAN_COMPAT_BIN does not point at a file: {path}")
        return path

    source = os.environ.get("INCLEAN_COMPAT_SOURCE")
    if source is None:
        source = "github-release" if os.environ.get("GITHUB_ACTIONS") == "true" else "local"

    if source == "github-release":
        return install_from_github_release(run_dir, min_cli_version)
    if source in {"local", "cargo"}:
        return install_from_cargo(run_dir, min_cli_version)
    if source == "cargo-binstall":
        return install_with_cargo_binstall(run_dir, min_cli_version)
    if source == "cargo-install":
        return install_with_cargo_install(run_dir, min_cli_version)
    raise RuntimeError(f"unsupported INCLEAN_COMPAT_SOURCE={source!r}")


def install_from_github_release(run_dir: Path, version: str) -> Path:
    target = "x86_64-unknown-linux-gnu"
    if platform.system() != "Linux" or platform.machine() not in {"x86_64", "AMD64"}:
        raise RuntimeError("github-release source only supports Linux x86_64")
    install_dir = run_dir / "install" / "github-release"
    install_dir.mkdir(parents=True)
    archive = install_dir / f"{BIN_NAME}-{target}.tar.gz"
    url = f"https://github.com/inaku-Gyan/inclean/releases/download/v{version}/{BIN_NAME}-{target}.tar.gz"
    print(f"downloading {url}")
    urllib.request.urlretrieve(url, archive)
    with tarfile.open(archive, "r:gz") as tar:
        tar.extractall(install_dir, filter="data")
    bin_path = install_dir / f"{BIN_NAME}-{target}" / BIN_NAME
    if not bin_path.is_file():
        raise RuntimeError(f"release archive did not contain expected binary: {bin_path}")
    bin_path.chmod(bin_path.stat().st_mode | 0o111)
    return bin_path


def install_from_cargo(run_dir: Path, version: str) -> Path:
    if shutil.which("cargo-binstall") is not None:
        return install_with_cargo_binstall(run_dir, version)
    return install_with_cargo_install(run_dir, version)


def install_with_cargo_binstall(run_dir: Path, version: str) -> Path:
    root = run_dir / "install" / "cargo-root"
    run(
        [
            "cargo",
            "binstall",
            BIN_NAME,
            "--version",
            version,
            "--root",
            str(root),
            "--no-confirm",
        ],
        cwd=REPO,
    )
    return cargo_root_binary(root)


def install_with_cargo_install(run_dir: Path, version: str) -> Path:
    root = run_dir / "install" / "cargo-root"
    run(
        [
            "cargo",
            "install",
            BIN_NAME,
            "--version",
            version,
            "--locked",
            "--root",
            str(root),
        ],
        cwd=REPO,
    )
    return cargo_root_binary(root)


def cargo_root_binary(root: Path) -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    bin_path = root / "bin" / f"{BIN_NAME}{suffix}"
    if not bin_path.is_file():
        raise RuntimeError(f"cargo install did not produce expected binary: {bin_path}")
    return bin_path


def discover_golden_cases() -> list[GoldenCase]:
    root = REPO / "tests" / "golden_tests"
    cases = []
    for case_dir in sorted(path for path in root.iterdir() if path.is_dir()):
        input_dir = case_dir / "input"
        expected_dir = case_dir / "expected"
        if input_dir.is_dir() and expected_dir.is_dir():
            cases.append(
                GoldenCase(
                    name=case_dir.name,
                    input_dir=input_dir,
                    expected_dir=expected_dir,
                    min_cli_version=read_fixture_min_cli(input_dir / "inclean.toml"),
                )
            )
    return cases


def discover_pipeline_cases() -> list[PipelineCase]:
    root = REPO / "tests" / "pipeline_cases"
    cases = []
    for case_dir in sorted(path for path in root.iterdir() if path.is_dir()):
        input_dir = case_dir / "input"
        spec_path = case_dir / "case.toml"
        if input_dir.is_dir() and spec_path.is_file():
            spec = load_toml(spec_path)
            expected_dir = case_dir / "expected"
            cases.append(
                PipelineCase(
                    name=case_dir.name,
                    input_dir=input_dir,
                    expected_dir=expected_dir if expected_dir.is_dir() else None,
                    spec=spec,
                    min_cli_version=read_fixture_min_cli(input_dir / "inclean.toml"),
                )
            )
    return cases


def read_fixture_min_cli(config_path: Path) -> SemVer:
    data = load_toml(config_path)
    value = data["project"]["min_inclean_version"]
    return SemVer.parse(value)


def load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as file:
        return tomllib.load(file)


def run_golden_case(old_cli: Path, run_dir: Path, case: GoldenCase) -> None:
    workdir = run_dir / "work" / "golden" / case.name
    copy_tree(case.input_dir, workdir)
    result = run([str(old_cli), "apply", str(workdir)], check=False)
    if result.returncode not in BUSINESS_EXIT_CODES:
        raise RuntimeError(format_command_failure(result))
    compare_trees(workdir, case.expected_dir)


def run_pipeline_case(old_cli: Path, run_dir: Path, case: PipelineCase) -> None:
    workdir = run_dir / "work" / "pipeline" / case.name
    copy_tree(case.input_dir, workdir)
    result = run([str(old_cli), "check", "all", str(workdir)], check=False)
    expected_exit = int(case.spec["exit_code"])
    if result.returncode != expected_exit:
        raise RuntimeError(
            f"exit_code mismatch: expected {expected_exit}, got {result.returncode}\n"
            f"{format_command_output(result)}"
        )
    if case.spec.get("apply", False):
        result = run([str(old_cli), "apply", str(workdir)], check=False)
        if result.returncode != expected_exit:
            raise RuntimeError(
                f"apply exit_code mismatch: expected {expected_exit}, got {result.returncode}\n"
                f"{format_command_output(result)}"
            )
        if case.expected_dir is None:
            raise RuntimeError("case has apply = true but no expected/ directory")
        compare_trees(workdir, case.expected_dir)


def copy_tree(src: Path, dst: Path) -> None:
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst)


def compare_trees(actual_root: Path, expected_root: Path) -> None:
    actual = list_files(actual_root)
    expected = list_files(expected_root)
    if actual != expected:
        actual_set = set(actual)
        expected_set = set(expected)
        missing = sorted(expected_set - actual_set)
        extra = sorted(actual_set - expected_set)
        raise RuntimeError(f"tree mismatch; missing={missing}, extra={extra}")
    for rel in actual:
        actual_path = actual_root / rel
        expected_path = expected_root / rel
        if not filecmp.cmp(actual_path, expected_path, shallow=False):
            raise RuntimeError(f"file mismatch at {rel}")


def list_files(root: Path) -> list[Path]:
    files = []
    for path in root.rglob("*"):
        if path.is_file() and path.name != "inclean.toml":
            files.append(path.relative_to(root))
    return sorted(files)


def run(
    args: list[str],
    cwd: Path | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(args, cwd=cwd, text=True, capture_output=True)
    if check and result.returncode != 0:
        raise RuntimeError(format_command_failure(result))
    return result


def format_command_failure(result: subprocess.CompletedProcess[str]) -> str:
    return f"command failed with exit {result.returncode}: {' '.join(result.args)}\n{format_command_output(result)}"


def format_command_output(result: subprocess.CompletedProcess[str]) -> str:
    return f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as err:
        print(f"error: {err}", file=sys.stderr)
        raise SystemExit(1)
