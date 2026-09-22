"""Explicit provider matrix metadata without storing credential values."""

from __future__ import annotations

import os
import re
from collections.abc import Mapping
from dataclasses import dataclass
from urllib.parse import unquote, urlsplit, urlunsplit
from uuid import uuid4


@dataclass(frozen=True, slots=True)
class ProviderTarget:
    name: str
    provider: str
    uri_env: str | None
    access_key_env: str | None = None
    secret_key_env: str | None = None
    endpoint_env: str | None = None
    region_env: str | None = None

    def uri(self, environ: Mapping[str, str] | None = None) -> str | None:
        values = environ if environ is not None else os.environ
        return values.get(self.uri_env) if self.uri_env else None

    def configured(self, environ: Mapping[str, str] | None = None) -> bool:
        values = environ if environ is not None else os.environ
        if self.uri_env is None:
            return True
        if self.uri(environ) is None:
            return False
        if not self.access_key_env and not self.secret_key_env:
            return True
        return bool(values.get(self.access_key_env or "")) == bool(values.get(self.secret_key_env or ""))

    def credential_state(self, environ: Mapping[str, str] | None = None) -> str:
        values = environ if environ is not None else os.environ
        if not self.access_key_env and not self.secret_key_env:
            return "anonymous"
        has_access = bool(values.get(self.access_key_env or ""))
        has_secret = bool(values.get(self.secret_key_env or ""))
        if has_access and has_secret:
            return "configured"
        if has_access or has_secret:
            return "incomplete"
        return "anonymous"

    def endpoint(self, environ: Mapping[str, str] | None = None) -> str | None:
        values = environ if environ is not None else os.environ
        return values.get(self.endpoint_env) if self.endpoint_env else None

    def region(self, environ: Mapping[str, str] | None = None) -> str | None:
        values = environ if environ is not None else os.environ
        return values.get(self.region_env) if self.region_env else None

    def isolated_uri(
        self,
        environ: Mapping[str, str] | None = None,
        *,
        run_id: str | None = None,
    ) -> str:
        """Append a unique child path under an explicitly named test prefix."""
        if self.uri_env is None:
            raise ValueError(f"{self.name} does not use an object-store URI")
        if not self.configured(environ):
            raise ValueError(f"{self.name} status=unverified: URI or credential pair is missing")
        value = self.uri(environ)
        if value is None:
            raise ValueError(f"{self.name} status=unverified: URI is missing")
        parsed = urlsplit(value)
        if (
            parsed.scheme != self.provider
            or not parsed.netloc
            or parsed.username
            or parsed.password
            or parsed.query
            or parsed.fragment
        ):
            raise ValueError(f"{self.name} status=unverified: URI must be a clean {self.provider} URI")
        components = [unquote(part) for part in parsed.path.split("/") if part]
        if any(part in {".", ".."} or "/" in part or "\\" in part for part in components):
            raise ValueError(f"{self.name} status=unverified: test prefix contains an unsafe path")
        if not any("test" in part.lower() for part in components):
            raise ValueError(f"{self.name} status=unverified: URI path must include a dedicated test prefix")
        selected_run_id = run_id or uuid4().hex
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9-]{0,63}", selected_run_id):
            raise ValueError("provider run id must be a safe path component")
        path = parsed.path.rstrip("/") + f"/radiust-validation-{selected_run_id}"
        return urlunsplit((parsed.scheme, parsed.netloc, path, "", ""))

    def report(self, environ: Mapping[str, str] | None = None) -> dict[str, str | bool | None]:
        return {
            "name": self.name,
            "provider": self.provider,
            "configured": self.configured(environ),
            "credential_state": self.credential_state(environ),
            "uri_env": self.uri_env,
            "status": "configured_unverified" if self.configured(environ) else "unverified",
        }


def provider_matrix() -> tuple[ProviderTarget, ...]:
    return (
        ProviderTarget("local", "local", None),
        ProviderTarget(
            "aws-s3",
            "s3",
            "RADIUST_PROVIDER_AWS_S3_URI",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            region_env="AWS_REGION",
        ),
        ProviderTarget(
            "s3-compatible",
            "s3",
            "RADIUST_PROVIDER_S3_URI",
            "RADIUST_PROVIDER_S3_ACCESS_KEY",
            "RADIUST_PROVIDER_S3_SECRET_KEY",
            endpoint_env="RADIUST_PROVIDER_S3_ENDPOINT",
            region_env="RADIUST_PROVIDER_S3_REGION",
        ),
        ProviderTarget(
            "aliyun-oss",
            "oss",
            "RADIUST_PROVIDER_OSS_URI",
            "RADIUST_PROVIDER_OSS_ACCESS_KEY",
            "RADIUST_PROVIDER_OSS_SECRET_KEY",
            endpoint_env="RADIUST_PROVIDER_OSS_ENDPOINT",
            region_env="RADIUST_PROVIDER_OSS_REGION",
        ),
    )


__all__ = ["ProviderTarget", "provider_matrix"]
