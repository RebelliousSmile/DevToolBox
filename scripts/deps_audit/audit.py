#!/usr/bin/env python3
"""Propose obsolete code libraries and tools for cleanup.

This is a static, offline audit: it never touches the network and never edits
files. It reports dependencies that are declared but no longer referenced by
the source code, so a human can decide whether to remove them.

Two ecosystems are inspected:

* Rust crates declared in ``Cargo.toml`` versus identifiers used in ``*.rs``.
* Python packages declared in each ``scripts/*/requirements.txt`` versus the
  modules imported by the sibling ``*.py`` files.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # Python < 3.11: minimal fallback parser is used.
    tomllib = None  # type: ignore[assignment]


# Packages whose import name differs from the distribution name on PyPI.
PYTHON_IMPORT_ALIASES = {
    "pyyaml": "yaml",
    "pillow": "PIL",
    "beautifulsoup4": "bs4",
    "python-dateutil": "dateutil",
    "msgpack-python": "msgpack",
}

_DEP_TABLE_SUFFIX = "dependencies"
_REQUIREMENT_NAME = re.compile(r"^([A-Za-z0-9][A-Za-z0-9._-]*)")
_IMPORT_LINE = re.compile(r"^\s*(?:import|from)\s+([A-Za-z0-9_.]+)")


@dataclass(frozen=True)
class Finding:
    ecosystem: str
    name: str
    location: str
    reason: str


def find_project_root(start: Path) -> Path:
    """Return the nearest ancestor (inclusive) that contains ``Cargo.toml``."""
    for candidate in (start, *start.parents):
        if (candidate / "Cargo.toml").is_file():
            return candidate
    raise FileNotFoundError(f"aucun Cargo.toml trouvé au-dessus de {start}")


def _dependency_names_from_toml(data: dict) -> set[str]:
    names: set[str] = set()

    def collect(table: object) -> None:
        if isinstance(table, dict):
            names.update(table.keys())

    collect(data.get("dependencies"))
    collect(data.get("dev-dependencies"))
    collect(data.get("build-dependencies"))
    # target."cfg(...)".dependencies and friends.
    target = data.get("target")
    if isinstance(target, dict):
        for entry in target.values():
            if isinstance(entry, dict):
                collect(entry.get("dependencies"))
                collect(entry.get("dev-dependencies"))
                collect(entry.get("build-dependencies"))
    return names


def _dependency_names_fallback(text: str) -> set[str]:
    """Extract dependency names without tomllib (best-effort, brace-aware)."""
    names: set[str] = set()
    in_dependency_table = False
    depth = 0
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].rstrip()
        stripped = line.strip()
        if depth == 0 and stripped.startswith("[") and stripped.endswith("]"):
            header = stripped[1:-1].strip().strip('"').strip("'")
            in_dependency_table = header.split(".")[-1].endswith(_DEP_TABLE_SUFFIX)
            continue
        if in_dependency_table and depth == 0 and stripped:
            match = re.match(r"([A-Za-z0-9_-]+)\s*(?:=|\.)", stripped)
            if match:
                names.add(match.group(1))
        depth += stripped.count("{") + stripped.count("[")
        depth -= stripped.count("}") + stripped.count("]")
        depth = max(depth, 0)
    return names


def parse_cargo_dependencies(cargo_toml: Path) -> set[str]:
    text = cargo_toml.read_text(encoding="utf-8")
    if tomllib is not None:
        return _dependency_names_from_toml(tomllib.loads(text))
    return _dependency_names_fallback(text)


def _iter_rust_sources(root: Path):
    for path in root.rglob("*.rs"):
        if "target" in path.parts:
            continue
        yield path


def collect_rust_source(root: Path) -> str:
    return "\n".join(path.read_text(encoding="utf-8", errors="replace") for path in _iter_rust_sources(root))


def audit_rust(root: Path) -> list[Finding]:
    cargo_toml = root / "Cargo.toml"
    dependencies = parse_cargo_dependencies(cargo_toml)
    source = collect_rust_source(root)
    findings: list[Finding] = []
    for crate in sorted(dependencies):
        module = crate.replace("-", "_")
        if re.search(rf"\b{re.escape(module)}\b", source) is None:
            findings.append(
                Finding(
                    ecosystem="rust",
                    name=crate,
                    location="Cargo.toml",
                    reason=f"aucune référence à `{module}` dans les sources Rust",
                )
            )
    return findings


def requirement_name(line: str) -> str | None:
    stripped = line.strip()
    if not stripped or stripped.startswith("#") or stripped.startswith("-"):
        return None
    match = _REQUIREMENT_NAME.match(stripped)
    return match.group(1) if match else None


def import_module_for(package: str) -> str:
    normalized = package.lower()
    if normalized in PYTHON_IMPORT_ALIASES:
        return PYTHON_IMPORT_ALIASES[normalized]
    return package.replace("-", "_")


def collect_python_imports(package_dir: Path) -> set[str]:
    modules: set[str] = set()
    for path in package_dir.rglob("*.py"):
        for raw in path.read_text(encoding="utf-8", errors="replace").splitlines():
            match = _IMPORT_LINE.match(raw)
            if match:
                modules.add(match.group(1).split(".", 1)[0])
    return modules


def audit_python(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for requirements in sorted((root / "scripts").rglob("requirements.txt")):
        package_dir = requirements.parent
        imported = collect_python_imports(package_dir)
        for line in requirements.read_text(encoding="utf-8").splitlines():
            package = requirement_name(line)
            if package is None:
                continue
            module = import_module_for(package)
            if module not in imported:
                relative = requirements.relative_to(root).as_posix()
                findings.append(
                    Finding(
                        ecosystem="python",
                        name=package,
                        location=relative,
                        reason=f"aucun `import {module}` dans {package_dir.name}/",
                    )
                )
    return findings


def audit(root: Path) -> list[Finding]:
    return audit_rust(root) + audit_python(root)


def format_report(findings: list[Finding]) -> str:
    if not findings:
        return "Aucune librairie ou outil obsolète détecté."
    lines = [
        f"{len(findings)} dépendance(s) candidate(s) au nettoyage :",
        "",
    ]
    for finding in findings:
        lines.append(f"  [{finding.ecosystem}] {finding.name} ({finding.location})")
        lines.append(f"      → {finding.reason}")
    lines.append("")
    lines.append(
        "Ce rapport est indicatif : vérifiez chaque entrée avant de la retirer "
        "(macros, features, ou usages dynamiques peuvent échapper à l'analyse statique)."
    )
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Propose les librairies et outils de code obsolètes au nettoyage."
    )
    parser.add_argument(
        "--project-root",
        type=Path,
        default=None,
        help="racine du projet (par défaut: détectée via Cargo.toml)",
    )
    parser.add_argument(
        "--json", action="store_true", help="émet le rapport au format JSON"
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="retourne le code 1 si au moins un candidat est détecté",
    )
    args = parser.parse_args(argv)

    start = (args.project_root or Path(__file__).resolve().parent).resolve()
    try:
        root = find_project_root(start)
    except FileNotFoundError as exc:
        print(exc, file=sys.stderr)
        return 2

    findings = audit(root)

    if args.json:
        payload = [finding.__dict__ for finding in findings]
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    else:
        print(format_report(findings))

    if args.check and findings:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
