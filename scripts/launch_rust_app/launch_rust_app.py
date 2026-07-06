from __future__ import annotations

import argparse
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


@dataclass(frozen=True)
class LaunchPlan:
    label: str
    executable: Path
    mode: str
    run_args: tuple[str, ...] = ()


@dataclass(frozen=True)
class BuildPlan:
    cwd: Path
    program: str
    args: tuple[str, ...]


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Build and launch a Rust desktop application from its debug/release output."
    )
    parser.add_argument("--label", required=True, help="Human-readable app name")
    parser.add_argument(
        "--project-dir",
        required=True,
        help="Project directory that contains the Rust build output",
    )
    parser.add_argument(
        "--release-relpath",
        required=True,
        help="Relative path from project-dir to the release executable",
    )
    parser.add_argument(
        "--debug-relpath",
        required=True,
        help="Relative path from project-dir to the debug executable",
    )
    parser.add_argument(
        "--mode",
        choices=("auto", "release", "debug"),
        default="auto",
        help="Preferred build to launch",
    )
    parser.add_argument(
        "--build-cwd",
        help="Optional working directory where the build command should run",
    )
    parser.add_argument(
        "--build-program",
        help="Optional program to execute before launch, for example cargo or pnpm",
    )
    parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Argument passed to the build command; repeat for multiple arguments",
    )
    parser.add_argument(
        "--run-arg",
        action="append",
        default=[],
        help="Argument passed to the launched executable; repeat for multiple arguments",
    )
    return parser


def resolve_plan(
    *,
    label: str,
    project_dir: Path,
    release_relpath: str,
    debug_relpath: str,
    mode: str,
    run_args: Sequence[str],
) -> LaunchPlan:
    release_executable = project_dir / release_relpath
    debug_executable = project_dir / debug_relpath
    candidates = {
        "release": release_executable,
        "debug": debug_executable,
    }
    order = ["release", "debug"] if mode == "auto" else [mode]
    for candidate_mode in order:
        executable = candidates[candidate_mode]
        if executable.is_file():
            return LaunchPlan(label=label, executable=executable, mode=candidate_mode, run_args=tuple(run_args))
    searched = "\n".join(
        f"- {candidate_mode}: {candidates[candidate_mode]}"
        for candidate_mode in ("release", "debug")
    )
    raise FileNotFoundError(
        f"Aucun binaire trouve pour {label}.\n"
        f"Projet: {project_dir}\n"
        f"Recherche:\n{searched}"
    )


def resolve_build_plan(
    *,
    build_cwd: str | None,
    build_program: str | None,
    build_args: Sequence[str],
) -> BuildPlan | None:
    if not build_program:
        return None
    if not build_cwd:
        raise ValueError("--build-cwd est requis quand --build-program est fourni")
    return BuildPlan(cwd=Path(build_cwd), program=build_program, args=tuple(build_args))


def run_build(build_plan: BuildPlan, label: str) -> None:
    if not build_plan.cwd.is_dir():
        raise FileNotFoundError(f"répertoire de build introuvable: {build_plan.cwd}")
    command = [build_plan.program, *build_plan.args]
    print(f"build: {label}")
    print(f"build cwd: {build_plan.cwd}")
    print("build command: " + " ".join(command))
    process = subprocess.Popen(
        command,
        cwd=str(build_plan.cwd),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    assert process.stdout is not None
    for line in process.stdout:
        print(line.rstrip("\r\n"))
    code = process.wait()
    if code != 0:
        raise RuntimeError(f"build échoué avec le code {code}")


def launch(plan: LaunchPlan) -> subprocess.Popen[bytes]:
    return subprocess.Popen(
        [str(plan.executable), *plan.run_args],
        cwd=str(plan.executable.parent),
    )


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    project_dir = Path(args.project_dir)
    if not project_dir.is_dir():
        print(f"ERREUR: projet introuvable: {project_dir}", file=sys.stderr)
        return 1

    try:
        build_plan = resolve_build_plan(
            build_cwd=args.build_cwd,
            build_program=args.build_program,
            build_args=args.build_arg,
        )
        if build_plan is not None:
            run_build(build_plan, args.label)
        plan = resolve_plan(
            label=args.label,
            project_dir=project_dir,
            release_relpath=args.release_relpath,
            debug_relpath=args.debug_relpath,
            mode=args.mode,
            run_args=args.run_arg,
        )
    except (FileNotFoundError, RuntimeError, ValueError) as error:
        print(f"ERREUR: {error}", file=sys.stderr)
        return 1

    process = launch(plan)
    print(f"application: {plan.label}")
    print(f"mode: {plan.mode}")
    print(f"executable: {plan.executable}")
    print(f"pid: {process.pid}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
