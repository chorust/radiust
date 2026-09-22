from __future__ import annotations

from datetime import datetime, timedelta
from pathlib import Path

import click

from ..api import download as sdk_download
from ..config import load_config
from ..models import Query, utc_datetime
from ..registry import get_source, sources
from .cache import cache_command
from .cat import cat_command
from .reporting import emit, emit_error, exit_code


def _time(value: str | None) -> datetime | None:
    if value is None:
        return None
    try:
        return utc_datetime(datetime.fromisoformat(value.replace("Z", "+00:00")))
    except ValueError as exc:
        raise click.BadParameter("time must be ISO-8601 with a timezone") from exc


def _query(source: str, product: str | None, station: tuple[str, ...], latest: bool, at: str | None, start: str | None, end: str | None, base_time: str | None, max_age: float | None) -> Query:
    return Query(source, product=product, stations=station, latest=latest, at=_time(at), start=_time(start), end=_time(end), base_time=_time(base_time), max_age=timedelta(seconds=max_age) if max_age is not None else None)


def _bbox(value: str | None) -> tuple[float, float, float, float] | None:
    if value is None:
        return None
    try:
        values = tuple(float(part.strip()) for part in value.split(","))
    except ValueError as exc:
        raise click.BadParameter("bbox must be west,south,east,north") from exc
    if len(values) != 4:
        raise click.BadParameter("bbox must be west,south,east,north")
    return values  # type: ignore[return-value]


def _root_options(local_path: Path | None, local_quiet: bool | None, local_verbose: bool | None) -> tuple[Path | None, bool, bool]:
    root = click.get_current_context().find_root().obj or {}
    config_path = local_path if local_path is not None else root.get("config_path")
    quiet = bool(local_quiet if local_quiet is not None else root.get("quiet", False))
    verbose = bool(local_verbose if local_verbose is not None else root.get("verbose", False))
    if quiet and verbose:
        raise click.UsageError("--quiet and --verbose are mutually exclusive")
    return config_path, quiet, verbose


def _config(config_path: Path | None, overrides: dict[str, object] | None = None):
    return load_config(overrides or {}, path=config_path)


@click.group()
@click.option("--conf", "config_path", type=click.Path(path_type=Path))
@click.option("--quiet", is_flag=True, default=False)
@click.option("--verbose", is_flag=True, default=False)
@click.pass_context
def main(ctx: click.Context, config_path: Path | None, quiet: bool, verbose: bool) -> None:
    """Acquire and inspect radar data."""
    if quiet and verbose:
        raise click.UsageError("--quiet and --verbose are mutually exclusive")
    ctx.ensure_object(dict)
    ctx.obj.update(config_path=config_path, quiet=quiet, verbose=verbose)


@main.command("list")
@click.argument("kind", required=False, default="sources")
@click.argument("source", required=False)
@click.option("--conf", "config_path", type=click.Path(path_type=Path), default=None)
@click.option("--quiet", is_flag=True, default=None)
@click.option("--verbose", is_flag=True, default=None)
@click.option("--json", "as_json", is_flag=True)
def list_command(kind: str, source: str | None, config_path: Path | None, quiet: bool | None, verbose: bool | None, as_json: bool) -> None:
    try:
        config_path, quiet, _verbose = _root_options(config_path, quiet, verbose)
        if kind in {"sources", "source"}:
            values = sources()
            payload = [{"id": item.id, "description": item.description, "availability": item.availability, "products": [p.id for p in item.products]} for item in values]
        elif kind in {"products", "stations"}:
            if not source:
                raise click.UsageError(f"{kind} requires a source id")
            info = get_source(source).info
            # Keep frozen model mappings intact until reporting's recursive
            # JSON projection.  dataclasses.asdict() deep-copies fields and
            # cannot copy ProductInfo.units, which is intentionally a
            # mappingproxy.
            payload = list(info.products) if kind == "products" else list(info.stations)
        else:
            info = get_source(kind).info
            payload = {"id": info.id, "description": info.description, "availability": info.availability, "products": [p.id for p in info.products], "stations": [s.id for s in info.stations]}
        emit(payload, as_json=as_json, quiet=quiet and not as_json, command="list")
    except Exception as exc:
        emit_error(exc, as_json=as_json)
        raise click.exceptions.Exit(2) from None


