"""Convenience functions for the public SDK."""

from __future__ import annotations

from typing import Any

from .client import AsyncClient, Client
from .rendering.api import render
from .terminal.api import show

__all__ = ["fetch", "afetch", "fetch_many", "afetch_many", "iter_fetch", "aiter_fetch", "download", "adownload", "render_field", "render", "show"]


def fetch(query: Any, *, config: Any = None) -> Any:
    with Client(config=config) as client:
        return client.fetch(query)


async def afetch(query: Any, *, config: Any = None) -> Any:
    async with AsyncClient(config=config) as client:
        return await client.fetch(query)


def fetch_many(query_or_refs: Any, *, config: Any = None, on_error: str = "collect", max_concurrency: int | None = None) -> Any:
    with Client(config=config) as client:
        return client.fetch_many(query_or_refs, on_error=on_error, max_concurrency=max_concurrency)


async def afetch_many(query_or_refs: Any, *, config: Any = None, on_error: str = "collect", max_concurrency: int | None = None) -> Any:
    async with AsyncClient(config=config) as client:
        return await client.fetch_many(query_or_refs, on_error=on_error, max_concurrency=max_concurrency)


def iter_fetch(query_or_refs: Any, *, config: Any = None, on_error: str = "collect", max_prefetch: int | None = None) -> Any:
    client = Client(config=config)
    stream = client.iter_fetch(query_or_refs, on_error=on_error, max_prefetch=max_prefetch)
    stream._owner_client = client
    return stream


async def aiter_fetch(query_or_refs: Any, *, config: Any = None, on_error: str = "collect", max_prefetch: int | None = None) -> Any:
    async with AsyncClient(config=config) as client:
        async for item in client.aiter_fetch(query_or_refs, on_error=on_error, max_prefetch=max_prefetch):
            yield item


def download(query_or_refs: Any, *, output: str = "./data", format: str = "netcdf", raw: bool = False, raw_only: bool = False, overwrite: bool = False, output_template: str | None = None, on_error: str = "collect", config: Any = None, **processing: Any) -> Any:
    with Client(config=config) as client:
        return client.download(query_or_refs, output=output, format=format, raw=raw, raw_only=raw_only, overwrite=overwrite, output_template=output_template, on_error=on_error, **processing)


async def adownload(query_or_refs: Any, **kwargs: Any) -> Any:
    async with AsyncClient(config=kwargs.pop("config", None)) as client:
        return await client.download(query_or_refs, **kwargs)


def render_field(value: Any, **options: Any) -> Any:
    return render(value, **options)
