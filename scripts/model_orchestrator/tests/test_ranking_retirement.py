from __future__ import annotations

import json
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path

from scripts.model_orchestrator.catalog import build_snapshot
from scripts.model_orchestrator.history import HistoryStore
from scripts.model_orchestrator.library import LibraryError, NeutralLibrary
from scripts.model_orchestrator.models import (
    AcquisitionOffer,
    Artifact,
    ArtifactIdentity,
    MigrationResult,
    MigrationValidation,
    PerformanceObservation,
    ToolReference,
)
from scripts.model_orchestrator.operations import recover_operation, reconcile_operations
from scripts.model_orchestrator.ranking import choose_fallback, rank_offers
from scripts.model_orchestrator.retirement import (
    RetirementError,
    RetirementTokenStore,
    create_retirement_plan,
)


def offer(provider: str, **values):
    defaults = dict(
        locator=f"{provider}://model",
        family="llm",
        immutable_revision="revision",
        filename="model.gguf",
        format="gguf",
        trusted_digest="a" * 64,
        network_bytes=200,
        local_copy_bytes=100,
        quantization="q4",
    )
    defaults.update(values)
    return AcquisitionOffer(provider=provider, **defaults)


class HistoryRankingTests(unittest.TestCase):
    def test_history_keeps_latest_ten_per_provider_and_kind(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = HistoryStore(Path(directory) / "history.json")
            for index in range(12):
                store.append(
                    PerformanceObservation(
                        "direct", "gguf", f"{index:02d}", True, 3, 1,
                        network_bytes=100, network_seconds=2,
                    )
                )
            rows = store.load()
            self.assertEqual(len(rows), 10)
            self.assertEqual(rows[0].timestamp, "02")
            self.assertEqual(rows[-1].timestamp, "11")

    def test_formula_cache_cold_order_confidence_and_manual_override(self) -> None:
        observations = [
            PerformanceObservation(
                "direct", "gguf", str(index), True, 4, 1,
                network_bytes=100, local_copy_bytes=50,
                network_seconds=2, local_copy_seconds=1,
            )
            for index in range(3)
        ]
        observations.append(
            PerformanceObservation(
                "direct", "gguf", "failure", False, 1, 1,
                failure_code="download-timeout",
            )
        )
        direct = offer("direct")
        unknown_ollama = offer("ollama", network_bytes=None, local_copy_bytes=0)
        cached_hf = offer(
            "huggingface", cache_verified=True, cached_bytes=200, local_copy_bytes=0
        )
        ranked = rank_offers((direct, unknown_ollama, cached_hf), observations)
        self.assertEqual(ranked[0].offer.provider, "huggingface")
        direct_row = next(row for row in ranked if row.offer.provider == "direct")
        self.assertAlmostEqual(direct_row.predicted_seconds, 7.0)
        self.assertAlmostEqual(direct_row.adjusted_seconds, 7.0 / 0.75)
        self.assertEqual(direct_row.confidence, "medium")
        self.assertEqual(direct_row.sample_count, 3)
        self.assertEqual(
            rank_offers((direct, cached_hf), observations, manual_provider="direct")[0].offer.provider,
            "direct",
        )

    def test_unknowns_use_configurable_cold_order_and_conversion_stays_disabled(self) -> None:
        rows = rank_offers(
            (
                offer("direct", network_bytes=None),
                offer("ollama", network_bytes=None),
                offer("huggingface", conversion_required=True, executable=False, network_bytes=None),
            ),
            (),
            cold_order=("direct", "ollama", "huggingface"),
        )
        self.assertEqual([row.offer.provider for row in rows], ["direct", "ollama", "huggingface"])
        self.assertFalse(rows[-1].offer.executable)

    def test_fallback_requires_same_exact_content_and_transport_failure(self) -> None:
        failed = offer("ollama")
        same = offer("huggingface")
        variant = replace(same, provider="direct", quantization="q5")
        self.assertEqual(
            choose_fallback(
                failed, (same, variant), failure_code="download-transport-error"
            ),
            same,
        )
        self.assertIsNone(
            choose_fallback(failed, (variant,), failure_code="download-transport-error")
        )
        self.assertIsNone(
            choose_fallback(failed, (same,), failure_code="download-checksum-mismatch")
        )


class RecoveryTests(unittest.TestCase):
    def test_actions_require_capability_and_exact_owned_operation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            library = NeutralLibrary(Path(directory) / "library")
            resumable = library.begin("resume-op", "model.gguf")
            Path(resumable.staging_path).write_bytes(b"partial")
            discardable = library.begin("discard-op", "model.gguf")
            actions = reconcile_operations(
                library,
                capabilities={
                    "resume-op": {"resume"},
                    "discard-op": {"discard-partial"},
                },
            )
            self.assertTrue(all(action.available for action in actions))
            recover_operation(
                library,
                operation_id="resume-op",
                action="resume",
                capabilities={"resume-op": {"resume"}},
            )
            recover_operation(
                library,
                operation_id="discard-op",
                action="discard-partial",
                capabilities={"discard-op": {"discard-partial"}},
            )
            self.assertIsNone(library.load_journal("discard-op"))
            with self.assertRaises(LibraryError):
                recover_operation(
                    library,
                    operation_id="../resume-op",
                    action="discard-partial",
                    capabilities={},
                )

    def test_migration_rollback_is_offered_only_with_driver_proof(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            migration_root = root / "migrations"
            migration_root.mkdir()
            (migration_root / "migration-one.json").write_text(
                json.dumps(
                    {
                        "plan": {"destination_root": str(root)},
                        "steps": [
                            {
                                "created_by_operation": True,
                                "target": "native:model",
                            }
                        ],
                    }
                )
            )
            library = NeutralLibrary(root / "library")
            without = reconcile_operations(
                library, capabilities={}, migration_journal_root=migration_root
            )[0]
            self.assertFalse(without.available)
            called = []
            recover_operation(
                library,
                operation_id="migration-one",
                action="rollback",
                capabilities={"migration-one": {"rollback"}},
                migration_journal_root=migration_root,
                rollback=called.append,
            )
            self.assertEqual(called, ["migration-one"])


def retirement_fixture(*, extra_reference=None, shared_allocation=True):
    identity = ArtifactIdentity("verified", "sha256", "a" * 64, "ollama-manifest")
    source = Artifact(
        "ollama:source",
        "/models/blobs/sha256-a",
        "llm",
        "gguf",
        identity=identity,
        logical_size=100,
        allocated_size=120,
        relationship="owner_blob",
        allocation_id="disk:inode",
        references=[ToolReference("ollama", "tiny:q4", owner=True)],
    )
    artifacts = [source]
    if shared_allocation:
        artifacts.append(
            Artifact(
                "library:source",
                "/library/model.gguf",
                "llm",
                "gguf",
                identity=identity,
                logical_size=100,
                allocated_size=120,
                relationship="canonical",
                allocation_id="disk:inode",
            )
        )
    if extra_reference is not None:
        artifacts.append(
            Artifact(
                "third-party",
                "/third/model.gguf",
                "llm",
                "gguf",
                identity=identity,
                relationship="copy",
                references=[extra_reference],
            )
        )
    snapshot = build_snapshot(
        platform="linux", artifacts=artifacts, generated_at="fresh"
    )
    validation = MigrationValidation(
        identity="passed",
        catalog="passed",
        load="passed",
        inference="unavailable",
        destination_digest="a" * 64,
    )
    migration = MigrationResult(
        "migration", True, (), validation, retirement_eligible=True
    )
    return snapshot, migration


class FakeDelete:
    def __init__(self):
        self.deleted = []

    def delete(self, native_id):
        self.deleted.append(native_id)


class RetirementTests(unittest.TestCase):
    def test_confirmed_ollama_retirement_is_state_bound_and_measured(self) -> None:
        snapshot, migration = retirement_fixture(shared_allocation=True)
        plan = create_retirement_plan(
            plan_id="retire-one",
            source_artifact_id="ollama:source",
            source_native_id="tiny:q4",
            snapshot=snapshot,
            migration_result=migration,
            migration_plan_digest="migration-digest",
            now_iso="now",
        )
        self.assertEqual(plan.avoided_bytes, 100)
        self.assertEqual(plan.estimated_reclaimable_bytes, 0)
        clock = [1000.0]
        meter = iter((10_000, 10_025))
        backend = FakeDelete()
        after = build_snapshot(
            platform="linux",
            artifacts=[snapshot.artifacts[0]],
            generated_at="after",
        )
        # Keep only the canonical library artifact, not the Ollama owner.
        after.artifacts = [
            artifact for artifact in snapshot.artifacts if artifact.artifact_id == "library:source"
        ]
        with tempfile.TemporaryDirectory() as directory:
            store = RetirementTokenStore(directory, clock=lambda: clock[0])
            token = store.issue(plan, snapshot, ttl_seconds=60)
            result = store.confirm(
                token.token,
                plan,
                fresh_snapshot=snapshot,
                backend=backend,
                reinventory=lambda: after,
                measure_free=lambda: next(meter),
            )
            self.assertEqual(backend.deleted, ["tiny:q4"])
            self.assertEqual(result.measured_freed_bytes, 25)
            self.assertEqual(result.logical_bytes, 100)
            self.assertEqual(result.avoided_bytes, 100)

    def test_changed_reference_expired_token_and_third_party_block_deletion(self) -> None:
        snapshot, migration = retirement_fixture(shared_allocation=False)
        plan = create_retirement_plan(
            plan_id="retire-stale", source_artifact_id="ollama:source",
            source_native_id="tiny:q4", snapshot=snapshot,
            migration_result=migration, migration_plan_digest="digest", now_iso="now",
        )
        backend = FakeDelete()
        clock = [0.0]
        with tempfile.TemporaryDirectory() as directory:
            store = RetirementTokenStore(directory, clock=lambda: clock[0])
            token = store.issue(plan, snapshot, ttl_seconds=10)
            changed, _migration = retirement_fixture(
                extra_reference=ToolReference("jan", "linked", owner=True),
                shared_allocation=False,
            )
            with self.assertRaises(RetirementError):
                store.confirm(
                    token.token, plan, fresh_snapshot=changed, backend=backend,
                    reinventory=lambda: changed, measure_free=lambda: 0,
                )
            self.assertEqual(backend.deleted, [])

            second = store.issue(plan, snapshot, ttl_seconds=10)
            clock[0] = 11
            with self.assertRaises(RetirementError):
                store.confirm(
                    second.token, plan, fresh_snapshot=snapshot, backend=backend,
                    reinventory=lambda: snapshot, measure_free=lambda: 0,
                )
        blocked, migration = retirement_fixture(
            extra_reference=ToolReference("comfyui", "workflow", workflow=True),
            shared_allocation=False,
        )
        with self.assertRaises(RetirementError):
            create_retirement_plan(
                plan_id="blocked", source_artifact_id="ollama:source",
                source_native_id="tiny:q4", snapshot=blocked,
                migration_result=migration, migration_plan_digest="digest", now_iso="now",
            )


if __name__ == "__main__":
    unittest.main()
