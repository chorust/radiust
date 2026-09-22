from __future__ import annotations

import base64


def iterm2_sequence(png: bytes, *, width: int | None = None, height: int | None = None) -> str:
    attributes = ["inline=1"]
    if width is not None:
        attributes.append(f"width={width}")
    if height is not None:
        attributes.append(f"height={height}")
    return f"\x1b]1337;File={';'.join(attributes)}:{base64.b64encode(png).decode('ascii')}\x07"
