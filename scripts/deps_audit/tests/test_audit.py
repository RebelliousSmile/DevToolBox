import tempfile
import unittest
from pathlib import Path

from audit import (
    _dependency_names_fallback,
    audit,
    audit_python,
    audit_rust,
    find_project_root,
    import_module_for,
    requirement_name,
)


def _write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


CARGO = """\
[package]
name = "demo"

[dependencies]
used = "1.0"
raw-window-handle = "0.6"
orphan = "2.0"
windows = { version = "0.52", features = ["Foo", "Bar"] }

[dev-dependencies]
only-in-tests = "1"
"""


class RustAuditTests(unittest.TestCase):
    def _project(self, directory: str) -> Path:
        root = Path(directory)
        _write(root / "Cargo.toml", CARGO)
        _write(
            root / "src" / "main.rs",
            "use used::thing;\nfn main() { raw_window_handle::grab(); windows::init(); }\n",
        )
        _write(root / "tests" / "it.rs", "fn t() { only_in_tests::run(); }\n")
        return root

    def test_reports_only_unreferenced_crates(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self._project(directory)
            findings = audit_rust(root)
            names = {finding.name for finding in findings}
            self.assertEqual(names, {"orphan"})

    def test_hyphenated_crate_maps_to_underscore_identifier(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self._project(directory)
            names = {finding.name for finding in audit_rust(root)}
            self.assertNotIn("raw-window-handle", names)

    def test_target_ignores_build_artifacts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self._project(directory)
            # A match inside target/ must not rescue an unused crate.
            _write(root / "target" / "debug" / "gen.rs", "orphan::x();\n")
            names = {finding.name for finding in audit_rust(root)}
            self.assertIn("orphan", names)


class FallbackParserTests(unittest.TestCase):
    def test_fallback_matches_toml_parser(self):
        names = _dependency_names_fallback(CARGO)
        self.assertEqual(
            names,
            {"used", "raw-window-handle", "orphan", "windows", "only-in-tests"},
        )

    def test_fallback_ignores_feature_lines_inside_braces(self):
        toml = "[dependencies]\nwindows = {\n  version = \"1\",\n  features = [\"A\"]\n}\n"
        self.assertEqual(_dependency_names_fallback(toml), {"windows"})


class PythonAuditTests(unittest.TestCase):
    def _project(self, directory: str) -> Path:
        root = Path(directory)
        _write(root / "Cargo.toml", "[package]\nname = \"demo\"\n")
        _write(
            root / "scripts" / "tool" / "requirements.txt",
            "# comment\nparamiko>=3.4\nPyYAML>=6\nunused-lib>=1\n-r other.txt\n",
        )
        _write(
            root / "scripts" / "tool" / "tool.py",
            "import paramiko\nimport yaml\n",
        )
        return root

    def test_reports_unused_python_package_using_import_alias(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self._project(directory)
            names = {finding.name for finding in audit_python(root)}
            self.assertEqual(names, {"unused-lib"})

    def test_requirement_name_parsing(self):
        self.assertEqual(requirement_name("paramiko>=3.4,<5"), "paramiko")
        self.assertIsNone(requirement_name("# comment"))
        self.assertIsNone(requirement_name("-r other.txt"))
        self.assertIsNone(requirement_name(""))

    def test_import_alias_mapping(self):
        self.assertEqual(import_module_for("PyYAML"), "yaml")
        self.assertEqual(import_module_for("raw-window-handle"), "raw_window_handle")


class IntegrationTests(unittest.TestCase):
    def test_find_project_root_walks_up(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _write(root / "Cargo.toml", "[package]\nname = \"demo\"\n")
            nested = root / "scripts" / "deps_audit"
            nested.mkdir(parents=True)
            self.assertEqual(find_project_root(nested), root)

    def test_audit_combines_both_ecosystems(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _write(root / "Cargo.toml", CARGO)
            _write(root / "src" / "main.rs", "fn main() {}\n")
            _write(root / "scripts" / "t" / "requirements.txt", "unused-lib>=1\n")
            _write(root / "scripts" / "t" / "t.py", "print('hi')\n")
            ecosystems = {finding.ecosystem for finding in audit(root)}
            self.assertEqual(ecosystems, {"rust", "python"})


if __name__ == "__main__":
    unittest.main()
