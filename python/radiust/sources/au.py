"""Australia Bureau of Meteorology anonymous FTP radar acquisition."""

from __future__ import annotations

import hashlib
import re
from datetime import datetime, timezone
from pathlib import Path

from ..context import SourceContext
from ..models import Artifact, FrameRef
from ..raw import RawFrame
from .legacy import LegacyImageSource


class AuSource(LegacyImageSource):
    FTP_ROOT = "ftp://ftp.bom.gov.au/anon/gen/radar/"
    _FRAME = re.compile(r"^(IDR(?P<number>\d{2})1)\.T\.(?P<time>\d{12})\.png$")

    @staticmethod
    def _ftp_options(context: SourceContext) -> tuple[bool, int, int, int]:
        limits = context.limits
        return (
            bool(limits["allow_network"]),
            int(limits["max_artifact_bytes"]),
            max(1, int(float(limits["request_timeout"]))),
            max(1, int(float(limits["frame_deadline"]))),
        )

    async def discover_entries(self, context: SourceContext):
        from radiust import _core

        allow_public, max_bytes, request_timeout, frame_deadline = self._ftp_options(context)
        listing = await _core.ftp_nlst(
            self.FTP_ROOT,
            "anonymous",
            "anonymous",
            allow_public,
            max_bytes,
            request_timeout,
            frame_deadline,
        )
        entries = []
        for line in listing:
            filename = Path(str(line)).name
            match = self._FRAME.fullmatch(filename)
            if match is None:
                continue
            try:
                valid_time = datetime.strptime(match["time"], "%Y%m%d%H%M").replace(
                    tzinfo=timezone.utc
                )
            except ValueError:
                continue
            station = f"AU{match['number']}"
            entries.append(
                {
                    "url": self.FTP_ROOT + filename,
                    "name": filename,
                    "station": station,
                    "valid_time": valid_time,
                    "revision": f"{match[1]}-{match['time']}",
                    "metadata": {
                        "time_semantics": "filename_utc",
                        "geometry_status": "unverified",
                        "ftp_mode": "passive_plain",
                    },
                }
            )
        return entries

    async def download(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        from radiust import _core

        context.cancellation.check()
        root = context.temp_root
        if root is None:
            raise RuntimeError("source context has no temporary root")
        allow_public, max_bytes, request_timeout, frame_deadline = self._ftp_options(context)
        payload = await _core.ftp_read(
            ref.uri,
            "anonymous",
            "anonymous",
            allow_public,
            max_bytes,
            request_timeout,
            frame_deadline,
        )
        payload = bytes(payload)
        context.check_bytes(len(payload))
        context.check_frame_bytes(len(payload))
        name = Path(ref.uri).name
        self._validate_image(payload, name)
        digest = hashlib.sha256(payload).hexdigest()
        target = root / name
        target.write_bytes(payload)
        artifact = Artifact(
            name=name,
            role="data",
            media_type="image/png",
            payload=target,
            source_revision=ref.revision,
            size_bytes=len(payload),
            sha256=digest,
        )
        return RawFrame(
            ref,
            (artifact,),
            metadata={
                "bbox": None,
                "value_mode": "unsupported",
                "ftp_mode": "passive_plain",
            },
        )


__all__ = ["AuSource"]
