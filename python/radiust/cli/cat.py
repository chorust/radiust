from __future__ import annotations

import sys
from pathlib import Path

import click

from ..cli.query import build_query
from ..cli.query import parse_time as _parse_time
from ..cli.safety import safe_text
from ..config import load_config
from ..display.isolated import RawPreviewAmbiguousError, preview_source_raw
from ..display.raw import preview_bytes
from ..errors import UnsupportedQueryError
from ..terminal import show
from ..terminal.capabilities import capabilities
from .progress import Progress, ProgressEvent


def _read_preview_file(path: Path, limit: int) -> bytes:
    """Enforce the byte budget during reading, including when a file grows."""
    if path.stat().st_size > limit:
        raise click.UsageError("raw image exceeds artifact byte limit")
    with path.open("rb") as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise click.UsageError("raw image exceeds artifact byte limit")
    return data


@click.command("cat")
@click.argument("source", required=False)
@click.option("--file", "file_path", type=click.Path(exists=True, dir_okay=False, path_type=Path))
@click.option("--conf", "config_path", type=click.Path(path_type=Path), default=None)
@click.option("--product")
@click.option("--station", multiple=True)
@click.option("--latest", is_flag=True)
@click.option(
    "--raw/--decoded", "raw_mode", default=True,
    help="Source mode: show original image pixels (default) or use the scientific decoder.",
)
@click.option(
    "--legacy-display", is_flag=True,
    help="Apply a matched, verified legacy display rule to the source image.",
)
@click.option("--at")
@click.option("--base-time")
@click.option("--renderer", type=click.Choice(["auto", "kitty", "iterm2", "ansi", "text"]), default="auto")
@click.option("--variable")
@click.option("--palette")
@click.option("--vmin", type=float)
@click.option("--vmax", type=float)
@click.option("--width", type=click.IntRange(min=1))
@click.option("--height", type=click.IntRange(min=1))
def cat_command(source: str | None, file_path: Path | None, config_path: Path | None, product: str | None, station: tuple[str, ...], latest: bool, raw_mode: bool, legacy_display: bool, at: str | None, base_time: str | None, renderer: str, variable: str | None, palette: str | None, vmin: float | None, vmax: float | None, width: int | None, height: int | None) -> None:
    if bool(source) == bool(file_path):
        raise click.UsageError("provide exactly one of SOURCE or --file PATH")
    context = click.get_current_context()
    mode_was_explicit = context.get_parameter_source("raw_mode") == click.core.ParameterSource.COMMANDLINE
    if file_path is not None and (mode_was_explicit or legacy_display):
        raise click.UsageError("--raw, --decoded and --legacy-display require SOURCE")
    if legacy_display and mode_was_explicit:
        raise click.UsageError("--legacy-display cannot be combined with --raw or --decoded")
    image_file = file_path is not None and file_path.suffix.lower() in {".png", ".gif"}
    raw_preview = source is not None and (raw_mode or legacy_display)
    if (raw_preview or image_file) and any(
        context.get_parameter_source(option) == click.core.ParameterSource.COMMANDLINE
        for option in ("variable", "palette", "vmin", "vmax")
    ):
        raise click.UsageError("raw image preview does not accept scientific decoding options")
    if raw_preview or image_file:
        detected = capabilities(sys.stdout)
        if not detected.is_tty and renderer != "text":
            raise click.UsageError("raw image preview requires a TTY; use --renderer text when stdout is redirected")
        if renderer == "kitty" and not detected.kitty or renderer == "iterm2" and not detected.iterm2:
            raise click.UsageError(f"{renderer} graphics capability was not confirmed")
    if file_path is not None:
        suffix = file_path.suffix.lower()
        if suffix not in {".nc", ".netcdf", ".png", ".gif"}:
            raise click.UsageError("--file accepts only NetCDF, PNG or GIF paths")
        if any((product, station, latest, base_time)):
            raise click.UsageError("file preview does not accept source query options")
        if image_file and at is not None:
            raise click.UsageError("raw image file has no selectable observation time")
        if renderer in {"kitty", "iterm2", "ansi"} and not sys.stdout.isatty():
            raise click.UsageError("image renderer requires a TTY; use --renderer text")
        at_value = _parse_time(at)
        try:
            if image_file:
                root = click.get_current_context().find_root().obj or {}
                effective = config_path if config_path is not None else root.get("config_path")
                limits = load_config(path=effective).values["runtime"]
                value = preview_bytes(_read_preview_file(file_path, int(limits.get("max_artifact_bytes", 512 * 1024 * 1024))), name=file_path.name, limits=limits)
            else:
                value = file_path
            show(value, renderer=renderer, width=width, height=height, at=at_value, variable=variable, palette=palette, vmin=vmin, vmax=vmax)
        except KeyboardInterrupt:
            raise click.exceptions.Exit(130) from None
        except (KeyError, ValueError) as exc:
            raise click.UsageError(safe_text(exc)) from None
        except Exception as exc:
            raise click.ClickException(safe_text(exc)) from None
        return
    try:
        query = build_query(source or "", product=product, stations=station, latest=latest, at=_parse_time(at), base_time=_parse_time(base_time))
    except (UnsupportedQueryError, ValueError) as exc:
        raise click.UsageError(safe_text(exc)) from None
    if renderer in {"kitty", "iterm2", "ansi"} and not sys.stdout.isatty():
        raise click.UsageError("image renderer requires a TTY; use --renderer text")
    try:
        root = click.get_current_context().find_root().obj or {}
        config_path = config_path if config_path is not None else root.get("config_path")
        config = load_config(path=config_path)
        if raw_preview:
            with Progress(quiet=bool(root.get("quiet", False))) as progress:
                preview_options = {"apply_legacy": True} if legacy_display else {}
                value = preview_source_raw(
                    query, config,
                    progress=lambda stage, done, total: progress.update(ProgressEvent(stage, done, total)),
                    **preview_options,
                )
            show(value, renderer=renderer, width=width, height=height)
            return

        from ..client import Client

        with Client(config=config) as client, Progress(quiet=bool(root.get("quiet", False))) as progress:
            field = client.fetch(query, progress=lambda stage, done, total: progress.update(ProgressEvent(stage, done, total)))
        show(field, renderer=renderer, width=width, height=height, variable=variable, palette=palette, vmin=vmin, vmax=vmax)
    except click.UsageError:
        raise
    except RawPreviewAmbiguousError as exc:
        choices = "; ".join(
            safe_text(f"product={item.get('product')} station={item.get('station') or 'unknown'} at={item.get('valid_time')}")
            for item in exc.candidates
        )
        raise click.UsageError(
            f"raw preview requires a unique frame; {exc}; {choices}; select --station/--product/--at"
        ) from None
    except KeyboardInterrupt:
        raise click.exceptions.Exit(130) from None
    except Exception as exc:
        raise click.ClickException(safe_text(exc)) from None
