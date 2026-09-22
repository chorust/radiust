"""Bind TMD GIFs to the UTC timestamps rendered in their frame footers."""

from __future__ import annotations

import re
import shutil
import subprocess
from datetime import datetime, timedelta, timezone
from io import BytesIO
from itertools import pairwise

from PIL import Image

from ..errors import DecodeError

_FOOTER_BOX = (130, 665, 470, 680)
_EXPECTED_SIZE = (680, 680)
_FOOTER_DATE = re.compile(r"(?<!\d)(20\d{2})\s*-\s*(\d{2})\s*-\s*(\d{2})")
_FOOTER_CLOCK = re.compile(r"\s*(\d{2})\s*[: ]\s*(\d{2})\D{0,2}(\d{2})")


def read_tmd_footer_times(payload: bytes, *, station: str) -> tuple[datetime, ...]:
    """Read every displayed UTC timestamp, rejecting unrecognized layouts.

    TMD's GIF endpoints do not expose time in the GIF container or HTTP
    headers. The timestamp is rendered in a stable, station-specific footer;
    OCR is used only to bind identity to that printed time. It does not decode
    the radar colors or infer geometry.
    """

    executable = shutil.which("tesseract")
    if executable is None:
        raise DecodeError(
            "Tesseract is required for TMD timestamp binding ('tesseract' executable not found)"
        )

    if station not in {"cmp1", "kkn240Loop"}:
        raise DecodeError(f"unsupported TMD timestamp profile for station {station!r}")

    try:
        image = Image.open(BytesIO(payload))
    except Exception as exc:
        raise DecodeError(f"unable to inspect TMD {station} GIF timestamp: {exc}") from exc

    with image:
        if image.format != "GIF" or image.size != _EXPECTED_SIZE:
            raise DecodeError(
                f"TMD {station} timestamp profile expects a 680x680 GIF; "
                f"received {image.format or 'unknown format'} {image.size}"
            )

        frame_count = int(getattr(image, "n_frames", 1))
        if frame_count < 1 or frame_count > 12:
            raise DecodeError(f"TMD {station} GIF has unsupported frame count {frame_count}")

        times: list[datetime] = []
        for index in range(frame_count):
            image.seek(index)
            footer = (
                image.convert("L")
                .crop(_FOOTER_BOX)
                .resize(
                    ((_FOOTER_BOX[2] - _FOOTER_BOX[0]) * 6, (_FOOTER_BOX[3] - _FOOTER_BOX[1]) * 6),
                    Image.Resampling.LANCZOS,
                )
            )
            if station == "kkn240Loop":
                footer = footer.point(lambda value: 255 if value > 120 else 0)
            encoded = BytesIO()
            footer.save(encoded, format="PNG")

            try:
                result = subprocess.run(
                    [
                        executable,
                        "stdin",
                        "stdout",
                        "--psm",
                        "7",
                        "-c",
                        "tessedit_char_whitelist=0123456789:- ",
                    ],
                    input=encoded.getvalue(),
                    stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL,
                    timeout=5,
                    check=False,
                )
            except (OSError, subprocess.TimeoutExpired) as exc:
                raise DecodeError(
                    f"Tesseract could not read TMD {station} GIF frame {index}"
                ) from exc
            if result.returncode != 0:
                raise DecodeError(f"Tesseract failed on TMD {station} GIF frame {index}")

            recognized = result.stdout.decode("ascii", errors="ignore")
            date_match = _FOOTER_DATE.search(recognized)
            if date_match is None:
                raise DecodeError(
                    f"TMD {station} GIF frame {index} has no unambiguous UTC footer timestamp"
                )
            clock_match = _FOOTER_CLOCK.search(recognized, date_match.end())
            if clock_match is None:
                raise DecodeError(
                    f"TMD {station} GIF frame {index} has no unambiguous UTC footer timestamp"
                )
            try:
                observation_time = datetime(
                    *(int(value) for value in date_match.groups()),
                    *(int(value) for value in clock_match.groups()),
                    tzinfo=timezone.utc,
                )
            except ValueError as exc:
                raise DecodeError(
                    f"TMD {station} GIF frame {index} has an invalid UTC footer timestamp"
                ) from exc
            times.append(observation_time)

    if any(later <= earlier for earlier, later in pairwise(times)):
        raise DecodeError(f"TMD {station} GIF footer timestamps are not strictly increasing")
    if len(times) > 1 and any(
        later - earlier != timedelta(minutes=15) for earlier, later in pairwise(times)
    ):
        raise DecodeError(
            f"TMD {station} GIF footer timestamps do not follow the verified 15-minute cadence"
        )
    return tuple(times)


__all__ = ["read_tmd_footer_times"]
