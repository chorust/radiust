from __future__ import annotations

import base64


def kitty_chunks(png: bytes, *, chunk_size: int = 4096) -> tuple[str, ...]:
    if chunk_size < 1:
        raise ValueError("chunk_size must be positive")
    encoded = base64.b64encode(png).decode("ascii")
    chunks = [encoded[index : index + chunk_size] for index in range(0, len(encoded), chunk_size)]
    if not chunks:
        chunks = [""]
    return tuple(f"\x1b_Ga=T,f=100,m={1 if index < len(chunks) - 1 else 0};{part}\x1b\\" for index, part in enumerate(chunks))
