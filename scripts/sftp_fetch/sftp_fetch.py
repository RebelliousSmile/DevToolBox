#!/usr/bin/env python3
"""Download an explicit list of files from an SFTP server."""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import logging
import os
import posixpath
import stat
import sys
import threading
from dataclasses import dataclass, field as dataclass_field
from pathlib import Path, PurePosixPath
from typing import Any

try:
    import paramiko
except ModuleNotFoundError:  # Message contrôlé au lieu d'une trace Python brute.
    paramiko = None  # type: ignore[assignment]

try:
    import yaml
except ModuleNotFoundError:
    yaml = None  # type: ignore[assignment]


LOG = logging.getLogger("sftp_fetch")


class ConfigurationError(ValueError):
    """Raised when the YAML configuration is invalid."""


class DependencyError(RuntimeError):
    """Raised when an optional runtime dependency is not installed."""


@dataclass(frozen=True)
class Download:
    name: str
    remote: str
    local: Path | None = None


@dataclass(frozen=True)
class RemoteFile:
    remote: str
    size: int | None = None
    mtime: int | None = None


@dataclass(frozen=True)
class Transfer:
    remote: RemoteFile
    local: Path


@dataclass
class Progress:
    total: int
    processed: int = 0
    lock: threading.Lock = dataclass_field(default_factory=threading.Lock)

    def advance(self) -> int:
        with self.lock:
            self.processed += 1
            return self.processed


