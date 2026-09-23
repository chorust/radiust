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
@click.option("--quiet", is_flag=True, default=None)
@click.option("--verbose", is_flag=True, default=None)
def cache_command(quiet: bool | None, verbose: bool | None) -> None:
    """Inspect and maintain the acquisition cache."""


def _options(quiet: bool | None, verbose: bool | None) -> tuple[bool, bool]:
    context = click.get_current_context()
    group = context.parent
    root = context.find_root().obj or {}
    is_quiet = bool(quiet or (group and group.params.get("quiet")) or root.get("quiet"))
    is_verbose = bool(verbose or (group and group.params.get("verbose")) or root.get("verbose"))
    if is_quiet and is_verbose:
        raise click.UsageError("--quiet and --verbose are mutually exclusive")
    return is_quiet, is_verbose


def _emit_cache(payload: dict, *, operation: str, store: CacheStore, as_json: bool,
                quiet: bool, verbose: bool, dry_run: bool | None = None) -> None:
    if verbose and not as_json:
        payload = {**payload, "diagnostics": {"operation": operation,
                  "cache_root": str(store.root), "dry_run": dry_run}}
    emit(payload, as_json=as_json, quiet=quiet and not as_json, command=f"cache {operation}")


@cache_command.command("status")
@click.option("--cache-dir", type=click.Path(path_type=Path))
@click.option("--output-root", type=click.Path(path_type=Path))
@click.option("--json", "as_json", is_flag=True)
@click.option("--quiet", is_flag=True, default=None)
@click.option("--verbose", is_flag=True, default=None)
def status_command(cache_dir: Path | None, output_root: Path | None, as_json: bool,
                   quiet: bool | None, verbose: bool | None) -> None:
    quiet, verbose = _options(quiet, verbose)
    store = _store(cache_dir, output_root)
    _emit_cache(store.status(), operation="status", store=store, as_json=as_json,
                quiet=quiet, verbose=verbose)


@cache_command.command("gc")
@click.option("--cache-dir", type=click.Path(path_type=Path))
@click.option("--output-root", type=click.Path(path_type=Path))
@click.option("--dry-run", is_flag=True)
@click.option("--json", "as_json", is_flag=True)
@click.option("--quiet", is_flag=True, default=None)
@click.option("--verbose", is_flag=True, default=None)
def gc_command(cache_dir: Path | None, output_root: Path | None, dry_run: bool, as_json: bool,
               quiet: bool | None, verbose: bool | None) -> None:
    quiet, verbose = _options(quiet, verbose)
    store = _store(cache_dir, output_root)
    _emit_cache(store.gc(dry_run=dry_run), operation="gc", store=store, as_json=as_json,
                quiet=quiet, verbose=verbose, dry_run=dry_run)


@cache_command.command("clear")
@click.option("--cache-dir", type=click.Path(path_type=Path))
@click.option("--output-root", type=click.Path(path_type=Path))
@click.option("--dry-run", is_flag=True)
@click.option("--yes", is_flag=True, help="Confirm removal in non-interactive use.")
@click.option("--json", "as_json", is_flag=True)
@click.option("--quiet", is_flag=True, default=None)
@click.option("--verbose", is_flag=True, default=None)
def clear_command(cache_dir: Path | None, output_root: Path | None, dry_run: bool, yes: bool,
                  as_json: bool, quiet: bool | None, verbose: bool | None) -> None:
    quiet, verbose = _options(quiet, verbose)
    if not dry_run and not yes:
        if not sys.stdout.isatty():
            raise click.UsageError("cache clear requires --yes in non-interactive use")
        if not click.confirm("Remove cache-owned entries?", default=False):
            store = _store(cache_dir, output_root)
            _emit_cache({"schema_version": 1, "dry_run": False, "removed": []},
                        operation="clear", store=store, as_json=as_json,
                        quiet=quiet, verbose=verbose, dry_run=False)
            return
    store = _store(cache_dir, output_root)
    _emit_cache(store.clear(dry_run=dry_run), operation="clear", store=store,
                as_json=as_json, quiet=quiet, verbose=verbose, dry_run=dry_run)


__all__ = ["cache_command"]
