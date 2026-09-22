from __future__ import annotations

import pytest
from radiust.errors import TransportError
from radiust.transport import HTTPTransport


def test_public_http_is_rejected_before_io() -> None:
    with pytest.raises(TransportError, match="public network"):
        HTTPTransport().get_sync("https://example.com/data.png")


def test_loopback_allowance_is_explicit() -> None:
    transport = HTTPTransport()
    transport._check("http://127.0.0.1:9/data.png")


def test_tls_compatibility_keeps_chain_and_hostname_verification_enabled() -> None:
    context = HTTPTransport()._tls_context()
    assert context.verify_mode == __import__("ssl").CERT_REQUIRED
    assert context.check_hostname is True
