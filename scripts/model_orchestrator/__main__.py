"""Read-only schema and fixture entry point for the model orchestrator."""

from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import asdict
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
from .catalog import build_snapshot, inventory_snapshot
from .download import create_plan, execute_plan, public_offer, resolve_request
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
    AdapterCapabilities,
    Artifact,
    ArtifactIdentity,
    LibraryRecord,
    MigrationPlan,
    RootEvidence,
    SCHEMA_VERSION,
    ToolInstallation,
    ValidationEvidence,
)
from .providers import builtin_providers
from .providers.direct import ProviderError
from .settings import ModelSettings, load_settings, save_settings


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Orchestrateur de modèles locaux")
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("schema", help="afficher la version du schéma JSON")
    subparsers.add_parser("fixture", help="émettre un catalogue déterministe de contrat")
    subparsers.add_parser("inventory", help="inventorier les modèles locaux sans mutation")
    settings = subparsers.add_parser("settings", help="afficher ou modifier la bibliothèque locale")
    settings.add_argument("--set-library-root")
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
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.command == "schema":
        print(json.dumps({"schema_version": SCHEMA_VERSION}, sort_keys=True))
        return 0
    if args.command == "inventory":
        print(json.dumps(inventory_snapshot().to_dict(), ensure_ascii=False, sort_keys=True))
        return 0
    platform_name = "windows" if sys.platform == "win32" else "linux"
    if args.command == "settings":
        settings = load_settings(platform_name=platform_name, env=os.environ)
        if args.set_library_root:
            settings = ModelSettings(args.set_library_root)
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
        print(
            json.dumps(
                [asdict(provider.status()) for provider in builtin_providers()],
                ensure_ascii=False,
                sort_keys=True,
            )
        )
        return 0
    if args.command in {"resolve", "download"}:
        request = AcquisitionRequest(
            args.locator,
            args.family,
            alternatives=tuple(getattr(args, "alternative", ())),
            user_sha256=args.sha256,
        )
        try:
            offers = resolve_request(request)
        except ProviderError as exc:
            print(json.dumps({"error_code": exc.code, "message": exc.message}, ensure_ascii=False))
            return 2
        if args.command == "resolve":
            print(
                json.dumps(
                    [asdict(public_offer(offer)) for offer in offers],
                    ensure_ascii=False,
                    sort_keys=True,
                )
            )
            return 0
        selected_root = args.root or load_settings(
            platform_name=platform_name, env=os.environ
        ).library_root
        result = execute_plan(
            create_plan(args.operation_id, offers[0]),
            library=NeutralLibrary(selected_root),
            write_event=sys.stdout.write,
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


if __name__ == "__main__":
    raise SystemExit(main())