def _mapping(value: Any, name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ConfigurationError(f"'{name}' doit être un objet YAML")
    return value


def _required_text(data: dict[str, Any], key: str) -> str:
    value = data.get(key)
    if not isinstance(value, str) or not value.strip():
        raise ConfigurationError(f"'{key}' doit être une chaîne non vide")
    return value.strip()


def _env_secret(auth: dict[str, Any], key: str) -> str | None:
    env_name = auth.get(key)
    if env_name is None:
        return None
    if not isinstance(env_name, str) or not env_name:
        raise ConfigurationError(f"'auth.{key}' doit contenir un nom de variable")
    value = os.environ.get(env_name)
    if value is None:
        raise ConfigurationError(f"variable d'environnement absente: {env_name}")
    return value


def load_config(path: Path) -> dict[str, Any]:
    if yaml is None:
        raise DependencyError(
            "PyYAML absent; exécutez: python -m pip install -r requirements.txt"
        )
    try:
        raw = yaml.safe_load(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise ConfigurationError(f"lecture impossible de {path}: {exc}") from exc
    except yaml.YAMLError as exc:
        raise ConfigurationError(f"YAML invalide dans {path}: {exc}") from exc

    config = _mapping(raw, "racine")
    server = _mapping(config.get("server"), "server")
    _required_text(server, "host")
    _required_text(server, "username")

    downloads = config.get("downloads")
    if not isinstance(downloads, list) or not downloads:
        raise ConfigurationError("'downloads' doit être une liste non vide")
    return config


def parse_downloads(values: list[Any]) -> list[Download]:
    result: list[Download] = []
    for index, value in enumerate(values, start=1):
        if isinstance(value, str):
            remote, local = value, None
            name = PurePosixPath(remote).name
        elif isinstance(value, dict):
            remote = _required_text(value, "remote")
            name_value = value.get("name", PurePosixPath(remote).name)
            if not isinstance(name_value, str) or not name_value.strip():
                raise ConfigurationError(
                    f"downloads[{index}].name doit être une chaîne non vide"
                )
            name = name_value.strip()
            local_value = value.get("local")
            if local_value is not None and (
                not isinstance(local_value, str) or not local_value.strip()
            ):
                raise ConfigurationError(
                    f"downloads[{index}].local doit être une chaîne non vide"
                )
            local = Path(local_value) if local_value else None
        else:
            raise ConfigurationError(
                f"downloads[{index}] doit être un chemin ou un objet"
            )

        normalized = posixpath.normpath(remote)
        if not normalized.startswith("/"):
            raise ConfigurationError(
                f"downloads[{index}].remote doit être un chemin absolu SFTP"
            )
        result.append(Download(name, normalized, local))

    names = [item.name.casefold() for item in result]
    if len(names) != len(set(names)):
        raise ConfigurationError("chaque entrée de 'downloads' doit avoir un nom unique")
    return result


def safe_local_path(destination: Path, relative: Path) -> Path:
    if relative.is_absolute():
        raise ConfigurationError(f"chemin local absolu interdit: {relative}")
    root = destination.resolve()
    candidate = (root / relative).resolve()
    if candidate != root and root not in candidate.parents:
        raise ConfigurationError(f"chemin local hors destination interdit: {relative}")
    return candidate


def default_relative_path(remote: str) -> Path:
    parts = [part for part in PurePosixPath(remote).parts if part not in ("/", "")]
    if not parts:
        raise ConfigurationError("la racine SFTP ne peut pas être téléchargée implicitement")
    return Path(*parts)


def connect(config: dict[str, Any]) -> tuple[paramiko.SSHClient, paramiko.SFTPClient]:
    if paramiko is None:
        raise DependencyError(
            "Paramiko absent; exécutez: python -m pip install -r requirements.txt"
        )
    server = _mapping(config["server"], "server")
    auth = _mapping(config.get("auth", {}), "auth")
    client = paramiko.SSHClient()

    known_hosts = server.get("known_hosts")
    if known_hosts:
        client.load_host_keys(str(Path(known_hosts).expanduser()))
    else:
        client.load_system_host_keys()
    client.set_missing_host_key_policy(paramiko.RejectPolicy())

    private_key = auth.get("private_key")
    if private_key is not None and not isinstance(private_key, str):
        raise ConfigurationError("'auth.private_key' doit être une chaîne")

    client.connect(
        hostname=_required_text(server, "host"),
        port=int(server.get("port", 22)),
        username=_required_text(server, "username"),
        password=_env_secret(auth, "password_env"),
        key_filename=str(Path(private_key).expanduser()) if private_key else None,
        passphrase=_env_secret(auth, "passphrase_env"),
        timeout=float(server.get("timeout_seconds", 15)),
        allow_agent=bool(auth.get("allow_agent", True)),
        look_for_keys=bool(auth.get("look_for_keys", True)),
    )
    return client, client.open_sftp()


def download_file(
    sftp: paramiko.SFTPClient,
    remote: str,
    local: Path,
    overwrite: bool,
    remote_size: int | None = None,
    remote_mtime: int | None = None,
    skip_unchanged: bool = True,
) -> bool:
    if local.exists() and not overwrite:
        raise FileExistsError(f"le fichier existe déjà: {local}")
    if local.exists() and skip_unchanged and remote_size is not None and remote_mtime is not None:
        local_stat = local.stat()
        if (
            local_stat.st_size == remote_size
            and abs(local_stat.st_mtime - remote_mtime) < 1
        ):
            return False
    local.parent.mkdir(parents=True, exist_ok=True)
    temporary = local.with_name(local.name + ".part")
    try:
        sftp.get(remote, str(temporary))
        os.replace(temporary, local)
        if remote_mtime is not None:
            os.utime(local, (remote_mtime, remote_mtime))
    finally:
        temporary.unlink(missing_ok=True)
    return True


def _download_batch(
    config: dict[str, Any],
    transfers: list[Transfer],
    overwrite: bool,
    skip_unchanged: bool,
    stop_on_error: bool,
    progress: Progress,
) -> list[tuple[str, Transfer, Exception | None]]:
    results: list[tuple[str, Transfer, Exception | None]] = []
    ssh, sftp = connect(config)
    try:
        for transfer in transfers:
            try:
                downloaded = download_file(
                    sftp,
                    transfer.remote.remote,
                    transfer.local,
                    overwrite,
                    transfer.remote.size,
                    transfer.remote.mtime,
                    skip_unchanged,
                )
                status = "downloaded" if downloaded else "skipped"
                results.append((status, transfer, None))
                current = progress.advance()
                if downloaded:
                    LOG.info(
                        "[%d/%d] téléchargé: %s -> %s",
                        current,
                        progress.total,
                        transfer.remote.remote,
                        transfer.local,
                    )
                else:
                    LOG.info(
                        "[%d/%d] inchangé: %s",
                        current,
                        progress.total,
                        transfer.remote.remote,
                    )
            except Exception as exc:
                results.append(("failed", transfer, exc))
                current = progress.advance()
                LOG.error(
                    "[%d/%d] échec pour %s: %s",
                    current,
                    progress.total,
                    transfer.remote.remote,
                    exc,
                )
                if stop_on_error:
                    break
    finally:
        sftp.close()
        ssh.close()
    return results


def _split_transfers(transfers: list[Transfer], workers: int) -> list[list[Transfer]]:
    batches = [[] for _ in range(min(workers, len(transfers)))]
    for index, transfer in enumerate(transfers):
        batches[index % len(batches)].append(transfer)
    return batches


def select_downloads(
    downloads: list[Download], selected_names: list[str] | None
) -> list[Download]:
    if not selected_names:
        return downloads
    requested = {name.casefold() for name in selected_names}
    available = {item.name.casefold(): item for item in downloads}
    unknown = requested - available.keys()
    if unknown:
        choices = ", ".join(item.name for item in downloads)
        raise ConfigurationError(
            f"sélection inconnue: {', '.join(sorted(unknown))}; choix: {choices}"
        )
    return [item for item in downloads if item.name.casefold() in requested]


def execute(
    config: dict[str, Any],
    dry_run: bool = False,
    selected_names: list[str] | None = None,
) -> tuple[int, int]:
    destination = Path(config.get("destination", "downloads")).expanduser()
    downloads = select_downloads(parse_downloads(config["downloads"]), selected_names)
    overwrite = bool(config.get("overwrite", False))
    skip_unchanged = bool(config.get("skip_unchanged", True))
    continue_on_error = bool(config.get("continue_on_error", True))
    try:
        parallel_downloads = int(config.get("parallel_downloads", 4))
    except (TypeError, ValueError) as exc:
        raise ConfigurationError("'parallel_downloads' doit être un entier") from exc
    if not 1 <= parallel_downloads <= 16:
        raise ConfigurationError("'parallel_downloads' doit être compris entre 1 et 16")

    if dry_run:
        for item in downloads:
            relative = item.local or default_relative_path(item.remote)
            LOG.info(
                "prévu [%s]: %s -> %s",
                item.name,
                item.remote,
                safe_local_path(destination, relative),
            )
        return 0, 0

    ssh, sftp = connect(config)
    transfers: list[Transfer] = []
    failed = 0
    try:
        for item in downloads:
            try:
                remote_stat = sftp.stat(item.remote)
                relative = item.local or default_relative_path(item.remote)
                if stat.S_ISDIR(remote_stat.st_mode):
                    raise IsADirectoryError(
                        f"{item.remote} est un dossier; seuls les fichiers sont pris en charge"
                    )
                if stat.S_ISREG(remote_stat.st_mode):
                    local = safe_local_path(destination, relative)
                    transfers.append(
                        Transfer(
                            RemoteFile(
                                item.remote,
                                getattr(remote_stat, "st_size", None),
                                getattr(remote_stat, "st_mtime", None),
                            ),
                            local,
                        )
                    )
                else:
                    raise OSError(f"type de fichier SFTP non pris en charge: {item.remote}")
            except Exception as exc:
                failed += 1
                LOG.error("échec pour %s: %s", item.remote, exc)
                if not continue_on_error:
                    break
    finally:
        sftp.close()
        ssh.close()

    if not transfers:
        return 0, failed

    if not continue_on_error:
        parallel_downloads = 1
    batches = _split_transfers(transfers, parallel_downloads)
    LOG.info(
        "%d fichier(s) à traiter avec %d connexion(s) parallèle(s)",
        len(transfers),
        len(batches),
    )
    downloaded_count = skipped_count = 0
    progress = Progress(len(transfers))
    with ThreadPoolExecutor(max_workers=len(batches)) as executor:
        futures = [
            executor.submit(
                _download_batch,
                config,
                batch,
                overwrite,
                skip_unchanged,
                not continue_on_error,
                progress,
            )
            for batch in batches
        ]
        for future in as_completed(futures):
            for status, transfer, error in future.result():
                if status == "downloaded":
                    downloaded_count += 1
                elif status == "skipped":
                    skipped_count += 1
                else:
                    failed += 1
    LOG.info(
        "transferts: %d téléchargé(s), %d inchangé(s), %d échec(s)",
        downloaded_count,
        skipped_count,
        failed,
    )
    return downloaded_count + skipped_count, failed


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Télécharge une liste explicite de fichiers via SFTP."
    )
    parser.add_argument("config", type=Path, help="fichier de configuration YAML")
    parser.add_argument(
        "--dry-run", action="store_true", help="valide et affiche les chemins sans connexion"
    )
    parser.add_argument(
        "--only",
        action="append",
        metavar="NOM",
        help="ne télécharge que cette entrée YAML (répétable)",
    )
    parser.add_argument("--verbose", action="store_true", help="active les logs détaillés")
    args = parser.parse_args(argv)
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(levelname)s %(message)s",
    )
    try:
        config = load_config(args.config)
        completed, failed = execute(
            config,
            dry_run=args.dry_run,
            selected_names=args.only,
        )
        LOG.info("terminé: %d fichier(s), %d échec(s)", completed, failed)
        return 1 if failed else 0
    except (ConfigurationError, DependencyError, OSError) as exc:
        LOG.error("%s", exc)
        return 2
    except Exception as exc:
        if paramiko is not None and isinstance(exc, paramiko.SSHException):
            LOG.error("%s", exc)
            return 2
        raise


if __name__ == "__main__":
    sys.exit(main())
