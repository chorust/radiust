from __future__ import annotations

import sys
from pathlib import Path

import click

from ..cache import CacheStore
from ..config import load_config
from .reporting import emit


def _store(cache_dir: Path | None, output_root: Path | None) -> CacheStore:
    config = load_config(
        {
            "cache": {"dir": str(cache_dir)} if cache_dir is not None else {},
            "storage": {"output": str(output_root)} if output_root is not None else {},
        },
        environ={},
    )
    return CacheStore(
        config.cache_dir,
        output_root=config.output_root,
        max_bytes=int(config.values["cache"]["max_bytes"]),
        max_age_days=int(config.values["cache"]["max_age_days"]),
        enabled=bool(config.values["cache"].get("enabled", True)),
    )


@click.group("cache")
def cache_command() -> None:
    """Inspect and maintain the acquisition cache."""


@cache_command.command("status")
@click.option("--cache-dir", type=click.Path(path_type=Path))
@click.option("--output-root", type=click.Path(path_type=Path))
@click.option("--json", "as_json", is_flag=True)
def status_command(cache_dir: Path | None, output_root: Path | None, as_json: bool) -> None:
    emit(_store(cache_dir, output_root).status(), as_json=as_json, command="cache status")


@cache_command.command("gc")
@click.option("--cache-dir", type=click.Path(path_type=Path))
@click.option("--output-root", type=click.Path(path_type=Path))
@click.option("--dry-run", is_flag=True)
@click.option("--json", "as_json", is_flag=True)
def gc_command(cache_dir: Path | None, output_root: Path | None, dry_run: bool, as_json: bool) -> None:
    emit(_store(cache_dir, output_root).gc(dry_run=dry_run), as_json=as_json, command="cache gc")


@cache_command.command("clear")
@click.option("--cache-dir", type=click.Path(path_type=Path))
@click.option("--output-root", type=click.Path(path_type=Path))
@click.option("--dry-run", is_flag=True)
@click.option("--yes", is_flag=True, help="Confirm removal in non-interactive use.")
@click.option("--json", "as_json", is_flag=True)
def clear_command(cache_dir: Path | None, output_root: Path | None, dry_run: bool, yes: bool, as_json: bool) -> None:
    if not dry_run and not yes:
        if not sys.stdout.isatty():
            raise click.UsageError("cache clear requires --yes in non-interactive use")
        if not click.confirm("Remove cache-owned entries?", default=False):
            emit({"schema_version": 1, "dry_run": False, "removed": []}, as_json=as_json, command="cache clear")
            return
    emit(_store(cache_dir, output_root).clear(dry_run=dry_run), as_json=as_json, command="cache clear")


__all__ = ["cache_command"]
