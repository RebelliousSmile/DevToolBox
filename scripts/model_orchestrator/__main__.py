"""Read-only schema and fixture entry point for the model orchestrator."""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import sys
from datetime import datetime, timezone
from dataclasses import asdict, replace
from pathlib import Path

from .adapters import AdapterContext
from .adapters.comfyui import (
    ComfyUIAdapter,
    ComfyUIGuidedIntegration,
    DevToolBoxComfyLaunchBackend,
)
from .adapters.jan import JanAdapter, JanGuidedIntegration
from .adapters.lm_studio import LMSCliMigrationBackend, LMStudioMigrationDriver
from .adapters.ollama import OllamaApiMigrationBackend, OllamaMigrationDriver
from .catalog import build_snapshot, canonical_artifacts, inventory_snapshot
from .download import create_plan, execute_plan, public_offer, resolve_request, review_digest
from .library import NeutralLibrary
from .migration import (
    GuidedMigrationStore,
    MigrationExecutor,
    create_migration_plan,
    observes_exact_guided_source,
    revalidate_plan,
)
from .models import (
    AcquisitionRequest,
    AcquisitionOffer,
    AdapterCapabilities,
    Artifact,
    ArtifactIdentity,
    LibraryRecord,
    MigrationPlan,
    MigrationResult,
    MigrationStep,
    MigrationValidation,
    Protection,
    RetirementPlan,
    RootEvidence,
    SCHEMA_VERSION,
    ToolInstallation,
    ToolReference,
    SourceError,
    CatalogSnapshot,
    ValidationEvidence,
)
from .history import HistoryStore
from .operations import recover_operation, reconcile_operations
from .providers import builtin_providers
from .providers.direct import ProviderError
from .ranking import rank_offers
from .retirement import (
    OllamaApiDeleteBackend,
    RetirementTokenStore,
    create_retirement_plan,
)
from .settings import ModelSettings, load_settings, save_settings, state_root


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Orchestrateur de modèles locaux")
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("schema", help="afficher la version du schéma JSON")
    subparsers.add_parser("fixture", help="émettre un catalogue déterministe de contrat")
    event_fixture = subparsers.add_parser(
        "event-fixture", help="émettre un flux NDJSON déterministe de contrat"
    )
    event_fixture.add_argument("--operation-id", default="fixture-operation")
    cancel_fixture = subparsers.add_parser(
        "cancel-fixture", help="attendre une annulation en possédant un descendant"
    )
    cancel_fixture.add_argument("--operation-id", default="fixture-cancel")
    subparsers.add_parser("inventory", help="inventorier les modèles locaux sans mutation")
    settings = subparsers.add_parser("settings", help="afficher ou modifier la bibliothèque locale")
    settings.add_argument("--set-library-root")
    settings.add_argument("--set-provider-order")
    settings.add_argument("--set-enabled-providers")
    settings.add_argument("--xet-enabled", action=argparse.BooleanOptionalAction, default=None)
    settings.add_argument("--set-keep-patterns")
    library = subparsers.add_parser("library", help="inspecter les artefacts canoniques")
    library.add_argument("--root")
    recovery = subparsers.add_parser("recovery", help="inspecter les opérations interrompues")
    recovery.add_argument("--root")
    subparsers.add_parser("providers", help="afficher l'état des fournisseurs")
    resolve = subparsers.add_parser("resolve", help="résoudre des locators exacts")
    resolve.add_argument("locator")
    resolve.add_argument("--alternative", action="append", default=[])
    resolve.add_argument("--family", choices=("llm", "image"), required=True)
    resolve.add_argument("--sha256")
    download = subparsers.add_parser("download", help="télécharger une offre exacte")
    download.add_argument("locator")
    download.add_argument("--family", choices=("llm", "image"), required=True)
    download.add_argument("--operation-id", required=True)
    download.add_argument("--root")
    download.add_argument("--sha256")
    download.add_argument("--review-digest")
    migration_plan = subparsers.add_parser("migration-plan", help="figer un plan de migration")
    migration_plan.add_argument("--source-record", required=True)
    migration_plan.add_argument("--destination-installation", required=True)
    migration_plan.add_argument("--plan-id", required=True)
    migration_plan.add_argument("--destination-root", required=True)
    migration_plan.add_argument("--native-id", required=True)
    migration_plan.add_argument("--target-path")
    migration_plan.add_argument("--out")
    migration_validate = subparsers.add_parser("migration-validate", help="revalider un plan")
    migration_validate.add_argument("--plan", required=True)
    migration_validate.add_argument("--destination-installation", required=True)
    migration_apply = subparsers.add_parser("migration-apply", help="appliquer un plan figé")
    migration_apply.add_argument("--plan", required=True)
    migration_apply.add_argument("--destination-installation", required=True)
    migration_apply.add_argument("--journal-root", required=True)
    guided_create = subparsers.add_parser("guided-create", help="préparer une migration guidée")
    guided_create.add_argument("--source-record", required=True)
    guided_create.add_argument("--journal-root", required=True)
    guided_create.add_argument("--migration-id", required=True)
    guided_create.add_argument("--destination", choices=("jan", "comfyui"), required=True)
    guided_create.add_argument("--category")
    guided_create.add_argument("--owned-config-root")
    guided_continue = subparsers.add_parser("guided-continue", help="reprendre après l'action guidée")
    guided_continue.add_argument("--journal-root", required=True)
    guided_continue.add_argument("--migration-id", required=True)
    guided_validate = subparsers.add_parser("guided-validate", help="inspecter la condition de reprise")
    guided_validate.add_argument("--journal-root", required=True)
    guided_validate.add_argument("--migration-id", required=True)
    guided_start = subparsers.add_parser(
        "guided-start", help="préparer une intégration guidée depuis la bibliothèque"
    )
    guided_start.add_argument("--artifact-id", required=True)
    guided_start.add_argument("--destination", choices=("jan", "comfyui"), required=True)
    guided_start.add_argument("--migration-id", required=True)
    guided_start.add_argument("--category")
    recommend = subparsers.add_parser("recommend", help="classer des offres avec l'historique local")
    recommend.add_argument("--offers", required=True)
    recommend.add_argument("--history", required=True)
    recommend.add_argument("--manual-provider")
    recover_list = subparsers.add_parser("recover-list", help="lister les actions de recovery")
    recover_list.add_argument("--root", required=True)
    recover_list.add_argument("--capabilities", required=True)
    recover_list.add_argument("--migration-journal-root")
    recover = subparsers.add_parser("recover", help="exécuter une action de recovery exacte")
    recover.add_argument("--root", required=True)
    recover.add_argument("--operation-id", required=True)
    recover.add_argument("--action", choices=("resume", "discard-partial"), required=True)
    recover.add_argument("--capability", action="append", default=[])
    retirement_plan = subparsers.add_parser("retirement-plan", help="figer un retrait Ollama")
    retirement_plan.add_argument("--snapshot", required=True)
    retirement_plan.add_argument("--migration-result", required=True)
    retirement_plan.add_argument("--plan-id", required=True)
    retirement_plan.add_argument("--source-artifact-id", required=True)
    retirement_plan.add_argument("--source-native-id", required=True)
    retirement_plan.add_argument("--migration-plan-digest", required=True)
    retirement_plan.add_argument("--out", required=True)
    retirement_token = subparsers.add_parser("retirement-token", help="émettre un jeton court")
    retirement_token.add_argument("--plan", required=True)
    retirement_token.add_argument("--snapshot", required=True)
    retirement_token.add_argument("--token-root", required=True)
    retirement_token.add_argument("--ttl", type=int, default=300)
    retirement_confirm = subparsers.add_parser("retirement-confirm", help="confirmer le retrait Ollama")
    retirement_confirm.add_argument("--plan", required=True)
    retirement_confirm.add_argument("--token", required=True)
    retirement_confirm.add_argument("--token-root", required=True)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.command == "schema":
        print(json.dumps({"schema_version": SCHEMA_VERSION}, sort_keys=True))
        return 0
    if args.command == "inventory":
        print(json.dumps(_fresh_model_snapshot().to_dict(), ensure_ascii=False, sort_keys=True))
        return 0
    if args.command == "event-fixture":
        from .events import EventStream

        stream = EventStream(args.operation_id, sys.stdout.write, clock=lambda: 0.0)
        stream.progress(512, 1024)
        stream.progress(1024, 1024)
        stream.completed("fixture-gguf")
        return 0
    if args.command == "cancel-fixture":
        from .events import EventStream, NativeChildRunner

        cancel_file = Path(os.environ["DEVTOOLBOX_MODEL_CANCEL_FILE"])
        stream = EventStream(args.operation_id, sys.stdout.write)
        result = NativeChildRunner().run(
            (
                sys.executable,
                "-c",
                "import subprocess,sys,time; "
                "subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); "
                "time.sleep(60)",
            ),
            env=os.environ,
            cancelled=cancel_file.is_file,
            timeout_seconds=20,
        )
        if result.cancelled:
            stream.cancelled()
            return 1
        stream.failed("Le fixture d'annulation n'a pas été annulé.")
        return 1
    platform_name = "windows" if sys.platform == "win32" else "linux"
    if args.command == "settings":
        settings = load_settings(platform_name=platform_name, env=os.environ)
        if any(
            value is not None
            for value in (
                args.set_library_root,
                args.set_provider_order,
                args.set_enabled_providers,
                args.xet_enabled,
                args.set_keep_patterns,
            )
        ):
            settings = replace(
                settings,
                library_root=args.set_library_root or settings.library_root,
                provider_order=(
                    tuple(value.strip() for value in args.set_provider_order.split(","))
                    if args.set_provider_order is not None
                    else settings.provider_order
                ),
                enabled_providers=(
                    tuple(
                        value.strip()
                        for value in args.set_enabled_providers.split(",")
                        if value.strip()
                    )
                    if args.set_enabled_providers is not None
                    else settings.enabled_providers
                ),
                xet_enabled=(args.xet_enabled if args.xet_enabled is not None else settings.xet_enabled),
                keep_patterns=(
                    tuple(
                        value.strip()
                        for value in args.set_keep_patterns.split(",")
                        if value.strip()
                    )
                    if args.set_keep_patterns is not None
                    else settings.keep_patterns
                ),
            )
            save_settings(settings, platform_name=platform_name, env=os.environ)
            settings = load_settings(platform_name=platform_name, env=os.environ)
        print(json.dumps(asdict(settings), ensure_ascii=False, sort_keys=True))
        return 0
    if args.command in {"library", "recovery"}:
        selected_root = args.root or load_settings(
            platform_name=platform_name, env=os.environ
        ).library_root
        library = NeutralLibrary(selected_root)
        rows = library.list_records() if args.command == "library" else library.reconcile()
        print(json.dumps([asdict(row) for row in rows], ensure_ascii=False, sort_keys=True))
        return 0
    if args.command == "providers":
        model_settings = load_settings(platform_name=platform_name, env=os.environ)
        print(
            json.dumps(
                [
                    asdict(provider.status())
                    for provider in builtin_providers(
                        enabled=model_settings.enabled_providers,
                        order=model_settings.provider_order,
                        xet_enabled=model_settings.xet_enabled,
                    )
                ],
                ensure_ascii=False,
                sort_keys=True,
            )
        )
        return 0
    if args.command in {"resolve", "download"}:
        model_settings = load_settings(platform_name=platform_name, env=os.environ)
        selected_providers = builtin_providers(
            enabled=model_settings.enabled_providers,
            order=model_settings.provider_order,
            xet_enabled=model_settings.xet_enabled,
        )
        request = AcquisitionRequest(
            args.locator,
            args.family,
            alternatives=tuple(getattr(args, "alternative", ())),
            user_sha256=args.sha256,
        )
        try:
            offers = resolve_request(request, selected_providers)
        except ProviderError as exc:
            print(json.dumps({"error_code": exc.code, "message": exc.message}, ensure_ascii=False))
            return 2
        if args.command == "resolve":
            rows = []
            for offer in offers:
                row = asdict(public_offer(offer))
                row["review_digest"] = review_digest(offer)
                rows.append(row)
            print(
                json.dumps(
                    rows,
                    ensure_ascii=False,
                    sort_keys=True,
                )
            )
            return 0
        selected_root = args.root or load_settings(
            platform_name=platform_name, env=os.environ
        ).library_root
        selected = offers[0]
        if args.review_digest:
            selected = next(
                (offer for offer in offers if review_digest(offer) == args.review_digest),
                None,
            )
            if selected is None:
                from .events import EventStream

                stream = EventStream(args.operation_id, sys.stdout.write)
                stream.failed("L'offre exacte a changé depuis sa revue.")
                return 2
        result = execute_plan(
            create_plan(args.operation_id, selected),
            library=NeutralLibrary(selected_root),
            write_event=sys.stdout.write,
            providers=selected_providers,
        )
        return 0 if result.record is not None else 1
    if args.command == "migration-plan":
        source = _library_record(Path(args.source_record))
        destination = _installation(Path(args.destination_installation))
        plan = create_migration_plan(
            plan_id=args.plan_id,
            source=source,
            destination=destination,
            destination_root=args.destination_root,
            destination_native_id=args.native_id,
            target_path=args.target_path,
        )
        rendered = json.dumps(asdict(plan), ensure_ascii=False, sort_keys=True) + "\n"
        if args.out:
            Path(args.out).write_text(rendered, encoding="utf-8")
        else:
            sys.stdout.write(rendered)
        return 0
    if args.command in {"migration-validate", "migration-apply"}:
        plan_payload = json.loads(Path(args.plan).read_text(encoding="utf-8"))
        plan_payload["capabilities"] = tuple(plan_payload.get("capabilities", ()))
        plan = MigrationPlan(**plan_payload)
        destination = _installation(Path(args.destination_installation))
        if args.command == "migration-validate":
            revalidate_plan(plan, destination)
            print(json.dumps({"valid": True, "plan_id": plan.plan_id}, sort_keys=True))
            return 0
        if plan.destination_tool == "ollama":
            driver = OllamaMigrationDriver(OllamaApiMigrationBackend())
        elif plan.destination_tool == "lm-studio":
            driver = LMStudioMigrationDriver(LMSCliMigrationBackend())
        else:
            raise ValueError("Destination de migration non prise en charge")
        result = MigrationExecutor(args.journal_root).apply(
            plan, destination=destination, driver=driver
        )
        print(json.dumps(asdict(result), ensure_ascii=False, sort_keys=True))
        return 0 if result.success else 1
    if args.command == "guided-create":
        store = GuidedMigrationStore(args.journal_root)
        migration = store.create(
            migration_id=args.migration_id,
            source=_library_record(Path(args.source_record)),
            destination_tool=args.destination,
            category=args.category,
        )
        if args.destination == "jan":
            JanGuidedIntegration(store).prepare(migration)
        else:
            if not args.category or not args.owned_config_root:
                raise ValueError("ComfyUI requiert --category et --owned-config-root")
            observation = ComfyUIAdapter().inventory(AdapterContext())
            launch_backend = DevToolBoxComfyLaunchBackend(
                Path(args.journal_root) / "comfy-launch-hooks.json",
                flag_supported=os.environ.get(
                    "DEVTOOLBOX_COMFY_EXTRA_CONFIG_SUPPORTED", ""
                ).lower()
                in {"1", "true", "yes"},
            )
            ComfyUIGuidedIntegration(
                store, args.owned_config_root, launch_backend
            ).prepare(migration, observation)
        print(json.dumps(asdict(migration), ensure_ascii=False, sort_keys=True))
        return 0
    if args.command == "guided-start":
        settings = load_settings(platform_name=platform_name, env=os.environ)
        library = NeutralLibrary(settings.library_root)
        source = next(
            (
                record
                for record in library.list_records()
                if record.artifact_id == args.artifact_id
                or f"library:{record.artifact_id}" == args.artifact_id
            ),
            None,
        )
        if source is None:
            raise ValueError("Artefact canonique exact introuvable")
        operations_root = state_root(platform_name=platform_name, env=os.environ) / "model-operations"
        store = GuidedMigrationStore(operations_root)
        migration = store.create(
            migration_id=args.migration_id,
            source=source,
            destination_tool=args.destination,
            category=args.category,
        )
        if args.destination == "jan":
            JanGuidedIntegration(store).prepare(migration)
        else:
            if not args.category:
                raise ValueError("Une catégorie ComfyUI exacte est requise")
            config_root = operations_root / "comfy-model-paths"
            observation = ComfyUIAdapter().inventory(AdapterContext())
            launch_backend = DevToolBoxComfyLaunchBackend(
                operations_root / "comfy-launch-hooks.json",
                flag_supported=os.environ.get(
                    "DEVTOOLBOX_COMFY_EXTRA_CONFIG_SUPPORTED", ""
                ).lower()
                in {"1", "true", "yes"},
            )
            ComfyUIGuidedIntegration(store, config_root, launch_backend).prepare(
                migration, observation
            )
        print(json.dumps(asdict(migration), ensure_ascii=False, sort_keys=True))
        return 0
    if args.command in {"guided-continue", "guided-validate"}:
        store = GuidedMigrationStore(args.journal_root)
        migration = store.load(args.migration_id)
        if migration.destination_tool == "jan":
            observation = JanAdapter().inventory(AdapterContext())
            visible = observes_exact_guided_source(migration, observation)
            if args.command == "guided-continue":
                JanGuidedIntegration(store).resume(migration, observation)
        elif migration.destination_tool == "comfyui":
            observation = ComfyUIAdapter().inventory(AdapterContext())
            config_root = (
                Path(migration.owned_config_path).parent
                if migration.owned_config_path
                else Path(args.journal_root) / "comfy-config"
            )
            integration = ComfyUIGuidedIntegration(store, config_root, _NoComfyHook())
            visible = integration.live_visible(migration, observation)
            if args.command == "guided-continue":
                integration.resume(migration, observation)
        else:
            raise ValueError("Destination guidée inconnue")
        payload = asdict(migration)
        payload["resume_condition_met"] = visible
        print(json.dumps(payload, ensure_ascii=False, sort_keys=True))
        return 0
    if args.command == "recommend":
        offers_payload = json.loads(Path(args.offers).read_text(encoding="utf-8"))
        offers = [AcquisitionOffer(**row) for row in offers_payload]
        ranked = rank_offers(
            offers,
            HistoryStore(args.history).load(),
            cold_order=load_settings(platform_name=platform_name, env=os.environ).provider_order,
            manual_provider=args.manual_provider,
        )
        payload = []
        for row in ranked:
            rendered = asdict(row)
            rendered["offer"] = asdict(public_offer(row.offer))
            payload.append(rendered)
        print(json.dumps(payload, ensure_ascii=False, sort_keys=True))
        return 0
    if args.command == "recover-list":
        capabilities = {
            key: set(values)
            for key, values in json.loads(
                Path(args.capabilities).read_text(encoding="utf-8")
            ).items()
        }
        rows = reconcile_operations(
            NeutralLibrary(args.root),
            capabilities=capabilities,
            migration_journal_root=args.migration_journal_root,
        )
        print(json.dumps([asdict(row) for row in rows], ensure_ascii=False, sort_keys=True))
        return 0
    if args.command == "recover":
        recover_operation(
            NeutralLibrary(args.root),
            operation_id=args.operation_id,
            action=args.action,
            capabilities={args.operation_id: set(args.capability)},
        )
        print(json.dumps({"operation_id": args.operation_id, "action": args.action}, sort_keys=True))
        return 0
    if args.command == "retirement-plan":
        plan = create_retirement_plan(
            plan_id=args.plan_id,
            source_artifact_id=args.source_artifact_id,
            source_native_id=args.source_native_id,
            snapshot=_snapshot(Path(args.snapshot)),
            migration_result=_migration_result(Path(args.migration_result)),
            migration_plan_digest=args.migration_plan_digest,
            now_iso=datetime.now(timezone.utc).isoformat(),
        )
        Path(args.out).write_text(
            json.dumps(asdict(plan), ensure_ascii=False, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        return 0
    if args.command == "retirement-token":
        plan = RetirementPlan(**json.loads(Path(args.plan).read_text(encoding="utf-8")))
        token = RetirementTokenStore(args.token_root).issue(
            plan, _snapshot(Path(args.snapshot)), ttl_seconds=args.ttl
        )
        print(json.dumps(asdict(token), ensure_ascii=False, sort_keys=True))
        return 0
    if args.command == "retirement-confirm":
        plan = RetirementPlan(**json.loads(Path(args.plan).read_text(encoding="utf-8")))
        fresh = _fresh_model_snapshot()
        store = RetirementTokenStore(args.token_root)
        result = store.confirm(
            args.token,
            plan,
            fresh_snapshot=fresh,
            backend=OllamaApiDeleteBackend(),
            reinventory=_fresh_model_snapshot,
        )
        print(json.dumps(asdict(result), ensure_ascii=False, sort_keys=True))
        return 0
    if args.command != "fixture":
        raise AssertionError(f"commande non traitée : {args.command}")
    artifact = Artifact(
        artifact_id="fixture-gguf",
        path="/fixtures/model.gguf",
        family="llm",
        format="gguf",
        identity=ArtifactIdentity(
            state="verified", algorithm="sha256", value="a" * 64, source="fixture"
        ),
        logical_size=1024,
        allocated_size=4096,
        relationship="canonical",
    )
    snapshot = build_snapshot(
        platform="fixture", artifacts=[artifact], generated_at="2026-08-28T00:00:00+00:00"
    )
    print(json.dumps(snapshot.to_dict(), ensure_ascii=False, sort_keys=True))
    return 0


def _library_record(path: Path) -> LibraryRecord:
    payload = json.loads(path.read_text(encoding="utf-8"))
    identity = ArtifactIdentity(**payload.pop("identity"))
    validation = ValidationEvidence(**payload.pop("validation"))
    return LibraryRecord(identity=identity, validation=validation, **payload)


def _installation(path: Path) -> ToolInstallation:
    payload = json.loads(path.read_text(encoding="utf-8"))
    payload["capabilities"] = AdapterCapabilities(**payload.get("capabilities", {}))
    payload["roots"] = tuple(payload.get("roots", ()))
    payload["root_evidence"] = tuple(
        RootEvidence(**row) for row in payload.get("root_evidence", ())
    )
    return ToolInstallation(**payload)


class _NoComfyHook:
    def supported_hook(self):
        return None

    def register(self, config_path, hook):
        raise RuntimeError("Aucun hook automatique configuré")

    def unregister(self, config_path, hook):
        raise RuntimeError("Aucun hook automatique configuré")


def _snapshot(path: Path) -> CatalogSnapshot:
    payload = json.loads(path.read_text(encoding="utf-8"))
    artifacts = []
    for row in payload.get("artifacts", []):
        row["identity"] = ArtifactIdentity(**row.get("identity", {}))
        row["references"] = [ToolReference(**item) for item in row.get("references", [])]
        row["protection"] = Protection(**row.get("protection", {}))
        artifacts.append(Artifact(**row))
    installations = []
    for row in payload.get("installations", []):
        row["capabilities"] = AdapterCapabilities(**row.get("capabilities", {}))
        row["roots"] = tuple(row.get("roots", ()))
        row["root_evidence"] = tuple(
            RootEvidence(**item) for item in row.get("root_evidence", ())
        )
        installations.append(ToolInstallation(**row))
    return CatalogSnapshot(
        generated_at=payload["generated_at"],
        platform=payload["platform"],
        installations=installations,
        artifacts=artifacts,
        source_errors=[SourceError(**row) for row in payload.get("source_errors", [])],
        warnings=payload.get("warnings", []),
        schema_version=payload.get("schema_version", SCHEMA_VERSION),
    )


def _migration_result(path: Path) -> MigrationResult:
    payload = json.loads(path.read_text(encoding="utf-8"))
    payload["steps"] = tuple(MigrationStep(**row) for row in payload.get("steps", ()))
    payload["validation"] = MigrationValidation(**payload.get("validation", {}))
    return MigrationResult(**payload)


def _fresh_model_snapshot() -> CatalogSnapshot:
    snapshot = inventory_snapshot()
    platform_name = "windows" if sys.platform == "win32" else "linux"
    settings = load_settings(platform_name=platform_name, env=os.environ)
    root = settings.library_root
    records = NeutralLibrary(root).list_records()
    artifacts = [*snapshot.artifacts, *canonical_artifacts(records)]
    for artifact in artifacts:
        for pattern in settings.keep_patterns:
            if fnmatch.fnmatch(artifact.artifact_id, pattern) or fnmatch.fnmatch(
                artifact.path, pattern
            ):
                artifact.protection.reasons.append(f"keep:{pattern}")
    return build_snapshot(
        platform=snapshot.platform,
        artifacts=artifacts,
        installations=snapshot.installations,
        source_errors=snapshot.source_errors,
        warnings=snapshot.warnings,
    )


if __name__ == "__main__":
    raise SystemExit(main())
