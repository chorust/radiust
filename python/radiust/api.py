"""Convenience functions for the public SDK."""

from __future__ import annotations

from typing import Any

from .client import AsyncClient, Client

__all__ = ["fetch", "afetch", "fetch_many", "afetch_many", "iter_fetch", "aiter_fetch", "download", "adownload", "read_dbz", "aread_dbz", "write", "awrite", "decode_gray_file", "adecode_gray_file"]


def fetch(query: Any, *, mode: str = "science", config: Any = None) -> Any:
    with Client(config=config) as client:
        return client.fetch(query, mode=mode)


async def afetch(query: Any, *, mode: str = "science", config: Any = None) -> Any:
    async with AsyncClient(config=config) as client:
        return await client.fetch(query, mode=mode)


def decode_gray_file(path: Any, *, frame_index: int | None = None, config: Any = None) -> Any:
    with Client(config=config) as client:
        return client.decode_gray_file(path, frame_index=frame_index)


async def adecode_gray_file(path: Any, *, frame_index: int | None = None, config: Any = None) -> Any:
    async with AsyncClient(config=config) as client:
        return await client.decode_gray_file(path, frame_index=frame_index)


def read_dbz(path: Any, *, variable: str | None = None, valid_time: str | None = None, config: Any = None) -> Any:
    with Client(config=config) as client:
        return client.read_dbz(path, variable=variable, valid_time=valid_time)


async def aread_dbz(path: Any, *, variable: str | None = None, valid_time: str | None = None, config: Any = None) -> Any:
    async with AsyncClient(config=config) as client:
        return await client.read_dbz(path, variable=variable, valid_time=valid_time)


def write(value: Any, *, output: Any = "./data", format: str = "netcdf", overwrite: bool = False, config: Any = None, **options: Any) -> Any:
    with Client(config=config) as client:
        return client.write(value, output=output, format=format, overwrite=overwrite, **options)


async def awrite(value: Any, *, output: Any = "./data", format: str = "netcdf", overwrite: bool = False, config: Any = None, **options: Any) -> Any:
    async with AsyncClient(config=config) as client:
        return await client.write(value, output=output, format=format, overwrite=overwrite, **options)


def fetch_many(query_or_refs: Any, *, mode: str = "science", config: Any = None, on_error: str = "collect", max_concurrency: int | None = None, progress: Any = None) -> Any:
    with Client(config=config) as client:
        return client.fetch_many(query_or_refs, mode=mode, on_error=on_error, max_concurrency=max_concurrency, progress=progress)


async def afetch_many(query_or_refs: Any, *, mode: str = "science", config: Any = None, on_error: str = "collect", max_concurrency: int | None = None, progress: Any = None) -> Any:
    async with AsyncClient(config=config) as client:
        return await client.fetch_many(query_or_refs, mode=mode, on_error=on_error, max_concurrency=max_concurrency, progress=progress)


def iter_fetch(query_or_refs: Any, *, mode: str = "science", config: Any = None, on_error: str = "collect", max_prefetch: int | None = None) -> Any:
    client = Client(config=config)
    stream = client.iter_fetch(query_or_refs, mode=mode, on_error=on_error, max_prefetch=max_prefetch)
    stream._owner_client = client
    return stream


async def aiter_fetch(query_or_refs: Any, *, mode: str = "science", config: Any = None, on_error: str = "collect", max_prefetch: int | None = None) -> Any:
    async with AsyncClient(config=config) as client:
        async for item in client.aiter_fetch(query_or_refs, mode=mode, on_error=on_error, max_prefetch=max_prefetch):
            yield item


def download(query_or_refs: Any, *, output: str = "./data", format: str = "netcdf", mode: str = "science", raw: bool = False, raw_only: bool = False, overwrite: bool = False, output_template: str | None = None, on_error: str = "collect", config: Any = None, progress: Any = None, **processing: Any) -> Any:
    with Client(config=config) as client:
        return client.download(query_or_refs, output=output, format=format, mode=mode, raw=raw, raw_only=raw_only, overwrite=overwrite, output_template=output_template, on_error=on_error, progress=progress, **processing)


async def adownload(query_or_refs: Any, **kwargs: Any) -> Any:
    async with AsyncClient(config=kwargs.pop("config", None)) as client:
        return await client.download(query_or_refs, **kwargs)
