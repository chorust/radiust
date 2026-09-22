"""United Kingdom radar source, retained as an explicit retired adapter."""

from __future__ import annotations

from .legacy import LegacyImageSource


class UkSource(LegacyImageSource):
    """DataPoint was decommissioned; catalog metadata makes discovery fail closed."""

    historical = False


__all__ = ["UkSource"]
