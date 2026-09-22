from __future__ import annotations

import sys
from datetime import datetime
from pathlib import Path

import click

from ..config import load_config
from ..errors import UnsupportedQueryError
from ..models import Query, utc_datetime
from ..terminal import show


def _parse_time(value: str | None) -> datetime | None:
    if value is None:
        return None
    try:
        return utc_datetime(datetime.fromisoformat(value.replace("Z", "+00:00")))
    except ValueError as exc:
        raise click.BadParameter("time must be ISO-8601 with a timezone") from exc


@click.command("cat")
@click.argument("source", required=False)
@click.option("--file", "file_path", type=click.Path(exists=True, dir_okay=False, path_type=Path))
@click.option("--conf", "config_path", type=click.Path(path_type=Path), default=None)
@click.option("--product")
@click.option("--station", multiple=True)
@click.option("--latest", is_flag=True)
@click.option("--at")
@click.option("--base-time")
@click.option("--renderer", type=click.Choice(["auto", "kitty", "iterm2", "ansi", "text"]), default="auto")
@click.option("--variable")
@click.option("--palette")
@click.option("--vmin", type=float)
@click.option("--vmax", type=float)
@click.option("--width", type=click.IntRange(min=1))
@click.option("--height", type=click.IntRange(min=1))
def cat_command(source: str | None, file_path: Path | None, config_path: Path | None, product: str | None, station: tuple[str, ...], latest: bool, at: str | None, base_time: str | None, renderer: str, variable: str | None, palette: str | None, vmin: float | None, vmax: float | None, width: int | None, height: int | None) -> None:
    if bool(source) == bool(file_path):
        raise click.UsageError("provide exactly one of SOURCE or --file PATH")
    if file_path is not None:
        suffix = file_path.suffix.lower()
        if suffix not in {".nc", ".netcdf", ".png"}:
            raise click.UsageError("--file accepts only NetCDF or PNG paths")
        if any((product, station, latest, base_time)):
            raise click.UsageError("file preview does not accept source query options")
        if suffix == ".png" and any((at, variable, palette, vmin is not None, vmax is not None)):
            raise click.UsageError("PNG preview does not accept scientific decoding options")
        if renderer in {"kitty", "iterm2", "ansi"} and not sys.stdout.isatty():
            raise click.UsageError("image renderer requires a TTY; use --renderer text")
        at_value = _parse_time(at)
        try:
            show(file_path, renderer=renderer, width=width, height=height, at=at_value, variable=variable, palette=palette, vmin=vmin, vmax=vmax)
        except (KeyError, ValueError) as exc:
            raise click.UsageError(str(exc)) from None
        except Exception as exc:
            raise click.ClickException(str(exc)) from None
        return
    if not latest and at is None:
        raise UnsupportedQueryError("cat requires --latest or --at")
    query = Query(source or "", product=product, stations=station, latest=latest, at=_parse_time(at), base_time=_parse_time(base_time))
    if renderer in {"kitty", "iterm2", "ansi"} and not sys.stdout.isatty():
        raise click.UsageError("image renderer requires a TTY; use --renderer text")
    try:
        from ..client import Client

        root = click.get_current_context().find_root().obj or {}
        config_path = config_path if config_path is not None else root.get("config_path")
        with Client(config=load_config(path=config_path)) as client:
            field = client.fetch(query)
        show(field, renderer=renderer, width=width, height=height, variable=variable, palette=palette, vmin=vmin, vmax=vmax)
    except Exception as exc:
        raise click.ClickException(str(exc)) from None