@main.command("discover")
@click.argument("source")
@click.option("--conf", "config_path", type=click.Path(path_type=Path), default=None)
@click.option("--product")
@click.option("--station", multiple=True)
@click.option("--latest", is_flag=True)
@click.option("--at")
@click.option("--start")
@click.option("--end")
@click.option("--base-time")
@click.option("--max-age", type=float)
@click.option("--quiet", is_flag=True, default=None)
@click.option("--verbose", is_flag=True, default=None)
@click.option("--json", "as_json", is_flag=True)
def discover_command(source: str, config_path: Path | None, product: str | None, station: tuple[str, ...], latest: bool, at: str | None, start: str | None, end: str | None, base_time: str | None, max_age: float | None, quiet: bool | None, verbose: bool | None, as_json: bool) -> None:
    try:
        from ..client import Client

        config_path, quiet, _verbose = _root_options(config_path, quiet, verbose)
        query = _query(source, product, station, latest, at, start, end, base_time, max_age)
        with Client(config=_config(config_path)) as client:
            refs = client.discover(query)
        payload = [{"source": r.source, "product": r.product, "station": r.station, "valid_time": r.valid_time.isoformat().replace("+00:00", "Z"), "logical_id": r.logical_id} for r in refs]
        emit(payload, as_json=as_json, quiet=quiet and not as_json, command="discover")
    except Exception as exc:
        emit_error(exc, as_json=as_json)
        raise click.exceptions.Exit(3 if getattr(exc, "code", "") == "no_data" else 2) from None


