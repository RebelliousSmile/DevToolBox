from __future__ import annotations

import json
import subprocess
import sys
import unittest

from scripts.model_orchestrator.catalog import build_snapshot
from scripts.model_orchestrator.models import Artifact, ArtifactIdentity, SourceError, ToolReference


class CatalogContractTests(unittest.TestCase):
    def artifact(self, name: str, identity: ArtifactIdentity) -> Artifact:
        return Artifact(
            artifact_id=name,
            path=f"/models/{name}.gguf",
            family="llm",
            format="gguf",
            identity=identity,
            logical_size=10,
            relationship="copy",
        )

    def test_verified_sha256_is_normalized_and_grouped(self) -> None:
        identity = ArtifactIdentity(
            state="verified", algorithm="sha256", value="A" * 64, source="test"
        )
        snapshot = build_snapshot(
            platform="linux",
            artifacts=[self.artifact("one", identity), self.artifact("two", identity)],
            generated_at="fixed",
        )
        self.assertEqual(identity.value, "a" * 64)
        self.assertEqual(
            {item.duplicate_group for item in snapshot.artifacts}, {f"sha256:{'a' * 64}"}
        )

    def test_name_and_size_do_not_group_provisional_artifacts(self) -> None:
        provisional = ArtifactIdentity(state="provisional", source="filename")
        snapshot = build_snapshot(
            platform="linux",
            artifacts=[self.artifact("same", provisional), self.artifact("same-copy", provisional)],
            generated_at="fixed",
        )
        self.assertTrue(all(item.duplicate_group is None for item in snapshot.artifacts))
        self.assertTrue(all(item.protection.protected for item in snapshot.artifacts))

    def test_references_add_stable_protection_reasons(self) -> None:
        artifact = self.artifact("used", ArtifactIdentity(state="unknown"))
        artifact.references = [
            ToolReference("comfyui", "workflow.json", workflow=True),
            ToolReference("jan", "model", loaded=True),
        ]
        snapshot = build_snapshot(platform="linux", artifacts=[artifact], generated_at="fixed")
        reasons = snapshot.artifacts[0].protection.reasons
        self.assertIn("identity-unverified", reasons)
        self.assertIn("workflow:comfyui", reasons)
        self.assertIn("loaded:jan", reasons)

    def test_partial_errors_survive_beside_artifacts(self) -> None:
        artifact = self.artifact("ok", ArtifactIdentity(state="unknown"))
        snapshot = build_snapshot(
            platform="linux",
            artifacts=[artifact],
            source_errors=[SourceError("jan", "adapter-unavailable", "Jan indisponible.")],
            generated_at="fixed",
        )
        payload = snapshot.to_dict()
        self.assertEqual(len(payload["artifacts"]), 1)
        self.assertEqual(payload["source_errors"][0]["code"], "adapter-unavailable")

    def test_cli_fixture_is_deterministic_json(self) -> None:
        command = [sys.executable, "-m", "scripts.model_orchestrator", "fixture"]
        first = subprocess.check_output(command, text=True)
        second = subprocess.check_output(command, text=True)
        self.assertEqual(first, second)
        self.assertEqual(json.loads(first)["schema_version"], 1)


if __name__ == "__main__":
    unittest.main()