@main.command("download")
@click.argument("source")
@click.option("--conf", "config_path", type=click.Path(path_type=Path), default=None)
@click.option("--product")
@click.option("--station", multiple=True)
@click.option("--latest", is_flag=True)
@click.option("--at")
@click.option("--start")
@click.option("--end")
@click.option("--base-time")
@click.option("--max-age", type=float)
@click.option("--output", default="./data", type=click.Path())
@click.option("--format", "format_name", default="netcdf", type=click.Choice(["netcdf", "geotiff", "png", "zarr"]))
@click.option("--output-template")
@click.option("--raw", is_flag=True)
@click.option("--raw-only", is_flag=True)
@click.option("--overwrite", is_flag=True)
@click.option("--variable")
@click.option("--grid", type=click.Choice(["native", "geographic"]), default="native")
@click.option("--bbox")
@click.option("--resolution", type=float)
@click.option("--resampling", type=click.Choice(["nearest", "bilinear"]), default="nearest")
@click.option("--no-cache", is_flag=True)
@click.option("--cache-dir", type=click.Path())
@click.option("--access-key")
@click.option("--secret-key")
@click.option("--endpoint")
@click.option("--quiet", is_flag=True, default=None)
@click.option("--verbose", is_flag=True, default=None)
@click.option("--dry-run", is_flag=True)
@click.option("--on-error", type=click.Choice(["collect", "continue", "raise", "stop"]), default="collect")
@click.option("--json", "as_json", is_flag=True)
def download_command(source: str, config_path: Path | None, product: str | None, station: tuple[str, ...], latest: bool, at: str | None, start: str | None, end: str | None, base_time: str | None, max_age: float | None, output: str, format_name: str, output_template: str | None, raw: bool, raw_only: bool, overwrite: bool, variable: str | None, grid: str, bbox: str | None, resolution: float | None, resampling: str, no_cache: bool, cache_dir: str | None, access_key: str | None, secret_key: str | None, endpoint: str | None, quiet: bool | None, verbose: bool | None, dry_run: bool, on_error: str, as_json: bool) -> None:
    try:
        config_path, quiet, _verbose = _root_options(config_path, quiet, verbose)
        query = _query(source, product, station, latest, at, start, end, base_time, max_age)
        bbox_value = _bbox(bbox)
        if grid == "native" and (bbox_value is not None or resolution is not None):
            raise click.UsageError("--bbox and --resolution require --grid geographic")
        if grid == "geographic" and (bbox_value is None or resolution is None):
            raise click.UsageError("--grid geographic requires --bbox and --resolution")
        if raw_only and any((variable, bbox_value, resolution is not None, grid != "native", resampling != "nearest")):
            raise click.UsageError("--raw-only cannot be combined with decoded processing options")
        if dry_run:
            from ..client import Client

            with Client(config=_config(config_path)) as client:
                refs = client.discover(query)
            from ..models import DownloadReport, FrameResult

            report = DownloadReport("download", "dry-run", tuple(FrameResult(ref, "planned") for ref in refs), query={"source": source})
        else:
            config_overrides = {"storage": {"output": output}}
            if no_cache:
                config_overrides["cache"] = {"enabled": False}
            if cache_dir is not None:
                config_overrides.setdefault("cache", {})["dir"] = cache_dir
            credentials = {key: value for key, value in {"access_key": access_key, "secret_key": secret_key, "endpoint": endpoint}.items() if value is not None}
            if credentials:
                config_overrides["sources"] = {source: credentials}
            report = sdk_download(query, output=output, format=format_name, raw=raw, raw_only=raw_only, overwrite=overwrite, output_template=output_template, on_error=on_error, config=_config(config_path, config_overrides), variable=variable, grid=grid, bbox=bbox_value, resolution=resolution, resampling=resampling)
        emit(report, as_json=as_json, quiet=quiet and not as_json)
        raise click.exceptions.Exit(exit_code(report))
    except click.exceptions.Exit:
        raise
    except Exception as exc:
        emit_error(exc, as_json=as_json)
        raise click.exceptions.Exit(2) from None


@main.command("doctor")
@click.option("--source")
@click.option("--network", is_flag=True)
@click.option("--conf", "config_path", type=click.Path(path_type=Path), default=None)
@click.option("--quiet", is_flag=True, default=None)
@click.option("--verbose", is_flag=True, default=None)
@click.option("--json", "as_json", is_flag=True)
def doctor_command(source: str | None, network: bool, config_path: Path | None, quiet: bool | None, verbose: bool | None, as_json: bool) -> None:
    from .doctor import run_doctor

    try:
        config_path, quiet, _verbose = _root_options(config_path, quiet, verbose)
        payload = run_doctor(source=source, network=network, config_path=config_path)
        emit(payload, as_json=as_json, quiet=quiet and not as_json)
    except Exception as exc:
        emit_error(exc, as_json=as_json)
        raise click.exceptions.Exit(2) from None


@main.command("config")
@click.argument("action", default="show")
@click.option("--conf", "config_path", type=click.Path(path_type=Path), default=None)
@click.option("--quiet", is_flag=True, default=None)
@click.option("--verbose", is_flag=True, default=None)
@click.option("--json", "as_json", is_flag=True)
def config_command(action: str, config_path: Path | None, quiet: bool | None, verbose: bool | None, as_json: bool) -> None:
    try:
        config_path, quiet, _verbose = _root_options(config_path, quiet, verbose)
        if action != "show":
            raise click.UsageError("only config show is supported")
        emit(_config(config_path).redacted(), as_json=as_json, quiet=quiet and not as_json)
    except Exception as exc:
        emit_error(exc, as_json=as_json)
        raise click.exceptions.Exit(2) from None


main.add_command(cat_command)
main.add_command(cache_command)


if __name__ == "__main__":
    main()
